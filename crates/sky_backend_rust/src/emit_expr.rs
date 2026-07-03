//! Expression and function emission (M0 subset).
//!
//! Ports the M0-relevant arms of `Sky/Generate/Rust/Builder/ExprEmitter.hs` and
//! the function-item shape from `ModuleEmitter.hs`. The byte target is golden
//! `main.rs` lines 129–137 (`main_update` / `sky_main`).

use core::fmt::Write as _;

use sky_diagnostics::{DResult, Diagnostic, LowerError, Span};
use sky_intern::Symbol;
use sky_ir::{BinOp, BoundSet, Callee, Expr, Func, IrType, KernelFn, Match, Pat};

use crate::EmitCtx;
use crate::emit_types::{GenericScope, render_type};
use crate::naming::kernel_name;

/// The deepest expression nesting the backend will descend before failing fast.
///
/// `emit_expr` recurses one Rust stack frame per IR-expression level (`BinOp`
/// operands, call arguments, match scrutinee/arm bodies). An adversarially or
/// buggily deep IR spine would otherwise overflow the native stack with no
/// diagnostic. The parser already caps *source* nesting at 256 (SKY-P0003);
/// this matching bound is defence-in-depth against an IR produced past that —
/// well below the native stack ceiling (≤ 2 MB default thread stack), so the
/// guard fires first. Sized conservatively to leave headroom for the frame size
/// of `emit_expr_at` in debug builds.
const MAX_EMIT_DEPTH: u16 = 96;

/// One indentation level: four spaces, matching the golden's formatting.
fn indent_of(level: usize) -> String {
    "    ".repeat(level)
}

/// The Rust spelling of a binary operator for use in infix emission.
///
/// Every Sky M1-core arithmetic/comparison/boolean operator maps to the
/// identically-spelled Rust operator except `/=` (Sky inequality → Rust `!=`).
///
/// `IntDiv` and `Append` are listed here only to keep the match exhaustive
/// (a compiler requirement when a new `BinOp` variant is added); they MUST
/// NOT reach the infix branch:
/// - `BinOp::IntDiv` emits as a helper call, never as infix — `//` is a Rust
///   line comment, so reaching this arm is a codegen bug, caught at compile time
///   by the exhaustive `match op` in `Expr::BinOp`.
/// - `BinOp::Append` emits as `format!`, similarly intercepted before this arm.
const fn op_str(op: BinOp) -> &'static str {
    match op {
        BinOp::Add => "+",
        BinOp::Sub => "-",
        BinOp::Mul => "*",
        BinOp::Div => "/",
        BinOp::Eq => "==",
        BinOp::Neq => "!=",
        BinOp::Lt => "<",
        BinOp::Gt => ">",
        BinOp::Le => "<=",
        BinOp::Ge => ">=",
        BinOp::And => "&&",
        BinOp::Or => "||",
        // `IntDiv` is routed through sky_runtime::math::sky_int_div in the
        // Expr::BinOp handler — it must never reach the infix `op_str` path.
        // `//` here is a Rust line comment, making silent corruption impossible:
        // any accidental infix emit would comment out the rest of the expression.
        // Listed for exhaustiveness so adding a future BinOp variant is a
        // compile error here, not a silent gap.
        BinOp::IntDiv => "//",
        // `Append` has no infix Rust form; the `BinOp` arm routes it to
        // `format!` before reaching here. Listed for exhaustiveness.
        BinOp::Append => "++",
    }
}

/// Render an `f64` as a Rust literal that is guaranteed to TYPE as `f64`.
///
/// Rust's default `f64` Display drops the decimal point for a whole number
/// (`3.0` prints as `3`), and a bare `3` types as an integer — so a whole-number
/// float literal must keep (or regain) a decimal point. The shortest round-trip
/// Display is used (so the emitted literal parses back to the same bit pattern),
/// and `.0` is appended only when the rendering carries no `.`/`e` exponent
/// marker. A non-finite value (an over-range lexeme reads back as `inf`) can have
/// no decimal literal, so it renders through the `f64` associated constants,
/// keeping the emission total and valid Rust.
fn float_literal(f: f64) -> String {
    if f.is_nan() {
        return "f64::NAN".to_owned();
    }
    if f.is_infinite() {
        return if f < 0.0 {
            "f64::NEG_INFINITY"
        } else {
            "f64::INFINITY"
        }
        .to_owned();
    }
    let s = format!("{f}");
    if s.bytes().any(|b| b == b'.' || b == b'e' || b == b'E') {
        s
    } else {
        format!("{s}.0")
    }
}

/// The Rust name of a call target.
pub fn callee_name(ctx: &EmitCtx, callee: &Callee) -> DResult<String> {
    match callee {
        Callee::Func(id) => Ok(ctx.func_name(*id)?.to_owned()),
        Callee::Kernel(k) => Ok(kernel_name(*k).to_owned()),
    }
}

/// Whether a kernel's runtime function takes its two arguments in the OPPOSITE
/// order to the Sky call. The `Maybe` / `Result` mapping combinators are
/// container-first in the runtime (`sky_maybe_map(m, f)`) but function-first in
/// Sky (`Maybe.map f m`); every other wired kernel matches the Sky order. Used by
/// the [`Expr::Call`] emitter to reverse the rendered argument list.
const fn kernel_swaps_first_two(k: sky_ir::KernelFn) -> bool {
    matches!(
        k,
        KernelFn::MaybeMap
            | KernelFn::MaybeAndThen
            | KernelFn::ResultMap
            // `Result.andThen f r` / `Result.mapError f r` — Sky passes the
            // fn first; the runtime `sky_result_and_then(r, f)` /
            // `sky_result_map_error(r, f)` take the container first.
            | KernelFn::ResultAndThen
            | KernelFn::ResultMapError
            // `JsonDec.andThen f decoder` — Sky passes fn first; Rust runtime
            // `decode_and_then(decoder, f)` expects decoder first.
            | KernelFn::JsonDecAndThen
            // `Task.andThen f task` — Sky passes continuation first; Rust runtime
            // `task_and_then(task, f)` expects effect first so Rust evaluates the
            // effect expression BEFORE the continuation closure captures shared Db
            // pool values, preventing E0507 / E0382 move conflicts at connect-use
            // sites (see `Expr::TaskSeq` below for the auto-force counterpart).
            | KernelFn::TaskAndThen
    )
}

/// Handle Http kernel calls that require custom argument wrapping.
///
/// Returns `Some(emitted)` for the three network-effect kernels
/// (`HttpGet` / `HttpPost` / `HttpRequest`), which need a `task_map`
/// closure that converts `sky_runtime::HttpResponse` into the synthesised
/// Sky record struct for `{body, headers, status}`.
///
/// `HttpParseQuery` returns `HashMap<String,String>` which is exactly
/// `Dict String String` — the standard `Expr::Call` emitter is correct
/// and this function returns `None` for it.
///
/// The conversion is a PURE FIELD-FOR-FIELD MOVE — no validation, no
/// second parse boundary. All guards (SSRF, body cap, timeout, error
/// redaction) live inside the runtime entry points; the emitter only
/// wraps the response record.
///
/// All three network kernels emit explicit `::<SkyError>` turbofish so
/// Rust can infer the error channel even when the `Err` arm is discarded.
/// The closure parameter is typed `|r: sky_runtime::HttpResponse|` so
/// the closure's input type is never ambiguous.
///
/// Factored out of `emit_expr_at` to keep that function's stack frame
/// small (matching the `emit_json_decoder_call` pattern).
#[inline(never)]
fn emit_http_call(
    ctx: &EmitCtx,
    callee: &Callee,
    args: &[Expr],
    indent: usize,
    child: u16,
    generics: GenericScope,
) -> DResult<Option<String>> {
    // Only the three network kernels need special treatment.
    let Callee::Kernel(k @ (KernelFn::HttpGet | KernelFn::HttpPost | KernelFn::HttpRequest)) =
        callee
    else {
        return Ok(None);
    };

    // Resolve the synthesised struct name for the HttpResponse field set
    // {body, headers, status}. The field set is sorted alphabetically;
    // these three names are already in alphabetical order.
    let resp_key: Vec<String> = vec!["body".to_owned(), "headers".to_owned(), "status".to_owned()];
    let resp_struct = ctx
        .record_struct_by_key(&resp_key)
        .map_err(|_| Diagnostic::CompilerBug {
            where_: "sky_backend_rust::emit_http_call",
            detail: "no synthesised struct for HttpResponse fieldset {body, headers, status}; \
                     the lowerer must surface the HttpResponse record type before emission"
                .to_owned(),
        })?;
    let resp_name = &resp_struct.name;

    // Build the task_map conversion closure shared by all three variants.
    // The closure is a pure field-for-field move — soundness note: all
    // fields are owned (String / i64 / HashMap), no borrows, no boxing.
    let conv = format!(
        "|r: sky_runtime::HttpResponse| {resp_name} {{ \
         body: r.body, headers: r.headers, status: r.status }}"
    );

    match k {
        KernelFn::HttpGet => {
            // Http.get : String -> Task Error HttpResponse
            // args[0] = url : String
            let url = args.first().ok_or_else(|| Diagnostic::CompilerBug {
                where_: "sky_backend_rust::emit_http_call",
                detail: "HttpGet expects exactly 1 argument (url)".to_owned(),
            })?;
            let url_s = emit_expr_at(ctx, url, indent, child, generics)?;
            Ok(Some(format!(
                "task_map(Box::new({conv}), \
                 sky_runtime::http_client::http_get::<SkyError>({url_s}))"
            )))
        }
        KernelFn::HttpPost => {
            // Http.post : String -> String -> Task Error HttpResponse
            // args[0] = url, args[1] = body
            let url = args.first().ok_or_else(|| Diagnostic::CompilerBug {
                where_: "sky_backend_rust::emit_http_call",
                detail: "HttpPost expects 2 arguments (url, body)".to_owned(),
            })?;
            let body_arg = args.get(1).ok_or_else(|| Diagnostic::CompilerBug {
                where_: "sky_backend_rust::emit_http_call",
                detail: "HttpPost expects 2 arguments (url, body)".to_owned(),
            })?;
            let url_s = emit_expr_at(ctx, url, indent, child, generics)?;
            let body_s = emit_expr_at(ctx, body_arg, indent, child, generics)?;
            Ok(Some(format!(
                "task_map(Box::new({conv}), \
                 sky_runtime::http_client::http_post::<SkyError>({url_s}, {body_s}))"
            )))
        }
        KernelFn::HttpRequest => {
            // Http.request : HttpRequest -> Task Error HttpResponse
            // args[0] = req : HttpRequest (synthesised record struct)
            //
            // Resolve the request struct field set for {body, followRedirects,
            // headers, maxRedirects, method, timeout, url} (alphabetical).
            let req_key: Vec<String> = vec![
                "body".to_owned(),
                "followRedirects".to_owned(),
                "headers".to_owned(),
                "maxRedirects".to_owned(),
                "method".to_owned(),
                "timeout".to_owned(),
                "url".to_owned(),
            ];
            let req_struct =
                ctx.record_struct_by_key(&req_key)
                    .map_err(|_| Diagnostic::CompilerBug {
                        where_: "sky_backend_rust::emit_http_call",
                        detail: "no synthesised struct for HttpRequest fieldset \
                             {body, followRedirects, headers, maxRedirects, method, timeout, url}; \
                             the lowerer must surface the HttpRequest record type"
                            .to_owned(),
                    })?;
            // Suppress the unused warning — the struct name is only needed for
            // the diagnostic above; field access uses the `__req` binding below.
            let _ = &req_struct.name;

            let req_expr = args.first().ok_or_else(|| Diagnostic::CompilerBug {
                where_: "sky_backend_rust::emit_http_call",
                detail: "HttpRequest expects exactly 1 argument (req record)".to_owned(),
            })?;
            let req_s = emit_expr_at(ctx, req_expr, indent, child, generics)?;
            // Bind the synthesised request struct once (`__req`) and move each
            // field exactly once into `sky_runtime::HttpRequest`. The runtime
            // struct uses `#[allow(non_snake_case)]` camelCase field names
            // verbatim — `followRedirects`, `maxRedirects` — so they must match
            // here exactly. The Sky names emit via `emit_ident` as-is (none are
            // Rust keywords); the runtime names are string literals.
            Ok(Some(format!(
                "({{ let __req = {req_s}; task_map(Box::new({conv}), \
                 sky_runtime::http_client::http_request::<SkyError>(\
                 sky_runtime::HttpRequest {{ \
                 method: __req.method, url: __req.url, body: __req.body, \
                 headers: __req.headers, timeout: __req.timeout, \
                 followRedirects: __req.followRedirects, \
                 maxRedirects: __req.maxRedirects }}))\
                 }})"
            )))
        }
        // The non-network Http kernels (HttpParseQuery) fall through to
        // `None` — handled above by the `match k` guard.
        _ => Ok(None),
    }
}

/// Handle Http builder kernel calls that emit inline struct construction or
/// clone-and-reassign record updates.
///
/// Returns `Some(emitted)` for the five pure builder kernels:
///
/// * **`HttpDefaultRequest url`** — emits a struct literal with sensible
///   defaults: `method = "GET"`, `body = ""`, `headers = []`,
///   `timeout = 30000`, `followRedirects = true`, `maxRedirects = 10`.
///
/// * **`HttpWithMethod m req`**, **`HttpWithTimeout t req`**,
///   **`HttpWithBody b req`** — each emits a clone-and-reassign block
///   (`{ let mut __sky_rec = (req).clone(); __sky_rec.field = val; __sky_rec }`)
///   matching the `emit_update` pattern so the source record is moved once.
///
/// * **`HttpWithHeader k v req`** — emits a prepend:
///   `{ let mut __sky_rec = (req).clone(); __sky_rec.headers.insert(0, (k, v)); __sky_rec }`.
///   PREPEND (cons-prepend) matches the Go reference implementation in `Http.sky`
///   (`{ req | headers = (k, v) :: req.headers }`), so `withHeader "B" "2"` after
///   `withHeader "A" "1"` yields `B:2,A:1` in iteration order.
///
/// Returns `None` for any other callee — the caller falls through to the
/// standard call path. Factored out of `emit_expr_at` to keep its stack frame
/// small (same rationale as `emit_http_call`).
#[inline(never)]
#[allow(clippy::too_many_lines)] // 5 match arms × ~20 lines = inherently verbose but linear
fn emit_http_builder_call(
    ctx: &EmitCtx,
    callee: &Callee,
    args: &[Expr],
    indent: usize,
    child: u16,
    generics: GenericScope,
) -> DResult<Option<String>> {
    let Callee::Kernel(
        k @ (KernelFn::HttpDefaultRequest
        | KernelFn::HttpWithMethod
        | KernelFn::HttpWithTimeout
        | KernelFn::HttpWithBody
        | KernelFn::HttpWithHeader),
    ) = callee
    else {
        return Ok(None);
    };

    // Resolve the synthesised struct for the HttpRequest fieldset
    // {body, followRedirects, headers, maxRedirects, method, timeout, url}.
    // The field set is sorted alphabetically and matches the `req_key` in
    // `emit_http_call`. The builder always returns this same struct type.
    let req_key: Vec<String> = vec![
        "body".to_owned(),
        "followRedirects".to_owned(),
        "headers".to_owned(),
        "maxRedirects".to_owned(),
        "method".to_owned(),
        "timeout".to_owned(),
        "url".to_owned(),
    ];
    let req_name = ctx
        .record_struct_by_key(&req_key)
        .map_err(|_| Diagnostic::CompilerBug {
            where_: "sky_backend_rust::emit_http_builder_call",
            detail: "no synthesised struct for HttpRequest fieldset \
                 {body, followRedirects, headers, maxRedirects, method, timeout, url}; \
                 the lowerer must surface the HttpRequest record type before emission"
                .to_owned(),
        })?
        .name
        .clone();

    match k {
        KernelFn::HttpDefaultRequest => {
            // defaultRequest : String -> HttpRequest  — inline struct literal
            let url = args.first().ok_or_else(|| Diagnostic::CompilerBug {
                where_: "sky_backend_rust::emit_http_builder_call",
                detail: "HttpDefaultRequest expects 1 argument (url)".to_owned(),
            })?;
            let url_s = emit_expr_at(ctx, url, indent, child, generics)?;
            Ok(Some(format!(
                "{req_name} {{ body: String::new(), followRedirects: true, \
                 headers: Vec::new(), maxRedirects: 10i64, \
                 method: \"GET\".to_string(), timeout: 30000i64, url: {url_s} }}"
            )))
        }
        KernelFn::HttpWithMethod => {
            // withMethod : String -> HttpRequest -> HttpRequest
            let m = args.first().ok_or_else(|| Diagnostic::CompilerBug {
                where_: "sky_backend_rust::emit_http_builder_call",
                detail: "HttpWithMethod expects 2 arguments (method, req)".to_owned(),
            })?;
            let req = args.get(1).ok_or_else(|| Diagnostic::CompilerBug {
                where_: "sky_backend_rust::emit_http_builder_call",
                detail: "HttpWithMethod expects 2 arguments (method, req)".to_owned(),
            })?;
            let m_s = emit_expr_at(ctx, m, indent, child, generics)?;
            let req_s = emit_expr_at(ctx, req, indent, child, generics)?;
            Ok(Some(format!(
                "{{ let mut __sky_rec = ({req_s}).clone(); \
                 __sky_rec.method = {m_s}; __sky_rec }}"
            )))
        }
        KernelFn::HttpWithTimeout => {
            // withTimeout : Int -> HttpRequest -> HttpRequest
            let t = args.first().ok_or_else(|| Diagnostic::CompilerBug {
                where_: "sky_backend_rust::emit_http_builder_call",
                detail: "HttpWithTimeout expects 2 arguments (timeout, req)".to_owned(),
            })?;
            let req = args.get(1).ok_or_else(|| Diagnostic::CompilerBug {
                where_: "sky_backend_rust::emit_http_builder_call",
                detail: "HttpWithTimeout expects 2 arguments (timeout, req)".to_owned(),
            })?;
            let t_s = emit_expr_at(ctx, t, indent, child, generics)?;
            let req_s = emit_expr_at(ctx, req, indent, child, generics)?;
            Ok(Some(format!(
                "{{ let mut __sky_rec = ({req_s}).clone(); \
                 __sky_rec.timeout = {t_s}; __sky_rec }}"
            )))
        }
        KernelFn::HttpWithBody => {
            // withBody : String -> HttpRequest -> HttpRequest
            let b = args.first().ok_or_else(|| Diagnostic::CompilerBug {
                where_: "sky_backend_rust::emit_http_builder_call",
                detail: "HttpWithBody expects 2 arguments (body, req)".to_owned(),
            })?;
            let req = args.get(1).ok_or_else(|| Diagnostic::CompilerBug {
                where_: "sky_backend_rust::emit_http_builder_call",
                detail: "HttpWithBody expects 2 arguments (body, req)".to_owned(),
            })?;
            let b_s = emit_expr_at(ctx, b, indent, child, generics)?;
            let req_s = emit_expr_at(ctx, req, indent, child, generics)?;
            Ok(Some(format!(
                "{{ let mut __sky_rec = ({req_s}).clone(); \
                 __sky_rec.body = {b_s}; __sky_rec }}"
            )))
        }
        KernelFn::HttpWithHeader => {
            // withHeader : String -> String -> HttpRequest -> HttpRequest
            // PREPENDS (key, value) — matches Go reference `(k,v) :: req.headers`.
            let k_arg = args.first().ok_or_else(|| Diagnostic::CompilerBug {
                where_: "sky_backend_rust::emit_http_builder_call",
                detail: "HttpWithHeader expects 3 arguments (key, value, req)".to_owned(),
            })?;
            let v_arg = args.get(1).ok_or_else(|| Diagnostic::CompilerBug {
                where_: "sky_backend_rust::emit_http_builder_call",
                detail: "HttpWithHeader expects 3 arguments (key, value, req)".to_owned(),
            })?;
            let req = args.get(2).ok_or_else(|| Diagnostic::CompilerBug {
                where_: "sky_backend_rust::emit_http_builder_call",
                detail: "HttpWithHeader expects 3 arguments (key, value, req)".to_owned(),
            })?;
            let k_s = emit_expr_at(ctx, k_arg, indent, child, generics)?;
            let v_s = emit_expr_at(ctx, v_arg, indent, child, generics)?;
            let req_s = emit_expr_at(ctx, req, indent, child, generics)?;
            Ok(Some(format!(
                "{{ let mut __sky_rec = ({req_s}).clone(); \
                 __sky_rec.headers.insert(0, ({k_s}, {v_s})); __sky_rec }}"
            )))
        }
        // Unreachable: the guard at the top of this function constrains `k` to the
        // five variants matched above. The `_ =>` arm keeps Rust's exhaustiveness
        // checker satisfied without introducing a catch-all over the full `KernelFn`
        // set (which would violate the no-catch-all principle for the logic above).
        _ => Ok(None),
    }
}

/// Handle Db kernel calls that require `SqlValue` / `SqlField` boundary
/// projection.
///
/// The Sky surface for parameterised Db calls (`Db.exec`, `Db.query`,
/// `Db.queryDecode`, `Db.insertFields`, `Db.updateFields`,
/// `Db.insertFieldsReturning`) passes a `List SqlValue` or
/// `List (String, SqlField)` as a plain Sky argument. The runtime's typed-param
/// functions (`db_exec_params`, `db_query_params`, …) expect `Vec<SqlParam>` /
/// `Vec<(String, Option<SqlParam>)>`. The projection is emitted INLINE at the
/// call site — the Sky list is converted with a short `.into_iter().map(…)`
/// chain so the compiler never needs separate IR types for the two.
///
/// Kernels that accept only `Db` / `String` / `Int` / plain Dict arguments (no
/// `SqlValue` / `SqlField` in the parameter list) return `None` here and fall
/// through to the standard `name(args)` path.
///
/// Factored out of `emit_expr_at` to keep that function's stack frame small
/// (same rationale as `emit_http_call`).
#[inline(never)]
#[allow(clippy::too_many_lines)]
// linear dispatch over many projection cases
// The match below lists standard-path Db kernels explicitly (same Ok(None) body
// as the wildcard) so that any future param-taking Db kernel added to `KernelFn`
// that NEEDS a custom arm causes a *compile error* here — not a silent
// exit-0-then-cargo-fail when `_ => Ok(None)` swallows it.
// `match_same_arms` fires because both the list and `_` return `Ok(None)`; the
// documentation value justifies the suppression.
#[allow(clippy::match_same_arms)]
fn emit_db_call(
    ctx: &EmitCtx,
    callee: &Callee,
    args: &[Expr],
    indent: usize,
    child: u16,
    generics: GenericScope,
) -> DResult<Option<String>> {
    // Fast path: not a Db kernel at all.
    let Callee::Kernel(k) = callee else {
        return Ok(None);
    };

    // Helper: emit a single arg by index, returning a CompilerBug on miss.
    macro_rules! arg {
        ($idx:expr, $name:literal) => {
            args.get($idx).ok_or_else(|| Diagnostic::CompilerBug {
                where_: "sky_backend_rust::emit_db_call",
                detail: format!("Db kernel {:?} missing arg[{}] ({})", k, $idx, $name),
            })
        };
    }

    // Projection snippets.
    // `project_params(s)` — `List SqlValue` → `Vec<SqlParam>`
    let project_params = |s: &str| {
        format!(
            "({s}).into_iter().map(|__p| __p.into_sql_param())\
             .collect::<Vec<_>>()"
        )
    };
    // `project_fields(s)` — `List (String, SqlField)` → `Vec<(String, Option<SqlParam>)>`
    let project_fields = |s: &str| {
        format!(
            "({s}).into_iter().map(|(__k, __v)| (__k, __v.into_field_param()))\
             .collect::<Vec<_>>()"
        )
    };
    // `project_where(s)` — `List (String, SqlValue)` → `Vec<(String, SqlParam)>`
    let project_where = |s: &str| {
        format!(
            "({s}).into_iter().map(|(__k, __v)| (__k, __v.into_sql_param()))\
             .collect::<Vec<_>>()"
        )
    };

    match k {
        // ── DbExecRaw: (conn, sql) — DDL / no-param statements ──────────────
        //
        // The connection is cloned here (and in every other task-returning Db
        // kernel below) because the emitter wraps sequential Db calls in nested
        // `task_and_then(effect, move |_| { … })` continuations.  Rust evaluates
        // function arguments left-to-right: the EFFECT is built first (arg 0),
        // which would MOVE the `conn` binding, leaving the continuation closure
        // unable to capture it.  Cloning at each call site is the idiomatic fix
        // for `Arc`-backed handles; the `Db` type wraps an `Arc<Pool<…>>` so
        // cloning is cheap (pointer increment only).
        KernelFn::DbExecRaw => {
            let conn_e = arg!(0, "conn")?;
            let sql_e = arg!(1, "sql")?;
            let conn_s = emit_expr_at(ctx, conn_e, indent, child, generics)?;
            let sql_s = emit_expr_at(ctx, sql_e, indent, child, generics)?;
            let fn_name = crate::naming::kernel_name(*k);
            Ok(Some(format!("{fn_name}({conn_s}.clone(), {sql_s})")))
        }
        // ── DbExec / DbQuery: (conn, sql, List SqlValue) ────────────────────
        KernelFn::DbExec | KernelFn::DbQuery => {
            let conn_e = arg!(0, "conn")?;
            let sql_e = arg!(1, "sql")?;
            let params_e = arg!(2, "params")?;
            let conn_s = emit_expr_at(ctx, conn_e, indent, child, generics)?;
            let sql_s = emit_expr_at(ctx, sql_e, indent, child, generics)?;
            let params_s = emit_expr_at(ctx, params_e, indent, child, generics)?;
            let fn_name = crate::naming::kernel_name(*k);
            Ok(Some(format!(
                "{fn_name}({conn_s}.clone(), {sql_s}, {})",
                project_params(&params_s)
            )))
        }
        // ── DbQueryDecode: (conn, sql, List SqlValue, decoder) ──────────────
        KernelFn::DbQueryDecode => {
            let conn_e = arg!(0, "conn")?;
            let sql_e = arg!(1, "sql")?;
            let params_e = arg!(2, "params")?;
            let dec_e = arg!(3, "decoder")?;
            let conn_s = emit_expr_at(ctx, conn_e, indent, child, generics)?;
            let sql_s = emit_expr_at(ctx, sql_e, indent, child, generics)?;
            let params_s = emit_expr_at(ctx, params_e, indent, child, generics)?;
            let dec_s = emit_expr_at(ctx, dec_e, indent, child, generics)?;
            let fn_name = crate::naming::kernel_name(*k);
            Ok(Some(format!(
                "{fn_name}({conn_s}.clone(), {sql_s}, {}, {dec_s})",
                project_params(&params_s)
            )))
        }
        // ── DbInsertFields: (conn, table, List (String, SqlField)) ───────────
        KernelFn::DbInsertFields => {
            let conn_e = arg!(0, "conn")?;
            let table_e = arg!(1, "table")?;
            let fields_e = arg!(2, "fields")?;
            let conn_s = emit_expr_at(ctx, conn_e, indent, child, generics)?;
            let table_s = emit_expr_at(ctx, table_e, indent, child, generics)?;
            let fields_s = emit_expr_at(ctx, fields_e, indent, child, generics)?;
            let fn_name = crate::naming::kernel_name(*k);
            Ok(Some(format!(
                "{fn_name}({conn_s}.clone(), {table_s}, {})",
                project_fields(&fields_s)
            )))
        }
        // ── DbUpdateFields: (conn, table, List (String,SqlValue), List (String,SqlField)) ─
        KernelFn::DbUpdateFields => {
            let conn_e = arg!(0, "conn")?;
            let table_e = arg!(1, "table")?;
            let where_e = arg!(2, "where_cols")?;
            let set_e = arg!(3, "set_fields")?;
            let conn_s = emit_expr_at(ctx, conn_e, indent, child, generics)?;
            let table_s = emit_expr_at(ctx, table_e, indent, child, generics)?;
            let where_s = emit_expr_at(ctx, where_e, indent, child, generics)?;
            let set_s = emit_expr_at(ctx, set_e, indent, child, generics)?;
            let fn_name = crate::naming::kernel_name(*k);
            Ok(Some(format!(
                "{fn_name}({conn_s}.clone(), {table_s}, {}, {})",
                project_where(&where_s),
                project_fields(&set_s)
            )))
        }
        // ── DbInsertFieldsReturning: (conn, table, List (String, SqlField), projection, decoder) ─
        KernelFn::DbInsertFieldsReturning => {
            let conn_e = arg!(0, "conn")?;
            let table_e = arg!(1, "table")?;
            let fields_e = arg!(2, "fields")?;
            let proj_e = arg!(3, "projection")?;
            let dec_e = arg!(4, "decoder")?;
            let conn_s = emit_expr_at(ctx, conn_e, indent, child, generics)?;
            let table_s = emit_expr_at(ctx, table_e, indent, child, generics)?;
            let fields_s = emit_expr_at(ctx, fields_e, indent, child, generics)?;
            let proj_s = emit_expr_at(ctx, proj_e, indent, child, generics)?;
            let dec_s = emit_expr_at(ctx, dec_e, indent, child, generics)?;
            let fn_name = crate::naming::kernel_name(*k);
            Ok(Some(format!(
                "{fn_name}({conn_s}.clone(), {table_s}, {}, {proj_s}, {dec_s})",
                project_fields(&fields_s)
            )))
        }
        // ── DbInsertRow: (conn, table, row: List (String, String)) ─────────────
        // The runtime function takes `row: HashMap<String, String>` while the
        // Sky type is `List (String, String)` (Vec<(String, String)> in Rust).
        // Emit `.into_iter().collect()` to convert at the call site.
        KernelFn::DbInsertRow => {
            let conn_e = arg!(0, "conn")?;
            let table_e = arg!(1, "table")?;
            let row_e = arg!(2, "row")?;
            let conn_s = emit_expr_at(ctx, conn_e, indent, child, generics)?;
            let table_s = emit_expr_at(ctx, table_e, indent, child, generics)?;
            let row_s = emit_expr_at(ctx, row_e, indent, child, generics)?;
            let fn_name = crate::naming::kernel_name(*k);
            Ok(Some(format!(
                "{fn_name}({conn_s}.clone(), {table_s}, ({row_s}).into_iter().collect::<HashMap<String, String>>())"
            )))
        }
        // ── DbUpdateById: (conn, table, id, row: List (String, String)) ────────
        // Same HashMap conversion needed.
        KernelFn::DbUpdateById => {
            let conn_e = arg!(0, "conn")?;
            let table_e = arg!(1, "table")?;
            let id_e = arg!(2, "id")?;
            let row_e = arg!(3, "row")?;
            let conn_s = emit_expr_at(ctx, conn_e, indent, child, generics)?;
            let table_s = emit_expr_at(ctx, table_e, indent, child, generics)?;
            let id_s = emit_expr_at(ctx, id_e, indent, child, generics)?;
            let row_s = emit_expr_at(ctx, row_e, indent, child, generics)?;
            let fn_name = crate::naming::kernel_name(*k);
            Ok(Some(format!(
                "{fn_name}({conn_s}.clone(), {table_s}, {id_s}, ({row_s}).into_iter().collect::<HashMap<String, String>>())"
            )))
        }
        // ── DbWithTransaction: (conn, body: Db -> Task e a) → Task e a ────────
        // Clone ensures the pool handle remains usable for any Db calls that
        // follow the `withTransaction` in the same continuation chain.  The body
        // closure itself receives its own pool copy through the task-local routing
        // (see `db_with_transaction` in the runtime), so the clone never causes an
        // extra SQLite connection.
        KernelFn::DbWithTransaction => {
            let conn_e = arg!(0, "conn")?;
            let body_e = arg!(1, "body")?;
            let conn_s = emit_expr_at(ctx, conn_e, indent, child, generics)?;
            let body_s = emit_expr_at(ctx, body_e, indent, child, generics)?;
            let fn_name = crate::naming::kernel_name(*k);
            Ok(Some(format!("{fn_name}({conn_s}.clone(), {body_s})")))
        }
        // ── DbMigrate: (conn, List (String, String)) → Task e (List String) ──
        // `List (String, String)` lowers to `Vec<(String, String)>` — no
        // conversion needed; the runtime `db_migrate_apply` takes exactly that.
        KernelFn::DbMigrate => {
            let conn_e = arg!(0, "conn")?;
            let migrations_e = arg!(1, "migrations")?;
            let conn_s = emit_expr_at(ctx, conn_e, indent, child, generics)?;
            let migrations_s = emit_expr_at(ctx, migrations_e, indent, child, generics)?;
            let fn_name = crate::naming::kernel_name(*k);
            Ok(Some(format!("{fn_name}({conn_s}.clone(), {migrations_s})")))
        }
        // ── DbGetById: (conn, table, id) ────────────────────────────────────
        // Conn must be cloned so subsequent Db calls in the same continuation
        // chain can still capture it (Pool<Sqlite> is not Copy).
        KernelFn::DbGetById => {
            let conn_e = arg!(0, "conn")?;
            let table_e = arg!(1, "table")?;
            let id_e = arg!(2, "id")?;
            let conn_s = emit_expr_at(ctx, conn_e, indent, child, generics)?;
            let table_s = emit_expr_at(ctx, table_e, indent, child, generics)?;
            let id_s = emit_expr_at(ctx, id_e, indent, child, generics)?;
            let fn_name = crate::naming::kernel_name(*k);
            Ok(Some(format!(
                "{fn_name}({conn_s}.clone(), {table_s}, {id_s})"
            )))
        }
        // ── DbDeleteById: (conn, table, id) ─────────────────────────────────
        KernelFn::DbDeleteById => {
            let conn_e = arg!(0, "conn")?;
            let table_e = arg!(1, "table")?;
            let id_e = arg!(2, "id")?;
            let conn_s = emit_expr_at(ctx, conn_e, indent, child, generics)?;
            let table_s = emit_expr_at(ctx, table_e, indent, child, generics)?;
            let id_s = emit_expr_at(ctx, id_e, indent, child, generics)?;
            let fn_name = crate::naming::kernel_name(*k);
            Ok(Some(format!(
                "{fn_name}({conn_s}.clone(), {table_s}, {id_s})"
            )))
        }
        // ── DbFindOneByField: (conn, table, field, value) ────────────────────
        KernelFn::DbFindOneByField => {
            let conn_e = arg!(0, "conn")?;
            let table_e = arg!(1, "table")?;
            let field_e = arg!(2, "field")?;
            let value_e = arg!(3, "value")?;
            let conn_s = emit_expr_at(ctx, conn_e, indent, child, generics)?;
            let table_s = emit_expr_at(ctx, table_e, indent, child, generics)?;
            let field_s = emit_expr_at(ctx, field_e, indent, child, generics)?;
            let value_s = emit_expr_at(ctx, value_e, indent, child, generics)?;
            let fn_name = crate::naming::kernel_name(*k);
            Ok(Some(format!(
                "{fn_name}({conn_s}.clone(), {table_s}, {field_s}, {value_s})"
            )))
        }
        // ── DbFindManyByField: (conn, table, field, value) ───────────────────
        KernelFn::DbFindManyByField => {
            let conn_e = arg!(0, "conn")?;
            let table_e = arg!(1, "table")?;
            let field_e = arg!(2, "field")?;
            let value_e = arg!(3, "value")?;
            let conn_s = emit_expr_at(ctx, conn_e, indent, child, generics)?;
            let table_s = emit_expr_at(ctx, table_e, indent, child, generics)?;
            let field_s = emit_expr_at(ctx, field_e, indent, child, generics)?;
            let value_s = emit_expr_at(ctx, value_e, indent, child, generics)?;
            let fn_name = crate::naming::kernel_name(*k);
            Ok(Some(format!(
                "{fn_name}({conn_s}.clone(), {table_s}, {field_s}, {value_s})"
            )))
        }
        // ── DbGet*: (field, row) — row is passed by reference so the same row
        // binding can be used in multiple consecutive accessor calls within a
        // single expression (e.g. inside a `list_map_consume` lambda that reads
        // several columns). The runtime functions take `row: &R where R: SkyRow`.
        KernelFn::DbGetString | KernelFn::DbGetInt | KernelFn::DbGetBool | KernelFn::DbGetField => {
            let field_e = arg!(0, "field")?;
            let row_e = arg!(1, "row")?;
            let field_s = emit_expr_at(ctx, field_e, indent, child, generics)?;
            let row_s = emit_expr_at(ctx, row_e, indent, child, generics)?;
            let fn_name = crate::naming::kernel_name(*k);
            Ok(Some(format!("{fn_name}({field_s}, &({row_s}))")))
        }
        // ── DbFindByConditions: (conn, table, conditions: Dict String String) ──
        //
        // The runtime `db_find_by_conditions` takes `HashMap<String, String>` —
        // identical to the IR's `Dict String String` representation — so no
        // conversion is needed beyond passing the value through.
        KernelFn::DbFindByConditions => {
            let conn_e = arg!(0, "conn")?;
            let table_e = arg!(1, "table")?;
            let conditions_e = arg!(2, "conditions")?;
            let conn_s = emit_expr_at(ctx, conn_e, indent, child, generics)?;
            let table_s = emit_expr_at(ctx, table_e, indent, child, generics)?;
            let conditions_s = emit_expr_at(ctx, conditions_e, indent, child, generics)?;
            let fn_name = crate::naming::kernel_name(*k);
            Ok(Some(format!(
                "{fn_name}({conn_s}.clone(), {table_s}, {conditions_s})"
            )))
        }
        // ── DbUnsafeFindWhere: (conn, table, where_clause, args: List String) ──
        //
        // The runtime `db_unsafe_find_where` takes `Vec<String>` for the `args`
        // parameter — the parameterized-binding channel that keeps this raw-SQL
        // path injection-safe.  The Sky `List String` IR type emits as a `Vec<_>`
        // that the runtime accepts directly.
        KernelFn::DbUnsafeFindWhere => {
            let conn_e = arg!(0, "conn")?;
            let table_e = arg!(1, "table")?;
            let where_e = arg!(2, "where_clause")?;
            let args_e = arg!(3, "args")?;
            let conn_s = emit_expr_at(ctx, conn_e, indent, child, generics)?;
            let table_s = emit_expr_at(ctx, table_e, indent, child, generics)?;
            let where_s = emit_expr_at(ctx, where_e, indent, child, generics)?;
            let args_s = emit_expr_at(ctx, args_e, indent, child, generics)?;
            let fn_name = crate::naming::kernel_name(*k);
            Ok(Some(format!(
                "{fn_name}({conn_s}.clone(), {table_s}, {where_s}, {args_s})"
            )))
        }
        // ── Standard-path Db kernels ────────────────────────────────────────────
        //
        // The kernels below route through `emit_call`'s standard path — their
        // argument types emit correctly without special-case projection.  List
        // them explicitly so any future param-taking Db kernel that needs a custom
        // arm is a compile error here, not a silent exit-0-then-cargo-fail.
        KernelFn::DbConnect
        | KernelFn::DbOpen
        | KernelFn::DbClose
        | KernelFn::DbDecString
        | KernelFn::DbDecInt
        | KernelFn::DbDecFloat
        | KernelFn::DbDecBool
        | KernelFn::DbDecNullable
        | KernelFn::DbDecMap
        | KernelFn::DbDecAndThen
        | KernelFn::DbDecSucceed
        | KernelFn::DbDecFail
        | KernelFn::DbDecMap2
        | KernelFn::DbDecMap3
        | KernelFn::DbDecMap4
        | KernelFn::DbDecRequired
        | KernelFn::DbDecOptional => Ok(None),
        // A Db kernel that reached this arm is a compiler bug: either add a
        // custom projection arm above, or add it to the standard-path list.
        // This arm is unreachable for any KernelFn variant listed above, so
        // its only way to fire is a newly-added Db* variant that was not wired
        // into either list — making the miss a compile-time-hard error rather
        // than a silent exit-0-then-cargo-fail.
        _ if k.is_db() => Err(Diagnostic::CompilerBug {
            where_: "sky_backend_rust::emit_db_call",
            detail: format!(
                "unprojected Db kernel {k:?}: add a custom projection arm \
                 or a standard-path entry to emit_db_call"
            ),
        }),
        // Non-Db kernel: let the standard call path handle it.
        _ => Ok(None),
    }
}

/// Handle TEA (`Cmd` / `Sub` / `Time.every`) kernel calls that require custom
/// argument wiring.
///
/// Returns `Some(emitted)` for:
///
/// * **`CmdNone` / `SubNone`** — zero-arg constructors; the runtime functions
///   take no arguments, so we emit `cmd_none()` / `sub_none()` rather than
///   going through the default N-arg path.
///
/// * **`CmdBatch` / `SubBatch`** — `List (Cmd msg) -> Cmd msg`; the list
///   argument is passed directly to the runtime (its `IrType::List` renders as
///   a Rust `Vec`), so we emit `cmd_batch(<list_expr>)` /
///   `sub_batch(<list_expr>)`.  (A previous version of this doc stated that the
///   argument was materialised via `vec_from_sky_list`; that was never the
///   actual code path — the emitted list expression already has `Vec` type.)
///
/// * **`CmdPerform`** — `Task Error a -> (Result Error a -> msg) -> Cmd msg`;
///   the callback must be boxed as a `Box<dyn Fn(SkyResult<A>) -> M + Send + 'static>`.
///   Emits `cmd_perform(<task>, Box::new(<f>))`.
///
/// * **`SubEvery` / `TimeEvery`** — `Int -> msg -> Sub msg`; these pass
///   through the standard N-arg path (no custom boxing needed), returning
///   `Ok(None)` so the standard emitter handles them.
///
/// Returns `Err(CompilerBug)` for any `k.is_tea()` variant that is:
///
/// * **M6-reserved** (`CmdPublish`, `CmdPublishNoEcho`, `SubSubscribeTopic`,
///   `PubSubPublish`, `PubSubPublishNoEcho`) — guard fires if a program
///   somehow reaches one (e.g. if `lower_callee` mis-routes it); not
///   user-reachable from M5c input.
///
/// Returns `Ok(None)` for non-TEA callees so the standard path handles them.
#[allow(clippy::match_same_arms)]
fn emit_tea_call(
    ctx: &EmitCtx,
    callee: &Callee,
    args: &[Expr],
    indent: usize,
    child: u16,
    generics: GenericScope,
) -> DResult<Option<String>> {
    let Callee::Kernel(k) = callee else {
        return Ok(None);
    };
    if !k.is_tea() {
        return Ok(None);
    }

    macro_rules! arg {
        ($idx:expr, $name:literal) => {
            args.get($idx).ok_or_else(|| Diagnostic::CompilerBug {
                where_: "sky_backend_rust::emit_tea_call",
                detail: format!("TEA kernel {:?} missing arg[{}] ({})", k, $idx, $name),
            })
        };
    }

    match k {
        // ── Arity-0: nullary TEA constructors ──────────────────────────────────
        // `Cmd.none : Cmd msg`  →  `cmd_none()`
        KernelFn::CmdNone => Ok(Some("cmd_none()".to_owned())),
        // `Sub.none : Sub msg`  →  `sub_none()`
        KernelFn::SubNone => Ok(Some("sub_none()".to_owned())),
        // ── Arity-1: list-of-cmds / list-of-subs ────────────────────────────────
        // `Cmd.batch : List (Cmd msg) -> Cmd msg`
        KernelFn::CmdBatch => {
            let list_e = arg!(0, "list")?;
            let list_s = emit_expr_at(ctx, list_e, indent, child, generics)?;
            Ok(Some(format!("cmd_batch({list_s})")))
        }
        // `Sub.batch : List (Sub msg) -> Sub msg`
        KernelFn::SubBatch => {
            let list_e = arg!(0, "list")?;
            let list_s = emit_expr_at(ctx, list_e, indent, child, generics)?;
            Ok(Some(format!("sub_batch({list_s})")))
        }
        // ── Arity-2: Cmd.perform (requires boxing the callback) ─────────────────
        // `Cmd.perform : Task Error a -> (Result Error a -> msg) -> Cmd msg`
        // Emits: `cmd_perform(<task>, <f>)`
        // The runtime's `cmd_perform` signature already boxes the callback,
        // so we can pass the emitted closure expression directly.
        KernelFn::CmdPerform => {
            let task_e = arg!(0, "task")?;
            let f_e = arg!(1, "to_msg")?;
            let task_s = emit_expr_at(ctx, task_e, indent, child, generics)?;
            let f_s = emit_expr_at(ctx, f_e, indent, child, generics)?;
            Ok(Some(format!("cmd_perform({task_s}, {f_s})")))
        }
        // ── Arity-2: tick subscriptions — standard path ──────────────────────────
        // `Sub.every : Int -> msg -> Sub msg` and
        // `Time.every : Int -> msg -> Sub msg`
        // Both pass through the default N-arg emitter (no boxing needed).
        KernelFn::SubEvery | KernelFn::TimeEvery => Ok(None),
        // ── M6 reserved: NOT emittable yet ───────────────────────────────────────
        // If a program somehow reaches one of these kernels through lower_callee
        // routing, that is a compiler invariant violation — hard error.
        KernelFn::CmdPublish
        | KernelFn::CmdPublishNoEcho
        | KernelFn::SubSubscribeTopic
        | KernelFn::PubSubPublish
        | KernelFn::PubSubPublishNoEcho => Err(Diagnostic::CompilerBug {
            where_: "sky_backend_rust::emit_tea_call",
            detail: format!(
                "M6-reserved TEA kernel {k:?} reached emit in M5c — \
                 this callee must not be routed by lower_callee yet"
            ),
        }),
        // Any other `k.is_tea()` variant not listed above is a new wired variant
        // that needs an explicit arm.  The `is_tea()` guard at the top of this
        // function means this arm is a hard compile-time-visible gap rather than
        // a silent `Ok(None)` pass-through.
        _ => Err(Diagnostic::CompilerBug {
            where_: "sky_backend_rust::emit_tea_call",
            detail: format!(
                "TEA kernel {k:?} is_tea() but has no emit arm — \
                 add it to emit_tea_call"
            ),
        }),
    }
}

/// Handle a `Sky.Http.Server` / `Middleware` / `RateLimit` kernel call.
///
/// Returns `Ok(None)` for all wired server kernels (they all use the standard
/// N-arg call path — no boxing or special argument transformation needed).
/// Returns a hard [`Diagnostic::CompilerBug`] for any `is_server()` variant
/// not listed here, so a future addition that forgets this function fails at
/// compile time.
fn emit_server_call(
    ctx: &EmitCtx,
    callee: &Callee,
    args: &[Expr],
    indent: usize,
    child: u16,
    generics: GenericScope,
) -> DResult<Option<String>> {
    let Callee::Kernel(k) = callee else {
        return Ok(None);
    };
    if !k.is_server() {
        return Ok(None);
    }
    match k {
        // ── Request accessor kernels that take `ServerRequest` by value ───────
        //
        // `ServerRequest: Clone` (see `runtime/src/sky_runtime/server.rs`).
        // When a handler calls more than one accessor on the same `req` binding,
        // the first call would move `req` and subsequent calls would fail with
        // E0382 "use of moved value". Emitting `req.clone()` for the request
        // argument keeps the binding alive across all reads — identical pattern
        // to `DictGet` (see the DictGet arm below `emit_server_call`).
        //
        // Server.body   : Request -> String   — req is the only arg (index 0)
        // Server.path   : Request -> String   — req is the only arg (index 0)
        // Server.method : Request -> String   — req is the only arg (index 0)
        KernelFn::ServerBody | KernelFn::ServerPath | KernelFn::ServerMethod => {
            let [req_e] = args else {
                return Err(Diagnostic::CompilerBug {
                    where_: "sky_backend_rust::emit_server_call",
                    detail: format!("{k:?} requires exactly 1 argument, got {}", args.len()),
                });
            };
            let fn_name = kernel_name(*k);
            let req_s = emit_expr_at(ctx, req_e, indent, child, generics)?;
            Ok(Some(format!("{fn_name}({req_s}.clone())")))
        }

        // Server.param      : String -> Request -> Maybe String — req is arg 1
        // Server.queryParam : String -> Request -> Maybe String — req is arg 1
        // Server.header     : String -> Request -> Maybe String — req is arg 1
        // Server.getCookie  : String -> Request -> Maybe String — req is arg 1
        KernelFn::ServerParam
        | KernelFn::ServerQueryParam
        | KernelFn::ServerHeader
        | KernelFn::ServerGetCookie => {
            let [name_e, req_e] = args else {
                return Err(Diagnostic::CompilerBug {
                    where_: "sky_backend_rust::emit_server_call",
                    detail: format!("{k:?} requires exactly 2 arguments, got {}", args.len()),
                });
            };
            let fn_name = kernel_name(*k);
            let name_s = emit_expr_at(ctx, name_e, indent, child, generics)?;
            let req_s = emit_expr_at(ctx, req_e, indent, child, generics)?;
            Ok(Some(format!("{fn_name}({name_s}, {req_s}.clone())")))
        }

        // All remaining server kernels use the standard N-arg call path — no
        // special boxing or argument projection is needed.
        KernelFn::ServerGet
        | KernelFn::ServerPost
        | KernelFn::ServerPut
        | KernelFn::ServerDelete
        | KernelFn::ServerAny
        | KernelFn::ServerApi
        | KernelFn::ServerStatic
        | KernelFn::ServerListen
        | KernelFn::ServerText
        | KernelFn::ServerJson
        | KernelFn::ServerHtml
        | KernelFn::ServerWithStatus
        | KernelFn::ServerWithHeader
        | KernelFn::ServerRedirect
        | KernelFn::ServerCookieNew
        | KernelFn::ServerWithCookie
        | KernelFn::MiddlewareWithCors
        | KernelFn::MiddlewareWithLogging
        | KernelFn::MiddlewareWithBasicAuth
        | KernelFn::MiddlewareWithRateLimit
        | KernelFn::RateLimitAllow => Ok(None),
        // Any is_server() variant not listed above is a gap — hard error so
        // the Rust compiler's exhaustiveness check catches it at compile time.
        _ => Err(Diagnostic::CompilerBug {
            where_: "sky_backend_rust::emit_server_call",
            detail: format!(
                "server kernel {k:?} is_server() but has no emit arm — \
                 add it to emit_server_call"
            ),
        }),
    }
}

/// Find a record field by its Sky source name in an IR field list.
///
/// Searches `fields` linearly for the entry whose interned symbol resolves to
/// `name`.  Returns a reference to the field's value expression on success.
///
/// # Errors
///
/// Returns [`Diagnostic::CompilerBug`] when no field with the requested name
/// is present in the list.  Fail-closed — never silently drops a missing
/// required field (MAKE INVALID STATES UNREPRESENTABLE principle).
fn lookup_field<'f>(
    ctx: &EmitCtx,
    fields: &'f [(Symbol, Expr)],
    name: &str,
    where_: &'static str,
) -> DResult<&'f Expr> {
    for (sym, expr) in fields {
        if ctx.resolve_ident(*sym)? == name {
            return Ok(expr);
        }
    }
    Err(Diagnostic::CompilerBug {
        where_,
        detail: format!(
            "required field `{name}` not found in Ui.layoutWith cfg record literal; \
             available fields: [{}]",
            fields
                .iter()
                .filter_map(|(s, _)| ctx.resolve_ident(*s).ok())
                .collect::<Vec<_>>()
                .join(", ")
        ),
    })
}

/// Handle `Std.Ui` / `Std.Html` kernel calls.
///
/// Phase 0 scope:
/// * The SIX render kernels (`UiLayout`, `UiLayoutWith`, `HtmlRender`,
///   `HtmlEscapeText`, `HtmlEscapeAttr`, `HtmlAttrToString`) are **fully
///   wired** here — they emit calls to `sky_runtime::ui::render::*` and
///   `sky_runtime::html::*`.
/// * The app-entry stubs (`LiveApp`, `LiveAppRouted`, `LiveRoute`,
///   `LiveRenderStatic`, `TuiProgram`, `TuiApp`, `WebviewApp`) return a
///   `CompilerBug` error.  Phase 1 will wire their bodies.
///
/// Returns `None` for any kernel that is not a Ui / Live / Tui / Webview
/// variant, letting the standard call path handle it.
#[allow(clippy::too_many_lines)] // declarative UI kernel dispatch — must list every variant explicitly
#[allow(clippy::many_single_char_names)] // r/g/b/a/k are conventional names for colour channels and kernel var
#[inline(never)]
fn emit_ui_call(
    ctx: &EmitCtx,
    callee: &Callee,
    args: &[Expr],
    indent: usize,
    child: u16,
    generics: GenericScope,
) -> DResult<Option<String>> {
    let Callee::Kernel(k) = callee else {
        return Ok(None);
    };
    // Only handle M7 kernels.
    if !k.is_ui() && !k.is_live() && !k.is_tui() && !k.is_webview() {
        return Ok(None);
    }
    match k {
        // ── Std.Ui / Std.Html render kernels (Phase 0 — fully wired) ─────────

        // `Ui.layout : List (Attribute msg) -> Element msg -> Html msg`
        //
        // Emits: `sky_runtime::ui::render::ui_layout(attrs, elem)`
        KernelFn::UiLayout => {
            let [attrs_e, elem_e] = args else {
                return Err(Diagnostic::CompilerBug {
                    where_: "sky_backend_rust::emit_ui_call::UiLayout",
                    detail: format!("Ui.layout requires exactly 2 arguments, got {}", args.len()),
                });
            };
            let attrs_s = emit_expr_at(ctx, attrs_e, indent, child, generics)?;
            let elem_s = emit_expr_at(ctx, elem_e, indent, child, generics)?;
            // Phase-1a: M is now inferred bottom-up from the concrete element /
            // attrs types that the region-type–sourced emit propagates.  No
            // turbofish required; Rust unifies M from the element argument or from
            // the enclosing function's return type annotation — both supply a
            // concrete `Msg` type.  The old `enclosing_ui_msg` mechanism is gone.
            Ok(Some(format!(
                "sky_runtime::ui::render::ui_layout({attrs_s}, {elem_s})"
            )))
        }

        // `Ui.layoutWith : { wrapperAttrs : ..., rootAttrs : ... } -> Element msg -> Html msg`
        //
        // Emits: `sky_runtime::ui::render::ui_layout_with_vecs::<M>(wrapper, root, elem)`
        //
        // DESIGN: the runtime's generic `ui_layout_with<M, C>` stub was the
        // silent-drop path (`_cfg` ignored, falls back to `ui_layout(vec![], …)`).
        // That path is deleted (MAKE INVALID STATES UNREPRESENTABLE).
        //
        // We delegate at the emit site instead: extract `wrapperAttrs` and
        // `rootAttrs` directly from the IR record literal and pass them as
        // `Vec<Attribute<M>>` to `ui_layout_with_vecs`, bypassing the unsynthesised
        // record struct that would trigger SKY-I0001 if materialised (T3 trap).
        //
        // Non-literal cfg (e.g. `let cfg = { … } in Ui.layoutWith cfg elem`) is
        // rejected fail-closed with `CompilerBug`; it is Phase-1 deferred work.
        KernelFn::UiLayoutWith => {
            let [cfg_e, elem_e] = args else {
                return Err(Diagnostic::CompilerBug {
                    where_: "sky_backend_rust::emit_ui_call::UiLayoutWith",
                    detail: format!(
                        "Ui.layoutWith requires exactly 2 arguments, got {}",
                        args.len()
                    ),
                });
            };
            // Extract fields from the IR literal rather than materialising a
            // synthesised Rust struct (which would ICE with SKY-I0001 because
            // no struct for the {wrapperAttrs, rootAttrs} shape is registered).
            let Expr::Record(fields) = cfg_e else {
                return Err(Diagnostic::CompilerBug {
                    where_: "sky_backend_rust::emit_ui_call::UiLayoutWith",
                    detail: "Ui.layoutWith cfg must be an inline record literal \
                             in Phase 0; non-literal cfg is deferred to Phase 1"
                        .into(),
                });
            };
            let wrapper_e = lookup_field(
                ctx,
                fields,
                "wrapperAttrs",
                "sky_backend_rust::emit_ui_call::UiLayoutWith::wrapperAttrs",
            )?;
            let root_e = lookup_field(
                ctx,
                fields,
                "rootAttrs",
                "sky_backend_rust::emit_ui_call::UiLayoutWith::rootAttrs",
            )?;
            let wrapper_s = emit_expr_at(ctx, wrapper_e, indent, child, generics)?;
            let root_s = emit_expr_at(ctx, root_e, indent, child, generics)?;
            let elem_s = emit_expr_at(ctx, elem_e, indent, child, generics)?;
            // Phase-1a: same bottom-up M inference as UiLayout — no turbofish.
            Ok(Some(format!(
                "sky_runtime::ui::render::ui_layout_with_vecs({wrapper_s}, {root_s}, {elem_s})"
            )))
        }

        // `Html.render : Html msg -> String`
        //
        // Emits: `sky_runtime::html::render_html(&html)`
        KernelFn::HtmlRender => {
            let [html_e] = args else {
                return Err(Diagnostic::CompilerBug {
                    where_: "sky_backend_rust::emit_ui_call::HtmlRender",
                    detail: format!(
                        "Html.render requires exactly 1 argument, got {}",
                        args.len()
                    ),
                });
            };
            let html_s = emit_expr_at(ctx, html_e, indent, child, generics)?;
            Ok(Some(format!("sky_runtime::html::render_html(&{html_s})")))
        }

        // `Html.escapeText : String -> String`
        //
        // Emits: `sky_runtime::html::html_escape_text_(s)` (takes owned String).
        KernelFn::HtmlEscapeText => {
            let [s_e] = args else {
                return Err(Diagnostic::CompilerBug {
                    where_: "sky_backend_rust::emit_ui_call::HtmlEscapeText",
                    detail: format!(
                        "Html.escapeText requires exactly 1 argument, got {}",
                        args.len()
                    ),
                });
            };
            let s_s = emit_expr_at(ctx, s_e, indent, child, generics)?;
            Ok(Some(format!("sky_runtime::html::html_escape_text_({s_s})")))
        }

        // `Html.escapeAttr : String -> String`
        //
        // Emits: `sky_runtime::html::html_escape_attr_(s)` (takes owned String).
        KernelFn::HtmlEscapeAttr => {
            let [s_e] = args else {
                return Err(Diagnostic::CompilerBug {
                    where_: "sky_backend_rust::emit_ui_call::HtmlEscapeAttr",
                    detail: format!(
                        "Html.escapeAttr requires exactly 1 argument, got {}",
                        args.len()
                    ),
                });
            };
            let s_s = emit_expr_at(ctx, s_e, indent, child, generics)?;
            Ok(Some(format!("sky_runtime::html::html_escape_attr_({s_s})")))
        }

        // `Html.attrToString : Html.Attribute msg -> String`
        //
        // Emits: `sky_runtime::html::html_attr_to_string_(attr)` (takes owned Attribute<M>).
        KernelFn::HtmlAttrToString => {
            let [attr_e] = args else {
                return Err(Diagnostic::CompilerBug {
                    where_: "sky_backend_rust::emit_ui_call::HtmlAttrToString",
                    detail: format!(
                        "Html.attrToString requires exactly 1 argument, got {}",
                        args.len()
                    ),
                });
            };
            let attr_s = emit_expr_at(ctx, attr_e, indent, child, generics)?;
            Ok(Some(format!(
                "sky_runtime::html::html_attr_to_string_({attr_s})"
            )))
        }

        // ── Std.Ui element builders ───────────────────────────────────────────

        // `Ui.none : Element msg`
        KernelFn::UiNone => Ok(Some("sky_runtime::ui::helpers::ui_none_()".to_owned())),

        // `Ui.text : String -> Element msg`
        KernelFn::UiText => {
            let [s_e] = args else {
                return Err(Diagnostic::CompilerBug {
                    where_: "sky_backend_rust::emit_ui_call::UiText",
                    detail: format!("Ui.text requires 1 argument, got {}", args.len()),
                });
            };
            let s = emit_expr_at(ctx, s_e, indent, child, generics)?;
            Ok(Some(format!("sky_runtime::ui::helpers::ui_text_({s})")))
        }

        // `Ui.html : Html msg -> Element msg`
        KernelFn::UiHtml => {
            let [h_e] = args else {
                return Err(Diagnostic::CompilerBug {
                    where_: "sky_backend_rust::emit_ui_call::UiHtml",
                    detail: format!("Ui.html requires 1 argument, got {}", args.len()),
                });
            };
            let h = emit_expr_at(ctx, h_e, indent, child, generics)?;
            Ok(Some(format!("sky_runtime::ui::helpers::ui_html_({h})")))
        }

        // `Ui.el : List (Attribute msg) -> Element msg -> Element msg`
        KernelFn::UiEl => {
            let [attrs_e, child_e] = args else {
                return Err(Diagnostic::CompilerBug {
                    where_: "sky_backend_rust::emit_ui_call::UiEl",
                    detail: format!("Ui.el requires 2 arguments, got {}", args.len()),
                });
            };
            let attrs = emit_expr_at(ctx, attrs_e, indent, child, generics)?;
            let ch = emit_expr_at(ctx, child_e, indent, child, generics)?;
            Ok(Some(format!(
                "sky_runtime::ui::helpers::ui_el_({attrs}, {ch})"
            )))
        }

        // `Ui.row : List (Attribute msg) -> List (Element msg) -> Element msg`
        KernelFn::UiRow => {
            let [attrs_e, children_e] = args else {
                return Err(Diagnostic::CompilerBug {
                    where_: "sky_backend_rust::emit_ui_call::UiRow",
                    detail: format!("Ui.row requires 2 arguments, got {}", args.len()),
                });
            };
            let attrs = emit_expr_at(ctx, attrs_e, indent, child, generics)?;
            let children = emit_expr_at(ctx, children_e, indent, child, generics)?;
            Ok(Some(format!(
                "sky_runtime::ui::helpers::ui_row_({attrs}, {children})"
            )))
        }

        // `Ui.column : List (Attribute msg) -> List (Element msg) -> Element msg`
        KernelFn::UiColumn => {
            let [attrs_e, children_e] = args else {
                return Err(Diagnostic::CompilerBug {
                    where_: "sky_backend_rust::emit_ui_call::UiColumn",
                    detail: format!("Ui.column requires 2 arguments, got {}", args.len()),
                });
            };
            let attrs = emit_expr_at(ctx, attrs_e, indent, child, generics)?;
            let children = emit_expr_at(ctx, children_e, indent, child, generics)?;
            Ok(Some(format!(
                "sky_runtime::ui::helpers::ui_column_({attrs}, {children})"
            )))
        }

        // `Ui.wrappedRow : List (Attribute msg) -> List (Element msg) -> Element msg`
        KernelFn::UiWrappedRow => {
            let [attrs_e, children_e] = args else {
                return Err(Diagnostic::CompilerBug {
                    where_: "sky_backend_rust::emit_ui_call::UiWrappedRow",
                    detail: format!("Ui.wrappedRow requires 2 arguments, got {}", args.len()),
                });
            };
            let attrs = emit_expr_at(ctx, attrs_e, indent, child, generics)?;
            let children = emit_expr_at(ctx, children_e, indent, child, generics)?;
            Ok(Some(format!(
                "sky_runtime::ui::helpers::ui_wrapped_row_({attrs}, {children})"
            )))
        }

        // `Ui.grid : List (Attribute msg) -> List (Element msg) -> Element msg`
        KernelFn::UiGrid => {
            let [attrs_e, children_e] = args else {
                return Err(Diagnostic::CompilerBug {
                    where_: "sky_backend_rust::emit_ui_call::UiGrid",
                    detail: format!("Ui.grid requires 2 arguments, got {}", args.len()),
                });
            };
            let attrs = emit_expr_at(ctx, attrs_e, indent, child, generics)?;
            let children = emit_expr_at(ctx, children_e, indent, child, generics)?;
            Ok(Some(format!(
                "sky_runtime::ui::helpers::ui_grid_({attrs}, {children})"
            )))
        }

        // ── Std.Ui attribute builders ─────────────────────────────────────────

        // `Ui.spacing : Int -> Attribute msg`
        KernelFn::UiSpacing => {
            let [n_e] = args else {
                return Err(Diagnostic::CompilerBug {
                    where_: "sky_backend_rust::emit_ui_call::UiSpacing",
                    detail: format!("Ui.spacing requires 1 argument, got {}", args.len()),
                });
            };
            let n = emit_expr_at(ctx, n_e, indent, child, generics)?;
            Ok(Some(format!("sky_runtime::ui::helpers::ui_spacing_({n})")))
        }

        // `Ui.padding : Int -> Attribute msg`
        KernelFn::UiPadding => {
            let [n_e] = args else {
                return Err(Diagnostic::CompilerBug {
                    where_: "sky_backend_rust::emit_ui_call::UiPadding",
                    detail: format!("Ui.padding requires 1 argument, got {}", args.len()),
                });
            };
            let n = emit_expr_at(ctx, n_e, indent, child, generics)?;
            Ok(Some(format!("sky_runtime::ui::helpers::ui_padding_({n})")))
        }

        // `Ui.paddingXY : Int -> Int -> Attribute msg`
        KernelFn::UiPaddingXY => {
            let [x_e, y_e] = args else {
                return Err(Diagnostic::CompilerBug {
                    where_: "sky_backend_rust::emit_ui_call::UiPaddingXY",
                    detail: format!("Ui.paddingXY requires 2 arguments, got {}", args.len()),
                });
            };
            let x = emit_expr_at(ctx, x_e, indent, child, generics)?;
            let y = emit_expr_at(ctx, y_e, indent, child, generics)?;
            Ok(Some(format!(
                "sky_runtime::ui::helpers::ui_padding_xy_({x}, {y})"
            )))
        }

        // `Ui.width : Length -> Attribute msg`
        KernelFn::UiWidth => {
            let [l_e] = args else {
                return Err(Diagnostic::CompilerBug {
                    where_: "sky_backend_rust::emit_ui_call::UiWidth",
                    detail: format!("Ui.width requires 1 argument, got {}", args.len()),
                });
            };
            let l = emit_expr_at(ctx, l_e, indent, child, generics)?;
            Ok(Some(format!("sky_runtime::ui::helpers::ui_width_({l})")))
        }

        // `Ui.height : Length -> Attribute msg`
        KernelFn::UiHeight => {
            let [l_e] = args else {
                return Err(Diagnostic::CompilerBug {
                    where_: "sky_backend_rust::emit_ui_call::UiHeight",
                    detail: format!("Ui.height requires 1 argument, got {}", args.len()),
                });
            };
            let l = emit_expr_at(ctx, l_e, indent, child, generics)?;
            Ok(Some(format!("sky_runtime::ui::helpers::ui_height_({l})")))
        }

        // `Ui.centerX : Attribute msg` (arity 0)
        KernelFn::UiCenterX => Ok(Some("sky_runtime::ui::helpers::ui_center_x_()".to_owned())),
        // `Ui.centerY : Attribute msg` (arity 0)
        KernelFn::UiCenterY => Ok(Some("sky_runtime::ui::helpers::ui_center_y_()".to_owned())),
        // `Ui.alignLeft : Attribute msg` (arity 0)
        KernelFn::UiAlignLeft => Ok(Some(
            "sky_runtime::ui::helpers::ui_align_left_()".to_owned(),
        )),
        // `Ui.alignRight : Attribute msg` (arity 0)
        KernelFn::UiAlignRight => Ok(Some(
            "sky_runtime::ui::helpers::ui_align_right_()".to_owned(),
        )),
        // `Ui.alignTop : Attribute msg` (arity 0)
        KernelFn::UiAlignTop => Ok(Some("sky_runtime::ui::helpers::ui_align_top_()".to_owned())),
        // `Ui.alignBottom : Attribute msg` (arity 0)
        KernelFn::UiAlignBottom => Ok(Some(
            "sky_runtime::ui::helpers::ui_align_bottom_()".to_owned(),
        )),
        // `Ui.pointer : Attribute msg` (arity 0)
        KernelFn::UiPointer => Ok(Some("sky_runtime::ui::helpers::ui_pointer_()".to_owned())),
        // `Ui.clip : Attribute msg` (arity 0)
        KernelFn::UiClip => Ok(Some("sky_runtime::ui::helpers::ui_clip_()".to_owned())),
        // `Ui.scrollbars : Attribute msg` (arity 0)
        KernelFn::UiScrollbars => Ok(Some(
            "sky_runtime::ui::helpers::ui_scrollbars_()".to_owned(),
        )),

        // `Ui.gridColumns : Int -> Attribute msg`
        KernelFn::UiGridColumns => {
            let [n_e] = args else {
                return Err(Diagnostic::CompilerBug {
                    where_: "sky_backend_rust::emit_ui_call::UiGridColumns",
                    detail: format!("Ui.gridColumns requires 1 argument, got {}", args.len()),
                });
            };
            let n = emit_expr_at(ctx, n_e, indent, child, generics)?;
            Ok(Some(format!(
                "sky_runtime::ui::helpers::ui_grid_columns_({n})"
            )))
        }

        // ── Std.Ui Length builders ────────────────────────────────────────────

        // `Ui.px : Int -> Length`
        KernelFn::UiPx => {
            let [n_e] = args else {
                return Err(Diagnostic::CompilerBug {
                    where_: "sky_backend_rust::emit_ui_call::UiPx",
                    detail: format!("Ui.px requires 1 argument, got {}", args.len()),
                });
            };
            let n = emit_expr_at(ctx, n_e, indent, child, generics)?;
            Ok(Some(format!("sky_runtime::ui::helpers::ui_px_({n})")))
        }

        // `Ui.fill : Length` (arity 0)
        KernelFn::UiFill => Ok(Some("sky_runtime::ui::helpers::ui_fill_()".to_owned())),
        // `Ui.content : Length` (arity 0)
        KernelFn::UiContent => Ok(Some("sky_runtime::ui::helpers::ui_content_()".to_owned())),
        // `Ui.shrink : Length` (arity 0)
        KernelFn::UiShrink => Ok(Some("sky_runtime::ui::helpers::ui_shrink_()".to_owned())),

        // `Ui.fillPortion : Int -> Length`
        KernelFn::UiFillPortion => {
            let [n_e] = args else {
                return Err(Diagnostic::CompilerBug {
                    where_: "sky_backend_rust::emit_ui_call::UiFillPortion",
                    detail: format!("Ui.fillPortion requires 1 argument, got {}", args.len()),
                });
            };
            let n = emit_expr_at(ctx, n_e, indent, child, generics)?;
            Ok(Some(format!(
                "sky_runtime::ui::helpers::ui_fill_portion_({n})"
            )))
        }

        // `Ui.vh : Int -> Length`
        KernelFn::UiVh => {
            let [n_e] = args else {
                return Err(Diagnostic::CompilerBug {
                    where_: "sky_backend_rust::emit_ui_call::UiVh",
                    detail: format!("Ui.vh requires 1 argument, got {}", args.len()),
                });
            };
            let n = emit_expr_at(ctx, n_e, indent, child, generics)?;
            Ok(Some(format!("sky_runtime::ui::helpers::ui_vh_({n})")))
        }

        // `Ui.vw : Int -> Length`
        KernelFn::UiVw => {
            let [n_e] = args else {
                return Err(Diagnostic::CompilerBug {
                    where_: "sky_backend_rust::emit_ui_call::UiVw",
                    detail: format!("Ui.vw requires 1 argument, got {}", args.len()),
                });
            };
            let n = emit_expr_at(ctx, n_e, indent, child, generics)?;
            Ok(Some(format!("sky_runtime::ui::helpers::ui_vw_({n})")))
        }

        // `Ui.minimum : Int -> Length -> Length`
        KernelFn::UiMinimum => {
            let [n_e, l_e] = args else {
                return Err(Diagnostic::CompilerBug {
                    where_: "sky_backend_rust::emit_ui_call::UiMinimum",
                    detail: format!("Ui.minimum requires 2 arguments, got {}", args.len()),
                });
            };
            let n = emit_expr_at(ctx, n_e, indent, child, generics)?;
            let l = emit_expr_at(ctx, l_e, indent, child, generics)?;
            Ok(Some(format!(
                "sky_runtime::ui::helpers::ui_minimum_({n}, {l})"
            )))
        }

        // `Ui.maximum : Int -> Length -> Length`
        KernelFn::UiMaximum => {
            let [n_e, l_e] = args else {
                return Err(Diagnostic::CompilerBug {
                    where_: "sky_backend_rust::emit_ui_call::UiMaximum",
                    detail: format!("Ui.maximum requires 2 arguments, got {}", args.len()),
                });
            };
            let n = emit_expr_at(ctx, n_e, indent, child, generics)?;
            let l = emit_expr_at(ctx, l_e, indent, child, generics)?;
            Ok(Some(format!(
                "sky_runtime::ui::helpers::ui_maximum_({n}, {l})"
            )))
        }

        // ── Std.Ui Color builders ─────────────────────────────────────────────

        // `Ui.rgb : Int -> Int -> Int -> Color`
        KernelFn::UiRgb => {
            let [r_e, g_e, b_e] = args else {
                return Err(Diagnostic::CompilerBug {
                    where_: "sky_backend_rust::emit_ui_call::UiRgb",
                    detail: format!("Ui.rgb requires 3 arguments, got {}", args.len()),
                });
            };
            let r = emit_expr_at(ctx, r_e, indent, child, generics)?;
            let g = emit_expr_at(ctx, g_e, indent, child, generics)?;
            let b = emit_expr_at(ctx, b_e, indent, child, generics)?;
            Ok(Some(format!(
                "sky_runtime::ui::helpers::ui_rgb_({r}, {g}, {b})"
            )))
        }

        // `Ui.rgba : Int -> Int -> Int -> Float -> Color`
        KernelFn::UiRgba => {
            let [r_e, g_e, b_e, a_e] = args else {
                return Err(Diagnostic::CompilerBug {
                    where_: "sky_backend_rust::emit_ui_call::UiRgba",
                    detail: format!("Ui.rgba requires 4 arguments, got {}", args.len()),
                });
            };
            let r = emit_expr_at(ctx, r_e, indent, child, generics)?;
            let g = emit_expr_at(ctx, g_e, indent, child, generics)?;
            let b = emit_expr_at(ctx, b_e, indent, child, generics)?;
            let a = emit_expr_at(ctx, a_e, indent, child, generics)?;
            Ok(Some(format!(
                "sky_runtime::ui::helpers::ui_rgba_({r}, {g}, {b}, {a})"
            )))
        }

        // `Ui.white : Color` (arity 0)
        KernelFn::UiWhite => Ok(Some("sky_runtime::ui::helpers::ui_white_()".to_owned())),
        // `Ui.black : Color` (arity 0)
        KernelFn::UiBlack => Ok(Some("sky_runtime::ui::helpers::ui_black_()".to_owned())),
        // `Ui.transparent : Color` (arity 0)
        KernelFn::UiTransparent => Ok(Some(
            "sky_runtime::ui::helpers::ui_transparent_()".to_owned(),
        )),

        // ── Background sub-module ─────────────────────────────────────────────

        // `Background.color : Color -> Attribute msg`
        KernelFn::BackgroundColor => {
            let [c_e] = args else {
                return Err(Diagnostic::CompilerBug {
                    where_: "sky_backend_rust::emit_ui_call::BackgroundColor",
                    detail: format!("Background.color requires 1 argument, got {}", args.len()),
                });
            };
            let c = emit_expr_at(ctx, c_e, indent, child, generics)?;
            Ok(Some(format!(
                "sky_runtime::ui::helpers::ui_background_color_({c})"
            )))
        }

        // `Background.image : String -> Attribute msg`
        KernelFn::BackgroundImage => {
            let [s_e] = args else {
                return Err(Diagnostic::CompilerBug {
                    where_: "sky_backend_rust::emit_ui_call::BackgroundImage",
                    detail: format!("Background.image requires 1 argument, got {}", args.len()),
                });
            };
            let s = emit_expr_at(ctx, s_e, indent, child, generics)?;
            Ok(Some(format!(
                "sky_runtime::ui::helpers::ui_background_image_({s})"
            )))
        }

        // ── Border sub-module ─────────────────────────────────────────────────

        // `Border.width : Int -> Attribute msg`
        KernelFn::BorderWidth => {
            let [n_e] = args else {
                return Err(Diagnostic::CompilerBug {
                    where_: "sky_backend_rust::emit_ui_call::BorderWidth",
                    detail: format!("Border.width requires 1 argument, got {}", args.len()),
                });
            };
            let n = emit_expr_at(ctx, n_e, indent, child, generics)?;
            Ok(Some(format!(
                "sky_runtime::ui::helpers::ui_border_width_({n})"
            )))
        }

        // `Border.rounded : Int -> Attribute msg`
        KernelFn::BorderRounded => {
            let [n_e] = args else {
                return Err(Diagnostic::CompilerBug {
                    where_: "sky_backend_rust::emit_ui_call::BorderRounded",
                    detail: format!("Border.rounded requires 1 argument, got {}", args.len()),
                });
            };
            let n = emit_expr_at(ctx, n_e, indent, child, generics)?;
            Ok(Some(format!(
                "sky_runtime::ui::helpers::ui_border_rounded_({n})"
            )))
        }

        // `Border.color : Color -> Attribute msg`
        KernelFn::BorderColor => {
            let [c_e] = args else {
                return Err(Diagnostic::CompilerBug {
                    where_: "sky_backend_rust::emit_ui_call::BorderColor",
                    detail: format!("Border.color requires 1 argument, got {}", args.len()),
                });
            };
            let c = emit_expr_at(ctx, c_e, indent, child, generics)?;
            Ok(Some(format!(
                "sky_runtime::ui::helpers::ui_border_color_({c})"
            )))
        }

        // ── Font sub-module ───────────────────────────────────────────────────

        // `Font.size : Int -> Attribute msg`
        KernelFn::FontSize => {
            let [n_e] = args else {
                return Err(Diagnostic::CompilerBug {
                    where_: "sky_backend_rust::emit_ui_call::FontSize",
                    detail: format!("Font.size requires 1 argument, got {}", args.len()),
                });
            };
            let n = emit_expr_at(ctx, n_e, indent, child, generics)?;
            Ok(Some(format!(
                "sky_runtime::ui::helpers::ui_font_size_({n})"
            )))
        }

        // `Font.color : Color -> Attribute msg`
        KernelFn::FontColor => {
            let [c_e] = args else {
                return Err(Diagnostic::CompilerBug {
                    where_: "sky_backend_rust::emit_ui_call::FontColor",
                    detail: format!("Font.color requires 1 argument, got {}", args.len()),
                });
            };
            let c = emit_expr_at(ctx, c_e, indent, child, generics)?;
            Ok(Some(format!(
                "sky_runtime::ui::helpers::ui_font_color_({c})"
            )))
        }

        // `Font.family : List String -> Attribute msg`
        KernelFn::FontFamily => {
            let [l_e] = args else {
                return Err(Diagnostic::CompilerBug {
                    where_: "sky_backend_rust::emit_ui_call::FontFamily",
                    detail: format!("Font.family requires 1 argument, got {}", args.len()),
                });
            };
            let l = emit_expr_at(ctx, l_e, indent, child, generics)?;
            Ok(Some(format!(
                "sky_runtime::ui::helpers::ui_font_family_({l})"
            )))
        }

        // `Font.bold : Attribute msg` (arity 0)
        KernelFn::FontBold => Ok(Some("sky_runtime::ui::helpers::ui_font_bold_()".to_owned())),
        // `Font.italic : Attribute msg` (arity 0)
        KernelFn::FontItalic => Ok(Some(
            "sky_runtime::ui::helpers::ui_font_italic_()".to_owned(),
        )),

        // ── Std.Html element builders ─────────────────────────────────────────

        // `Html.text : String -> Html msg`
        KernelFn::HtmlTextNode => {
            let [s_e] = args else {
                return Err(Diagnostic::CompilerBug {
                    where_: "sky_backend_rust::emit_ui_call::HtmlTextNode",
                    detail: format!("Html.text requires 1 argument, got {}", args.len()),
                });
            };
            let s = emit_expr_at(ctx, s_e, indent, child, generics)?;
            Ok(Some(format!(
                "sky_runtime::ui::helpers::html_text_node_({s})"
            )))
        }

        // `Html.raw : String -> Html msg`
        KernelFn::HtmlRawNode => {
            let [s_e] = args else {
                return Err(Diagnostic::CompilerBug {
                    where_: "sky_backend_rust::emit_ui_call::HtmlRawNode",
                    detail: format!("Html.raw requires 1 argument, got {}", args.len()),
                });
            };
            let s = emit_expr_at(ctx, s_e, indent, child, generics)?;
            Ok(Some(format!(
                "sky_runtime::ui::helpers::html_raw_node_({s})"
            )))
        }

        // `Html.node : String -> List Attr -> List Html -> Html msg`
        KernelFn::HtmlNode => {
            let [tag_e, attrs_e, children_e] = args else {
                return Err(Diagnostic::CompilerBug {
                    where_: "sky_backend_rust::emit_ui_call::HtmlNode",
                    detail: format!("Html.node requires 3 arguments, got {}", args.len()),
                });
            };
            let tag = emit_expr_at(ctx, tag_e, indent, child, generics)?;
            let attrs = emit_expr_at(ctx, attrs_e, indent, child, generics)?;
            let children = emit_expr_at(ctx, children_e, indent, child, generics)?;
            Ok(Some(format!(
                "sky_runtime::ui::helpers::html_node_({tag}, {attrs}, {children})"
            )))
        }

        // `Html.styleNode : List Attr -> String -> Html msg` (arity-2; the
        // dedicated kernel close-tag-neutralises the CSS body — F7).
        KernelFn::HtmlStyleNode => {
            let [attrs_e, css_e] = args else {
                return Err(Diagnostic::CompilerBug {
                    where_: "sky_backend_rust::emit_ui_call::HtmlStyleNode",
                    detail: format!("Html.styleNode requires 2 arguments, got {}", args.len()),
                });
            };
            let attrs = emit_expr_at(ctx, attrs_e, indent, child, generics)?;
            let css = emit_expr_at(ctx, css_e, indent, child, generics)?;
            Ok(Some(format!(
                "sky_runtime::ui::helpers::html_style_node_({attrs}, {css})"
            )))
        }

        // `Html.div : List Attr -> List Html -> Html msg`
        KernelFn::HtmlDiv => {
            let [attrs_e, children_e] = args else {
                return Err(Diagnostic::CompilerBug {
                    where_: "sky_backend_rust::emit_ui_call::HtmlDiv",
                    detail: format!("Html.div requires 2 arguments, got {}", args.len()),
                });
            };
            let attrs = emit_expr_at(ctx, attrs_e, indent, child, generics)?;
            let children = emit_expr_at(ctx, children_e, indent, child, generics)?;
            Ok(Some(format!(
                "sky_runtime::ui::helpers::html_div_({attrs}, {children})"
            )))
        }

        // `Html.span : List Attr -> List Html -> Html msg`
        KernelFn::HtmlSpan => {
            let [attrs_e, children_e] = args else {
                return Err(Diagnostic::CompilerBug {
                    where_: "sky_backend_rust::emit_ui_call::HtmlSpan",
                    detail: format!("Html.span requires 2 arguments, got {}", args.len()),
                });
            };
            let attrs = emit_expr_at(ctx, attrs_e, indent, child, generics)?;
            let children = emit_expr_at(ctx, children_e, indent, child, generics)?;
            Ok(Some(format!(
                "sky_runtime::ui::helpers::html_span_({attrs}, {children})"
            )))
        }

        // `Html.a : List Attr -> List Html -> Html msg`
        KernelFn::HtmlA => {
            let [attrs_e, children_e] = args else {
                return Err(Diagnostic::CompilerBug {
                    where_: "sky_backend_rust::emit_ui_call::HtmlA",
                    detail: format!("Html.a requires 2 arguments, got {}", args.len()),
                });
            };
            let attrs = emit_expr_at(ctx, attrs_e, indent, child, generics)?;
            let children = emit_expr_at(ctx, children_e, indent, child, generics)?;
            Ok(Some(format!(
                "sky_runtime::ui::helpers::html_a_({attrs}, {children})"
            )))
        }

        // `Html.button : List Attr -> List Html -> Html msg`
        KernelFn::HtmlButton => {
            let [attrs_e, children_e] = args else {
                return Err(Diagnostic::CompilerBug {
                    where_: "sky_backend_rust::emit_ui_call::HtmlButton",
                    detail: format!("Html.button requires 2 arguments, got {}", args.len()),
                });
            };
            let attrs = emit_expr_at(ctx, attrs_e, indent, child, generics)?;
            let children = emit_expr_at(ctx, children_e, indent, child, generics)?;
            Ok(Some(format!(
                "sky_runtime::ui::helpers::html_button_({attrs}, {children})"
            )))
        }

        // `Html.p (and other block elements) : List Attr -> List Html -> Html msg`
        KernelFn::HtmlP => {
            let [attrs_e, children_e] = args else {
                return Err(Diagnostic::CompilerBug {
                    where_: "sky_backend_rust::emit_ui_call::HtmlP",
                    detail: format!("Html.p/block requires 2 arguments, got {}", args.len()),
                });
            };
            let attrs = emit_expr_at(ctx, attrs_e, indent, child, generics)?;
            let children = emit_expr_at(ctx, children_e, indent, child, generics)?;
            Ok(Some(format!(
                "sky_runtime::ui::helpers::html_p_({attrs}, {children})"
            )))
        }

        // `Html.input : List Attr -> Html msg` (void element)
        KernelFn::HtmlInput => {
            let [attrs_e] = args else {
                return Err(Diagnostic::CompilerBug {
                    where_: "sky_backend_rust::emit_ui_call::HtmlInput",
                    detail: format!("Html.input requires 1 argument, got {}", args.len()),
                });
            };
            let attrs = emit_expr_at(ctx, attrs_e, indent, child, generics)?;
            Ok(Some(format!(
                "sky_runtime::ui::helpers::html_input_({attrs})"
            )))
        }

        // `Html.img : List Attr -> Html msg` (void element)
        KernelFn::HtmlImg => {
            let [attrs_e] = args else {
                return Err(Diagnostic::CompilerBug {
                    where_: "sky_backend_rust::emit_ui_call::HtmlImg",
                    detail: format!("Html.img requires 1 argument, got {}", args.len()),
                });
            };
            let attrs = emit_expr_at(ctx, attrs_e, indent, child, generics)?;
            Ok(Some(format!(
                "sky_runtime::ui::helpers::html_img_({attrs})"
            )))
        }

        // ── Phase-1a: Event-attribute builders ───────────────────────────────────
        //
        // Plain-message events (onClick/onFocus/onBlur/onMouseOver/onMouseOut):
        //   Ui.onClick : msg -> Attribute msg
        //   emit: sky_runtime::ui::helpers::ui_on_click_(msg_expr)
        //
        // String-carrying events (onInput/onChange/onKeyDown/onKeyUp) — T6 trap:
        //   The Sky fn arg is an emitted Rust fn-value (closure or fn-ptr).
        //   The runtime requires `Arc<dyn Fn(String)->M+Send+Sync>`.
        //   We emit: ui_on_input_(std::sync::Arc::new(move |_x| (f)(_x)))
        //   This is sound: the Arc captures `f` by move; `f` is always 'static
        //   since emitted Sky fns carry no borrow-lifetime context.

        // `Ui.onClick / Event.onClick : msg -> Attribute msg`
        KernelFn::UiOnClick => {
            let [msg_e] = args else {
                return Err(Diagnostic::CompilerBug {
                    where_: "sky_backend_rust::emit_ui_call::UiOnClick",
                    detail: format!("Ui.onClick requires 1 argument, got {}", args.len()),
                });
            };
            let msg_s = emit_expr_at(ctx, msg_e, indent, child, generics)?;
            Ok(Some(format!(
                "sky_runtime::ui::helpers::ui_on_click_({msg_s})"
            )))
        }

        // `Ui.onFocus : msg -> Attribute msg`
        KernelFn::UiOnFocus => {
            let [msg_e] = args else {
                return Err(Diagnostic::CompilerBug {
                    where_: "sky_backend_rust::emit_ui_call::UiOnFocus",
                    detail: format!("Ui.onFocus requires 1 argument, got {}", args.len()),
                });
            };
            let msg_s = emit_expr_at(ctx, msg_e, indent, child, generics)?;
            Ok(Some(format!(
                "sky_runtime::ui::helpers::ui_on_focus_({msg_s})"
            )))
        }

        // `Ui.onBlur : msg -> Attribute msg`
        KernelFn::UiOnBlur => {
            let [msg_e] = args else {
                return Err(Diagnostic::CompilerBug {
                    where_: "sky_backend_rust::emit_ui_call::UiOnBlur",
                    detail: format!("Ui.onBlur requires 1 argument, got {}", args.len()),
                });
            };
            let msg_s = emit_expr_at(ctx, msg_e, indent, child, generics)?;
            Ok(Some(format!(
                "sky_runtime::ui::helpers::ui_on_blur_({msg_s})"
            )))
        }

        // `Ui.onMouseOver : msg -> Attribute msg`
        KernelFn::UiOnMouseOver => {
            let [msg_e] = args else {
                return Err(Diagnostic::CompilerBug {
                    where_: "sky_backend_rust::emit_ui_call::UiOnMouseOver",
                    detail: format!("Ui.onMouseOver requires 1 argument, got {}", args.len()),
                });
            };
            let msg_s = emit_expr_at(ctx, msg_e, indent, child, generics)?;
            Ok(Some(format!(
                "sky_runtime::ui::helpers::ui_on_mouse_over_({msg_s})"
            )))
        }

        // `Ui.onMouseOut : msg -> Attribute msg`
        KernelFn::UiOnMouseOut => {
            let [msg_e] = args else {
                return Err(Diagnostic::CompilerBug {
                    where_: "sky_backend_rust::emit_ui_call::UiOnMouseOut",
                    detail: format!("Ui.onMouseOut requires 1 argument, got {}", args.len()),
                });
            };
            let msg_s = emit_expr_at(ctx, msg_e, indent, child, generics)?;
            Ok(Some(format!(
                "sky_runtime::ui::helpers::ui_on_mouse_out_({msg_s})"
            )))
        }

        // `Ui.onInput : (String -> msg) -> Attribute msg`  (T6: Arc-wrap the fn)
        KernelFn::UiOnInput => {
            let [f_e] = args else {
                return Err(Diagnostic::CompilerBug {
                    where_: "sky_backend_rust::emit_ui_call::UiOnInput",
                    detail: format!("Ui.onInput requires 1 argument, got {}", args.len()),
                });
            };
            let f_s = emit_expr_at(ctx, f_e, indent, child, generics)?;
            // Arc-wrap: the runtime needs Arc<dyn Fn(String)->M+Send+Sync>.
            // `f` is a 'static emitted Sky fn; `move` captures it by value.
            Ok(Some(format!(
                "sky_runtime::ui::helpers::ui_on_input_(::std::sync::Arc::new(move |_x| ({f_s})(_x)))"
            )))
        }

        // `Ui.onChange : (String -> msg) -> Attribute msg`  (T6: Arc-wrap)
        KernelFn::UiOnChange => {
            let [f_e] = args else {
                return Err(Diagnostic::CompilerBug {
                    where_: "sky_backend_rust::emit_ui_call::UiOnChange",
                    detail: format!("Ui.onChange requires 1 argument, got {}", args.len()),
                });
            };
            let f_s = emit_expr_at(ctx, f_e, indent, child, generics)?;
            Ok(Some(format!(
                "sky_runtime::ui::helpers::ui_on_change_(::std::sync::Arc::new(move |_x| ({f_s})(_x)))"
            )))
        }

        // `Ui.onKeyDown : (String -> msg) -> Attribute msg`  (T6: Arc-wrap)
        KernelFn::UiOnKeyDown => {
            let [f_e] = args else {
                return Err(Diagnostic::CompilerBug {
                    where_: "sky_backend_rust::emit_ui_call::UiOnKeyDown",
                    detail: format!("Ui.onKeyDown requires 1 argument, got {}", args.len()),
                });
            };
            let f_s = emit_expr_at(ctx, f_e, indent, child, generics)?;
            Ok(Some(format!(
                "sky_runtime::ui::helpers::ui_on_key_down_(::std::sync::Arc::new(move |_x| ({f_s})(_x)))"
            )))
        }

        // `Ui.onKeyUp : (String -> msg) -> Attribute msg`  (T6: Arc-wrap)
        KernelFn::UiOnKeyUp => {
            let [f_e] = args else {
                return Err(Diagnostic::CompilerBug {
                    where_: "sky_backend_rust::emit_ui_call::UiOnKeyUp",
                    detail: format!("Ui.onKeyUp requires 1 argument, got {}", args.len()),
                });
            };
            let f_s = emit_expr_at(ctx, f_e, indent, child, generics)?;
            Ok(Some(format!(
                "sky_runtime::ui::helpers::ui_on_key_up_(::std::sync::Arc::new(move |_x| ({f_s})(_x)))"
            )))
        }

        // `Event.onBool : (Bool -> msg) -> Attribute msg`  (T6: Arc-wrap, bool arg)
        KernelFn::UiOnBool => {
            let [f_e] = args else {
                return Err(Diagnostic::CompilerBug {
                    where_: "sky_backend_rust::emit_ui_call::UiOnBool",
                    detail: format!("Event.onBool requires 1 argument, got {}", args.len()),
                });
            };
            let f_s = emit_expr_at(ctx, f_e, indent, child, generics)?;
            Ok(Some(format!(
                "sky_runtime::ui::helpers::ui_on_bool_(::std::sync::Arc::new(move |_x| ({f_s})(_x)))"
            )))
        }

        // ── Live app-entry kernels (Phase 1b — fully wired) ───────────────────
        // Delegate to `emit_live::emit_live_call`; it returns `Some(s)` for the
        // four Live variants and `None` for anything else (the `_ => None` arm).
        // A `None` here is an internal error (the `is_live()` guard above already
        // filtered to Live variants), so promote it to a `CompilerBug`.
        KernelFn::LiveApp
        | KernelFn::LiveAppRouted
        | KernelFn::LiveRoute
        | KernelFn::LiveRenderStatic => {
            let s = crate::emit_live::emit_live_call(ctx, callee, args, indent, child, generics)?
                .ok_or_else(|| Diagnostic::CompilerBug {
                where_: "sky_backend_rust::emit_ui_call",
                detail: format!("emit_live returned None for Live kernel {k:?} — missing arm"),
            })?;
            Ok(Some(s))
        }

        // ── Tui app-entry kernels (Phase-1c — fully wired) ───────────────────
        // Delegate to `emit_tui::emit_tui_call`; it returns `Some(s)` for the
        // two Tui variants and `None` for anything else.  A `None` here is an
        // internal error (the `k.is_tui()` guard already filtered), so promote
        // it to a `CompilerBug`.
        KernelFn::TuiProgram | KernelFn::TuiApp => {
            let s = crate::emit_tui::emit_tui_call(ctx, callee, args, indent, child, generics)?
                .ok_or_else(|| Diagnostic::CompilerBug {
                    where_: "sky_backend_rust::emit_ui_call",
                    detail: format!("emit_tui returned None for Tui kernel {k:?} — missing arm"),
                })?;
            Ok(Some(s))
        }

        // ── Webview app-entry kernel (Phase-1d — fully wired) ─────────────────
        // Delegate to `emit_webview::emit_webview_call`; it returns `Some(s)` for
        // the WebviewApp variant and `None` for anything else. A `None` here is an
        // internal error (the `k.is_webview()` guard above already filtered), so
        // promote it to a `CompilerBug`.
        KernelFn::WebviewApp => {
            let s =
                crate::emit_webview::emit_webview_call(ctx, callee, args, indent, child, generics)?
                    .ok_or_else(|| Diagnostic::CompilerBug {
                        where_: "sky_backend_rust::emit_ui_call",
                        detail: format!(
                            "emit_webview returned None for Webview kernel {k:?} — missing arm"
                        ),
                    })?;
            Ok(Some(s))
        }

        // Any is_ui/live/tui/webview() variant not listed is a gap — hard error.
        _ => Err(Diagnostic::CompilerBug {
            where_: "sky_backend_rust::emit_ui_call",
            detail: format!(
                "UI/Live/Tui/Webview kernel {k:?} has no emit arm — add it to emit_ui_call"
            ),
        }),
    }
}

/// Emit an expression. `indent` is the indentation level (in 4-space units) of
/// the line the expression *starts* on; it is consumed only by the multi-line
/// `match` form. All other M0 expressions render inline (no leading whitespace,
/// no embedded newlines), so the caller positions them.
///
/// A binary operation is always parenthesised (`(count + 1)`) — matching the
/// golden — so precedence is explicit and never relies on Rust's binding rules.
///
/// The bounded entry point: emission starts at depth 0 and fails fast past
/// [`MAX_EMIT_DEPTH`] (SKY-L0200) rather than overflowing the native stack.
pub fn emit_expr(
    ctx: &EmitCtx,
    expr: &Expr,
    indent: usize,
    generics: GenericScope,
) -> DResult<String> {
    emit_expr_at(ctx, expr, indent, 0, generics)
}

/// Handle JSON decoder kernel calls that require custom argument wrapping.
///
/// Returns `Some(emitted)` for the three special cases:
///
/// * **Arity-0 primitive decoders** (`JsonDecString/Int/Float/Bool`) — these
///   carry a free `E: From<String>` type parameter that Rust cannot infer when
///   passed to another polymorphic function (e.g. `decode_from_json_string`).
///   Emits with an explicit `SkyError` turbofish.
///
/// * **`JsonDecSucceed` applied to a named multi-arg function** — `decode_succeed`
///   expects a `Box<dyn Fn() -> A + Send>` FACTORY. A named N-arg function
///   `makeUser` is wrapped as `decode_succeed(curry_N(makeUser))`.
///
/// * **`JsonDecList`** — `decode_list` expects `impl Fn() -> Decoder<E, T> + Send`
///   (a factory) rather than the decoder value. Wraps the argument in a
///   `move` closure: `decode_list(move || { inner })`.
///
/// Returns `None` for all other `Expr::Call` shapes, which fall through to the
/// standard emitter.  Factored out of `emit_expr_at` to avoid inflating that
/// function's stack frame (the depth-guard test relies on a bounded frame size).
#[inline(never)]
fn emit_json_decoder_call(
    ctx: &EmitCtx,
    callee: &Callee,
    args: &[Expr],
    indent: usize,
    child: u16,
    generics: GenericScope,
) -> DResult<Option<String>> {
    // ── Arity-0 primitives — turbofish SkyError ──────────────────────────────
    if args.is_empty()
        && matches!(
            callee,
            Callee::Kernel(
                sky_ir::KernelFn::JsonDecString
                    | sky_ir::KernelFn::JsonDecInt
                    | sky_ir::KernelFn::JsonDecFloat
                    | sky_ir::KernelFn::JsonDecBool
            )
        )
    {
        let name = callee_name(ctx, callee)?;
        return Ok(Some(format!("{name}::<SkyError>()")));
    }
    // ── JsonDecSucceed with named function argument ───────────────────────────
    if matches!(callee, Callee::Kernel(sky_ir::KernelFn::JsonDecSucceed))
        && let Some(Expr::FuncValue {
            callee: fn_callee,
            ty: IrType::Fun(params, _),
        }) = args.first()
        && !params.is_empty()
    {
        let fn_name = callee_name(ctx, fn_callee)?;
        let n = params.len();
        return Ok(Some(format!("decode_succeed(curry{n}({fn_name}))")));
    }
    // ── JsonDecList — wrap argument in factory closure ────────────────────────
    if matches!(callee, Callee::Kernel(sky_ir::KernelFn::JsonDecList))
        && let Some(inner) = args.first()
    {
        let inner_s = emit_expr_at(ctx, inner, indent, child, generics)?;
        return Ok(Some(format!("decode_list(move || {{ {inner_s} }})")));
    }
    Ok(None)
}

/// Depth-tracked recursion behind [`emit_expr`]. `depth` is the IR-nesting level
/// of `expr` (0 at the function body); it gates the bounded-emit guard and is
/// independent of `indent` (the textual indentation of `match` arms).
///
/// `pub(crate)` so that `emit_live` can call it directly (Live kernel bodies
/// emit sub-expressions at the same depth level as their enclosing expression).
#[allow(clippy::too_many_lines)]
pub fn emit_expr_at(
    ctx: &EmitCtx,
    expr: &Expr,
    indent: usize,
    depth: u16,
    generics: GenericScope,
) -> DResult<String> {
    if depth > MAX_EMIT_DEPTH {
        return Err(Diagnostic::Lower {
            span: Span::DUMMY,
            msg: LowerError::BackendNestingTooDeep {
                limit: MAX_EMIT_DEPTH,
            },
        });
    }
    let child = depth + 1;
    match expr {
        Expr::Int(n) => Ok(n.to_string()),
        // A float literal renders as an f64-typed Rust literal. A whole-number
        // value keeps its decimal point (`3.0`) so Rust never types it as an
        // integer; see [`float_literal`].
        Expr::Float(f) => Ok(float_literal(*f)),
        // A string literal renders as an owned `String` (Sky `String` is Rust
        // `String`, never `&str`). The `{:?}` Debug form produces a valid Rust
        // string literal with deterministic escaping.
        Expr::Str(s) => Ok(format!("{s:?}.to_string()")),
        // A character literal renders as a Rust `char`. The carried text is a
        // single character (lexer invariant); `{:?}` escapes it deterministically.
        // A malformed (non-single-char) value falls back to a string literal
        // rather than emitting invalid Rust, staying total.
        Expr::Char(c) => {
            let mut chars = c.chars();
            Ok(match (chars.next(), chars.next()) {
                (Some(ch), None) => format!("{ch:?}"),
                _ => format!("{c:?}"),
            })
        }
        // A boolean value renders as the Rust keyword constant.
        Expr::Bool(b) => Ok(if *b { "true" } else { "false" }.to_owned()),
        // The unit value renders as the Rust unit expression `()`.
        Expr::Unit => Ok("()".to_owned()),
        Expr::Var(sym) => ctx.emit_ident(*sym),
        Expr::Ctor { ty, variant, args } => {
            emit_ctor(ctx, *ty, *variant, args, indent, depth, generics)
        }
        Expr::BinOp { op, lhs, rhs } => {
            let l = emit_expr_at(ctx, lhs, indent, child, generics)?;
            let r = emit_expr_at(ctx, rhs, indent, child, generics)?;
            // Exhaustive match — no wildcard. Adding a new `BinOp` variant
            // without wiring it here is a compile error, not a silent gap.
            match op {
                // `++` (string append) has no Rust infix form for two owned
                // `String`s; `format!` borrows both via `Display` and yields a
                // fresh `String` — no ownership or clone obligation.
                BinOp::Append => Ok(format!("format!(\"{{}}{{}}\", {l}, {r})")),
                // `//` (integer division). Raw Rust `/` on `i64` panics on
                // `b == 0` AND on `i64::MIN / -1`; `//` is itself a Rust line
                // comment, so raw infix emit is doubly unsound. Route through
                // the total helper that matches Sky-Go `rt.IntDiv` semantics:
                // b==0 → panic("attempt to divide by zero") (abort, exit 101);
                // i64::MIN / -1 → i64::MIN (wrapping, no abort).
                BinOp::IntDiv => Ok(format!("sky_runtime::math::sky_int_div({l}, {r})")),
                // Every remaining operator has a sound Rust infix form.
                BinOp::Add
                | BinOp::Sub
                | BinOp::Mul
                | BinOp::Div
                | BinOp::Eq
                | BinOp::Neq
                | BinOp::Lt
                | BinOp::Gt
                | BinOp::Le
                | BinOp::Ge
                | BinOp::And
                | BinOp::Or => Ok(format!("({} {} {})", l, op_str(*op), r)),
            }
        }
        Expr::Let { name, value, body } => {
            // A `let` expression renders as a parenthesised Rust block so it
            // composes inline anywhere an expression is expected:
            // `({ let <name> = <value>; <body> })`.
            let name = ctx.emit_ident(*name)?;
            let value = emit_expr_at(ctx, value, indent, child, generics)?;
            let body = emit_expr_at(ctx, body, indent, child, generics)?;
            Ok(format!("({{ let {name} = {value}; {body} }})"))
        }
        Expr::Destructure {
            binder,
            value,
            body,
        } => {
            // An irrefutable destructuring binding renders as a parenthesised
            // Rust block, exactly like `Let`, but with a pattern binder:
            // `({ let <binder> = <value>; <body> })`. The binder is irrefutable
            // (the lowerer guarantees it — a tuple of var / wildcard binders, or
            // a bare var / wildcard), so the `let` is exhaustive Rust.
            let binder = render_pat(ctx, binder)?;
            let value = emit_expr_at(ctx, value, indent, child, generics)?;
            let body = emit_expr_at(ctx, body, indent, child, generics)?;
            Ok(format!("({{ let {binder} = {value}; {body} }})"))
        }
        Expr::If { cond, then_, else_ } => {
            // Parenthesised so the whole `if`/`else` is a single expression
            // value, independent of surrounding precedence.
            let cond = emit_expr_at(ctx, cond, indent, child, generics)?;
            let then_ = emit_expr_at(ctx, then_, indent, child, generics)?;
            let else_ = emit_expr_at(ctx, else_, indent, child, generics)?;
            Ok(format!("(if {cond} {{ {then_} }} else {{ {else_} }})"))
        }
        Expr::Call { callee, args } => {
            // JSON decoder kernel special cases are factored into a separate
            // `#[inline(never)]` helper to keep the `emit_expr_at` stack frame
            // small enough for the depth-guard test (SKY-L0200). The helper
            // returns `None` when no special case applies.
            if let Some(result) =
                emit_json_decoder_call(ctx, callee, args, indent, child, generics)?
            {
                return Ok(result);
            }
            // Http network kernel special cases: Http.get / Http.post /
            // Http.request need a task_map conversion closure (Design B).
            // Http.parseQuery falls through (standard path is correct).
            if let Some(result) = emit_http_call(ctx, callee, args, indent, child, generics)? {
                return Ok(result);
            }
            // Http builder kernels: Http.defaultRequest / Http.withMethod /
            // Http.withTimeout / Http.withBody / Http.withHeader emit inline
            // struct construction or clone-and-reassign record updates.
            if let Some(result) =
                emit_http_builder_call(ctx, callee, args, indent, child, generics)?
            {
                return Ok(result);
            }
            // Db projection kernels: DbExec / DbQuery / DbQueryDecode /
            // DbInsertFields / DbUpdateFields / DbInsertFieldsReturning need
            // `List SqlValue` / `List (String, SqlField)` projected to
            // `Vec<SqlParam>` / `Vec<(String, Option<SqlParam>)>` at the call
            // site via the generated `into_sql_param` / `into_field_param` methods.
            if let Some(result) = emit_db_call(ctx, callee, args, indent, child, generics)? {
                return Ok(result);
            }
            if let Some(result) = emit_tea_call(ctx, callee, args, indent, child, generics)? {
                return Ok(result);
            }
            if let Some(result) = emit_server_call(ctx, callee, args, indent, child, generics)? {
                return Ok(result);
            }
            // M7: Std.Ui / Std.Html / Std.Live / Std.Tui / Std.Webview kernels.
            if let Some(result) = emit_ui_call(ctx, callee, args, indent, child, generics)? {
                return Ok(result);
            }
            // Dict.get borrows semantics: the runtime takes the HashMap by
            // value, but Sky dicts are persistent — the same dict binding may
            // be passed to multiple Dict.get calls in one let-chain (e.g.
            // `let a = Dict.get "a" d; let b = Dict.get "b" d`).  Cloning the
            // dict arg before each call keeps the original binding alive and
            // avoids the "use of moved value" Rust compile error.
            if matches!(callee, Callee::Kernel(KernelFn::DictGet))
                && let [key_arg, dict_arg] = args.as_slice()
            {
                let key_s = emit_expr_at(ctx, key_arg, indent, child, generics)?;
                let dict_s = emit_expr_at(ctx, dict_arg, indent, child, generics)?;
                return Ok(format!("dict_get({key_s}, {dict_s}.clone())"));
            }
            let name = callee_name(ctx, callee)?;
            let mut parts = Vec::with_capacity(args.len());
            for arg in args {
                parts.push(emit_expr_at(ctx, arg, indent, child, generics)?);
            }
            // A handful of Maybe/Result kernels take the container BEFORE the
            // function in the runtime (`sky_maybe_map(m, f)`) whereas Sky passes
            // the function first (`Maybe.map f m`). The lowerer keeps the Sky
            // order; re-point the two arguments here so the runtime call is
            // well-formed.
            if matches!(callee, Callee::Kernel(k) if kernel_swaps_first_two(*k)) {
                parts.reverse();
            }
            Ok(format!("{name}({})", parts.join(", ")))
        }
        Expr::Tuple(elems) => {
            // A tuple constructor renders inline as `(e1, e2, ...)`. The IR
            // invariant guarantees arity ≥ 2, so this is always a genuine Rust
            // tuple; the emission stays total over any vector regardless.
            let mut parts = Vec::with_capacity(elems.len());
            for elem in elems {
                parts.push(emit_expr_at(ctx, elem, indent, child, generics)?);
            }
            Ok(format!("({})", parts.join(", ")))
        }
        Expr::List { elem, items } => emit_list(ctx, elem, items, indent, depth, generics),
        Expr::Cons { head, tail } => {
            // `head :: tail` renders through the runtime's move-only list prepend.
            let h = emit_expr_at(ctx, head, indent, child, generics)?;
            let t = emit_expr_at(ctx, tail, indent, child, generics)?;
            Ok(format!("sky_runtime::list::sky_list_cons({h}, {t})"))
        }
        // The record arms own several `Vec`/`String` locals; keeping their
        // bodies in dedicated functions (not inlined into this match) holds
        // `emit_expr_at`'s own stack frame small, so the depth guard — not a
        // native overflow — is what bounds a deep `BinOp`/`Call` spine.
        Expr::Record(fields) => emit_record(ctx, fields, indent, depth, generics),
        Expr::Access { record, field } => {
            // Field access `<record>.<field>`. The base is parenthesised so a
            // record literal in record position (`{ ... }.field`) is never
            // misparsed; the field ident is keyword-mangled to match the struct.
            let base = emit_expr_at(ctx, record, indent, child, generics)?;
            let field = ctx.emit_ident(*field)?;
            Ok(format!("({base}).{field}"))
        }
        Expr::Update { record, fields } => {
            emit_update(ctx, record, fields, indent, depth, generics)
        }
        Expr::Lambda { params, ret, body } => {
            emit_lambda(ctx, params, ret, body, indent, depth, generics)
        }
        Expr::Apply { func, args } => emit_apply(ctx, func, args, indent, depth, generics),
        Expr::FuncValue { callee, ty } => emit_func_value(ctx, callee, ty, generics),
        Expr::Match(m) => emit_match(ctx, m, indent, depth, generics),
        // F1 (auto-force): a discarded Task binding becomes
        //   task_and_then(<effect>, Box::new(move |_| { <rest> }))
        // so the future is properly awaited rather than silently dropped.
        //
        // ARGUMENT ORDER: the runtime `task_and_then(task, f)` takes the effect
        // FIRST and the continuation SECOND. Rust evaluates function arguments
        // left-to-right, so the effect expression is evaluated before the
        // continuation closure is constructed. This matters when the same `Db`
        // pool handle is used in both the effect (e.g. `db_exec_raw(conn, ...)`)
        // and the continuation (`move |_| { ... conn ... }`): placing the effect
        // first lets the continuation capture the pool handle by move without a
        // double-move error, provided that Db kernels emit `conn.clone()` for the
        // pool argument (see `emit_db_call`).
        //
        // The closure parameter type and return type are inferred by Rust from
        // the task_and_then signature — `effect_s: SkyTask<A>` pins A (the
        // discarded type) and `rest_s: SkyTask<B>` pins B (the result type),
        // avoiding the incorrect hardcoded `()` that would fail for any non-unit
        // effect type or non-unit rest type.
        Expr::TaskSeq { effect, rest } => {
            let child = depth + 1;
            let effect_s = emit_expr_at(ctx, effect, indent, child, generics)?;
            let rest_s = emit_expr_at(ctx, rest, indent, child, generics)?;
            Ok(format!(
                "task_and_then({effect_s}, Box::new(move |_| {{ {rest_s} }}))"
            ))
        }
        // TCO nodes are produced by the lowerer's rewrite and consumed by
        // `emit_func` / `emit_expr_tail`; reaching one on the ordinary value-emit
        // path means the rewrite left a jump/loop outside a tail context — a
        // compiler bug, surfaced fail-closed (never a panic, never a wildcard).
        Expr::TailLoop { .. } | Expr::TailRecur { .. } => Err(Diagnostic::CompilerBug {
            where_: "sky_backend_rust::emit_expr_at",
            detail: "TailLoop/TailRecur reached the non-tail emit path".to_string(),
        }),
    }
}

/// Emit a list literal. A non-empty list renders as `vec![e0, e1, …]`; the empty
/// list as a typed `Vec::<T>::new()` so its element type is never ambiguous (a
/// bare `vec![]` could fail to infer in a polymorphic position). Kept out of the
/// `emit_expr_at` match (`#[inline(never)]`) so its locals don't inflate the
/// recursive frame.
#[inline(never)]
fn emit_list(
    ctx: &EmitCtx,
    elem: &IrType,
    items: &[Expr],
    indent: usize,
    depth: u16,
    generics: GenericScope,
) -> DResult<String> {
    let child = depth + 1;
    if items.is_empty() {
        // For `IrType::Json` (unresolved / wildcard `Value = any`) the type
        // annotation `Vec::<JsonVal>::new()` CONFLICTS with callers that expect
        // `Vec<Attribute<M>>` / `Vec<Element<M>>` etc. — Rust's type checker
        // rejects the explicit annotation even though it would accept an
        // unannotated `Vec::new()` via inference.  Emit the bare form and let
        // Rust infer the element type from the surrounding call's expected type.
        // All other element types are precise enough that an explicit annotation
        // resolves ambiguity without breaking callers.
        if matches!(elem, IrType::Json) {
            return Ok("Vec::new()".to_owned());
        }
        let ty = render_type(ctx, elem, generics)?;
        return Ok(format!("Vec::<{ty}>::new()"));
    }
    let mut parts = Vec::with_capacity(items.len());
    for item in items {
        parts.push(emit_expr_at(ctx, item, indent, child, generics)?);
    }
    // Non-empty lists whose element type is a parametric Ui type (`Attribute<M>`
    // / `Element<M>` / `Html<M>` / …) need an explicit type annotation on the
    // emitted Rust `Vec` so that the `M` type parameter can be inferred by the
    // Rust compiler.  Without this, callers like `Ui.layoutWith` whose attrs
    // lists are always non-empty (no empty-list turbofish to anchor M) produce
    // E0283 because every helper (`ui_padding_`, `ui_spacing_`, …) is itself
    // generic in M and no concrete M appears elsewhere in the expression.
    //
    // The annotation wraps the vec in a typed `let` block:
    //   `{ let __sky_m: Vec<Attribute<()>> = vec![ui_padding_(12)]; __sky_m }`
    // The variable name `__sky_m` is scoped to the anonymous block and cannot
    // shadow user-visible bindings.  The block is a Rust expression, valid in
    // every argument position.
    //
    // This path is skipped for `IrType::Json` (the elem type is unresolved)
    // because annotating with `Vec<JsonVal>` would CONFLICT with callers that
    // expect `Vec<Attribute<M>>` — the same reason empty Json lists emit bare
    // `Vec::new()` rather than a typed form.
    if matches!(elem, IrType::Ui { .. }) {
        let ty = render_type(ctx, elem, generics)?;
        return Ok(format!(
            "{{ let __sky_m: Vec<{ty}> = vec![{}]; __sky_m }}",
            parts.join(", ")
        ));
    }
    Ok(format!("vec![{}]", parts.join(", ")))
}

/// Emit a constructor application. A nullary constructor renders as the bare
/// path `EnumName::Variant` (byte-identical to M0); a payload constructor renders
/// `EnumName::Variant(arg0, arg1, …)`. A payload position on a type-size cycle
/// back to its own enum is wrapped in `Box::new(…)` to balance the boxed enum
/// field (see [`crate::EmitCtx::is_cyclic_self_field`]). Kept out of the
/// `emit_expr_at` match (`#[inline(never)]`) so its locals don't inflate the
/// recursive frame.
#[inline(never)]
fn emit_ctor(
    ctx: &EmitCtx,
    ty: Symbol,
    variant: Symbol,
    args: &[Expr],
    indent: usize,
    depth: u16,
    generics: GenericScope,
) -> DResult<String> {
    let child = depth + 1;
    // A built-in `Maybe` / `Result` constructor routes to the runtime enum
    // (`SkyMaybe::Just(..)`, `SkyResult::Err(..)`); its payload is never a
    // self-recursive user field, so no field-boxing lookup applies.
    if let Some(runtime) = ctx.builtin_runtime_enum(ty) {
        let path = format!("{runtime}::{}", ctx.emit_ident(variant)?);
        if args.is_empty() {
            return Ok(path);
        }
        let mut parts = Vec::with_capacity(args.len());
        for arg in args {
            parts.push(emit_expr_at(ctx, arg, indent, child, generics)?);
        }
        return Ok(format!("{path}({})", parts.join(", ")));
    }
    let path = format!("{}::{}", ctx.enum_name(ty)?, ctx.emit_ident(variant)?);
    if args.is_empty() {
        return Ok(path);
    }
    let fields = ctx.variant_fields(ty, variant)?;
    if fields.len() != args.len() {
        return Err(Diagnostic::CompilerBug {
            where_: "sky_backend_rust::emit_ctor",
            detail: format!(
                "constructor {} of enum {} applied to {} args but declares {} fields; \
                 a constructor application must be saturated",
                variant.as_raw(),
                ty.as_raw(),
                args.len(),
                fields.len()
            ),
        });
    }
    let mut parts = Vec::with_capacity(args.len());
    for (arg, field_ty) in args.iter().zip(fields.iter()) {
        let rendered = emit_expr_at(ctx, arg, indent, child, generics)?;
        // A cyclic self-edge field is boxed in the enum, so its construction
        // argument is boxed too.
        if ctx.is_cyclic_self_field(field_ty, ty) {
            parts.push(format!("Box::new({rendered})"));
        } else {
            parts.push(rendered);
        }
    }
    Ok(format!("{path}({})", parts.join(", ")))
}

/// Emit a `match`. An arm head is a constructor pattern (M3a/M3b-2, exhaustive
/// over the enum's variants) or — for the M3b-3 flat refutable match — a literal
/// (`0` / `'a'` / `"hi"` / `true` / `false`), a wildcard / variable binder, or
/// an alias. A cyclic self-edge constructor payload field is boxed in the enum,
/// so a variable bound to such a field is unboxed (`let x = *x;`) at the top of
/// the arm body, giving the binder the enum's own (owned) type rather than
/// `Box<…>`.
///
/// `String` scrutinees match against `scrut.as_str()` because Rust string
/// literal patterns are `&str`; any top-level binder in such an arm is rebound
/// to an owned `String` (`let name = name.to_string();`) so the arm body sees
/// the Sky `String` type, keeping the lowering sound. Kept out of the
/// `emit_expr_at` match (`#[inline(never)]`) for the same frame-size reason as
/// the neighbouring helpers.
#[inline(never)]
fn emit_match(
    ctx: &EmitCtx,
    m: &Match,
    indent: usize,
    depth: u16,
    generics: GenericScope,
) -> DResult<String> {
    let child = depth + 1;
    let (scrut, str_mode, list_mode) = emit_match_scrutinee(ctx, m, indent, depth, generics)?;
    let arm_indent = indent_of(indent + 1);
    let close_indent = indent_of(indent);
    let mut arms = Vec::with_capacity(m.arms().len());
    for arm in m.arms() {
        let (pat, prelude) = emit_arm_head(ctx, &arm.pat, str_mode, list_mode)?;
        let body = emit_expr_at(ctx, &arm.body, indent + 1, child, generics)?;
        let arm_body = if prelude.is_empty() {
            body
        } else {
            format!("{{ {prelude}{body} }}")
        };
        arms.push(format!("{arm_indent}{pat} => {arm_body},"));
    }
    Ok(format!(
        "match {scrut} {{\n{}\n{close_indent}}}",
        arms.join("\n")
    ))
}

/// Emit the scrutinee of a `Match` plus its two mode flags. A string scrutinee is
/// matched as `&str` (so literal patterns apply) — the presence of a `Pat::Str`
/// head is the reliable signal (the type checker proved the scrutinee a
/// `String`). A LIST scrutinee (the runtime's `Vec<T>`) is matched as a slice so
/// the native Rust slice patterns `[]` / `[a, b]` / `[x, rest @ ..]` apply — a
/// `Pat::Slice` head is the signal. Shared by the value-context (`emit_match`)
/// and tail-context (`emit_expr_tail`) match emitters so the two agree exactly.
fn emit_match_scrutinee(
    ctx: &EmitCtx,
    m: &Match,
    indent: usize,
    depth: u16,
    generics: GenericScope,
) -> DResult<(String, bool, bool)> {
    let child = depth + 1;
    let scrut_expr = emit_expr_at(ctx, m.scrutinee(), indent, child, generics)?;
    let str_mode = m.arms().iter().any(|a| matches!(a.pat, Pat::Str(_)));
    let list_mode = m.arms().iter().any(|a| matches!(a.pat, Pat::Slice { .. }));
    let scrut = if str_mode {
        format!("({scrut_expr}).as_str()")
    } else if list_mode {
        format!("({scrut_expr}).as_slice()")
    } else {
        scrut_expr
    };
    Ok((scrut, str_mode, list_mode))
}

/// Render one match-arm head to its Rust pattern plus any leading rebind/unbox
/// prelude. A constructor head goes through `emit_ctor_arm_pat` (which unboxes a
/// cyclic self-field binder); a flat-match leaf head — literal / wildcard /
/// variable / alias / slice — goes through `render_pat` (total over the whole
/// set), with a `String`/slice binder rebind prelude in string/list mode. Shared
/// by the value-context and tail-context match emitters.
fn emit_arm_head(
    ctx: &EmitCtx,
    pat: &Pat,
    str_mode: bool,
    list_mode: bool,
) -> DResult<(String, String)> {
    if let Pat::Ctor { ty, variant, args } = pat {
        emit_ctor_arm_pat(ctx, *ty, *variant, args)
    } else {
        let prelude = if str_mode {
            str_binder_rebinds(ctx, pat)?
        } else if list_mode {
            list_binder_rebinds(ctx, pat)?
        } else {
            String::new()
        };
        Ok((render_pat(ctx, pat)?, prelude))
    }
}

/// Render a constructor arm head to its Rust pattern plus any leading unbox
/// statements. A cyclic self-edge payload field is boxed in the enum, so a
/// variable bound to it is unboxed (`let x = *x;`) at the arm body's head.
fn emit_ctor_arm_pat(
    ctx: &EmitCtx,
    ty: Symbol,
    variant: Symbol,
    args: &[Pat],
) -> DResult<(String, String)> {
    // A built-in `Maybe` / `Result` pattern matches the runtime enum; its
    // payload is never a boxed self-edge field, so no unbox prelude is needed.
    if let Some(runtime) = ctx.builtin_runtime_enum(ty) {
        let path = format!("{runtime}::{}", ctx.emit_ident(variant)?);
        if args.is_empty() {
            return Ok((path, String::new()));
        }
        let mut sub_pats = Vec::with_capacity(args.len());
        for sub in args {
            sub_pats.push(render_pat(ctx, sub)?);
        }
        return Ok((format!("{path}({})", sub_pats.join(", ")), String::new()));
    }
    let path = format!("{}::{}", ctx.enum_name(ty)?, ctx.emit_ident(variant)?);
    if args.is_empty() {
        return Ok((path, String::new()));
    }
    let fields = ctx.variant_fields(ty, variant)?;
    if fields.len() != args.len() {
        return Err(Diagnostic::CompilerBug {
            where_: "sky_backend_rust::emit_match",
            detail: format!(
                "constructor pattern {} of enum {} binds {} sub-patterns but the \
                 variant declares {} fields",
                variant.as_raw(),
                ty.as_raw(),
                args.len(),
                fields.len()
            ),
        });
    }
    let mut sub_pats = Vec::with_capacity(args.len());
    let mut unbox_lines = String::new();
    for (sub, field_ty) in args.iter().zip(fields.iter()) {
        sub_pats.push(render_pat(ctx, sub)?);
        // A variable bound to a boxed self-edge field is unboxed so the body
        // sees the payload's own type, not `Box<…>`.
        if ctx.is_cyclic_self_field(field_ty, ty)
            && let Pat::Var(s) = sub
        {
            let binder = ctx.emit_ident(*s)?;
            write!(unbox_lines, "let {binder} = *{binder}; ").map_err(|e| {
                Diagnostic::CompilerBug {
                    where_: "sky_backend_rust::emit_match",
                    detail: format!("writing unbox binder failed: {e}"),
                }
            })?;
        }
    }
    Ok((format!("{path}({})", sub_pats.join(", ")), unbox_lines))
}

/// Build the `let name = name.to_string();` prelude that rebinds every top-level
/// binder a string-match arm introduces from `&str` to an owned `String`, so the
/// arm body sees the Sky `String` type. A variable binds itself; an alias binds
/// its name and recurses into its inner pattern; a wildcard / literal binds
/// nothing.
fn str_binder_rebinds(ctx: &EmitCtx, pat: &Pat) -> DResult<String> {
    let mut out = String::new();
    collect_str_rebinds(ctx, pat, &mut out)?;
    Ok(out)
}

fn collect_str_rebinds(ctx: &EmitCtx, pat: &Pat, out: &mut String) -> DResult<()> {
    match pat {
        Pat::Var(s) => {
            let name = ctx.emit_ident(*s)?;
            write!(out, "let {name} = {name}.to_string(); ").map_err(|e| {
                Diagnostic::CompilerBug {
                    where_: "sky_backend_rust::str_binder_rebinds",
                    detail: format!("writing rebind binder failed: {e}"),
                }
            })?;
            Ok(())
        }
        Pat::Alias(inner, name) => {
            let n = ctx.emit_ident(*name)?;
            write!(out, "let {n} = {n}.to_string(); ").map_err(|e| Diagnostic::CompilerBug {
                where_: "sky_backend_rust::str_binder_rebinds",
                detail: format!("writing rebind binder failed: {e}"),
            })?;
            collect_str_rebinds(ctx, inner, out)
        }
        // A string scrutinee admits no constructor / tuple / record / non-string
        // literal head (the type checker proves the scrutinee a `String`); these
        // introduce no `String`-typed binder to rebind.
        Pat::Wildcard
        | Pat::Str(_)
        | Pat::Int(_)
        | Pat::Bool(_)
        | Pat::Char(_)
        | Pat::Ctor { .. }
        | Pat::Tuple(_)
        | Pat::Record(_)
        | Pat::Slice { .. } => Ok(()),
    }
}

/// In LIST mode the scrutinee is matched as a slice (`(v).as_slice()`), so every
/// binder a list arm introduces is a borrow: an ELEMENT binder is `&T` and a
/// REST / whole-list binder is `&[T]`. This builds the `let … = …;` prelude that
/// rebinds each to the owned Sky value the arm body expects — an element via
/// `.clone()` (so the body sees `T`), a rest / whole list via `.to_vec()` (so the
/// body sees `Vec<T>`). Cloning is the sound owned destructure of a shared slice;
/// the lowerer gates a list `case` binding a still-generic (non-`Clone`) element
/// type (SKY-L0102), so the `.clone()` / `.to_vec()` always resolve.
fn list_binder_rebinds(ctx: &EmitCtx, pat: &Pat) -> DResult<String> {
    let mut out = String::new();
    match pat {
        Pat::Slice { prefix, rest } => {
            for sub in prefix {
                collect_elem_rebinds(ctx, sub, &mut out)?;
            }
            if let Some(r) = rest {
                collect_list_rebinds(ctx, r, &mut out)?;
            }
        }
        // A whole-list catch-all binder (`xs ->`) or an alias over a list arm
        // (`(x :: rest) as whole ->`): the matched value IS the list.
        Pat::Var(_) => collect_list_rebinds(ctx, pat, &mut out)?,
        Pat::Alias(inner, name) => {
            rebind_to_vec(ctx, *name, &mut out)?;
            out.push_str(&list_binder_rebinds(ctx, inner)?);
        }
        // A wildcard binds nothing; other heads never reach a list `case`.
        Pat::Wildcard
        | Pat::Str(_)
        | Pat::Int(_)
        | Pat::Bool(_)
        | Pat::Char(_)
        | Pat::Ctor { .. }
        | Pat::Tuple(_)
        | Pat::Record(_) => {}
    }
    Ok(out)
}

/// Collect the owned-by-`clone` rebinds for an ELEMENT sub-pattern (a head
/// position of a slice). Every variable / alias binder there is `&T` and is
/// cloned to `T`; nested tuple / constructor / record element patterns recurse.
fn collect_elem_rebinds(ctx: &EmitCtx, pat: &Pat, out: &mut String) -> DResult<()> {
    match pat {
        Pat::Var(s) => rebind_clone(ctx, *s, out),
        Pat::Alias(inner, name) => {
            rebind_clone(ctx, *name, out)?;
            collect_elem_rebinds(ctx, inner, out)
        }
        Pat::Tuple(subs) => {
            for sub in subs {
                collect_elem_rebinds(ctx, sub, out)?;
            }
            Ok(())
        }
        Pat::Ctor { args, .. } => {
            for sub in args {
                collect_elem_rebinds(ctx, sub, out)?;
            }
            Ok(())
        }
        Pat::Record(fields) => {
            for (_, sub) in fields {
                collect_elem_rebinds(ctx, sub, out)?;
            }
            Ok(())
        }
        // A wildcard / literal element binds nothing. A nested slice element is
        // gated at lowering (it never reaches the backend), so it needs no rebind.
        Pat::Wildcard
        | Pat::Int(_)
        | Pat::Bool(_)
        | Pat::Char(_)
        | Pat::Str(_)
        | Pat::Slice { .. } => Ok(()),
    }
}

/// Collect the owned-by-`to_vec` rebinds for a REST / whole-list binder (`&[T]`
/// → `Vec<T>`). The lowerer admits only a variable / wildcard rest, so this is a
/// single binder (an alias recurses defensively).
fn collect_list_rebinds(ctx: &EmitCtx, pat: &Pat, out: &mut String) -> DResult<()> {
    match pat {
        Pat::Var(s) => rebind_to_vec(ctx, *s, out),
        Pat::Alias(inner, name) => {
            rebind_to_vec(ctx, *name, out)?;
            collect_list_rebinds(ctx, inner, out)
        }
        Pat::Wildcard
        | Pat::Int(_)
        | Pat::Bool(_)
        | Pat::Char(_)
        | Pat::Str(_)
        | Pat::Ctor { .. }
        | Pat::Tuple(_)
        | Pat::Record(_)
        | Pat::Slice { .. } => Ok(()),
    }
}

/// Emit `let <name> = <name>.clone();` — rebind a slice ELEMENT binder (`&T`) to
/// the owned `T` the arm body expects.
fn rebind_clone(ctx: &EmitCtx, sym: Symbol, out: &mut String) -> DResult<()> {
    let name = ctx.emit_ident(sym)?;
    write!(out, "let {name} = {name}.clone(); ").map_err(|e| Diagnostic::CompilerBug {
        where_: "sky_backend_rust::list_binder_rebinds",
        detail: format!("writing element rebind failed: {e}"),
    })
}

/// Emit `let <name> = <name>.to_vec();` — rebind a slice REST / whole-list binder
/// (`&[T]`) to the owned `Vec<T>` the arm body expects.
fn rebind_to_vec(ctx: &EmitCtx, sym: Symbol, out: &mut String) -> DResult<()> {
    let name = ctx.emit_ident(sym)?;
    write!(out, "let {name} = {name}.to_vec(); ").map_err(|e| Diagnostic::CompilerBug {
        where_: "sky_backend_rust::list_binder_rebinds",
        detail: format!("writing rest rebind failed: {e}"),
    })
}

/// Render a pattern to its Rust spelling. Total and recursive over the entire
/// M3a/M3b-1/M3b-2/M3b-3 pattern set:
///
/// * a variable binder (the keyword-mangled name),
/// * a wildcard (`_`),
/// * a literal leaf — int (`0`), bool (`true`), char (`'a'`), string (`"hi"`),
/// * an alias / `as` pattern (`name @ <inner>`),
/// * a tuple pattern (`(sub0, sub1, …)`),
/// * a constructor pattern (`EnumName::Variant` / `EnumName::Variant(sub0, …)`),
/// * a record pattern (`RecXY { x: sub0, y: sub1, .. }`).
///
/// Every nested sub-position recurses through this same function, so an
/// arbitrarily nested shape (`Just (a, b)`, `Node (Node …) x r`,
/// `{ point = (a, b) }`) renders correctly. The renderer stays total: no arm
/// panics, and every fallible lookup is surfaced as a [`Diagnostic`].
fn render_pat(ctx: &EmitCtx, pat: &Pat) -> DResult<String> {
    match pat {
        Pat::Var(sym) => ctx.emit_ident(*sym),
        Pat::Wildcard => Ok("_".to_owned()),
        // Literal leaves render as Rust literals. Int reuses the same spelling as
        // the `Expr::Int` emitter; Bool maps to the Rust keyword constant; Char
        // and Str escape via the `{:?}` Debug form, which produces a valid Rust
        // literal (quotes, backslashes and control chars escaped) and is
        // deterministic.
        Pat::Int(n) => Ok(n.to_string()),
        Pat::Bool(b) => Ok(if *b { "true" } else { "false" }.to_owned()),
        // A well-formed Char pattern carries exactly one character → Rust char
        // literal. A malformed (multi-char / empty) carried string falls back to
        // a string literal rather than emitting invalid Rust, staying total.
        Pat::Char(c) => {
            let mut chars = c.chars();
            match (chars.next(), chars.next()) {
                (Some(ch), None) => Ok(format!("{ch:?}")),
                _ => Ok(format!("{c:?}")),
            }
        }
        Pat::Str(s) => Ok(format!("{s:?}")),
        // `inner as name` → Rust binding-with-subpattern `name @ <inner>`. The
        // inner sub-pattern recurses through this same total renderer.
        Pat::Alias(inner, name) => {
            let name = ctx.emit_ident(*name)?;
            let inner = render_pat(ctx, inner)?;
            Ok(format!("{name} @ {inner}"))
        }
        Pat::Tuple(elems) => {
            // A tuple pattern destructures element-by-element: `(p0, p1, …)`.
            // Stays total over any element vector (no arity assumption).
            let mut subs = Vec::with_capacity(elems.len());
            for sub in elems {
                subs.push(render_pat(ctx, sub)?);
            }
            Ok(format!("({})", subs.join(", ")))
        }
        Pat::Ctor { ty, variant, args } => {
            // A built-in `Maybe` / `Result` pattern routes to the runtime enum
            // path; otherwise it is a user enum resolved by `enum_name`.
            let path = match ctx.builtin_runtime_enum(*ty) {
                Some(runtime) => format!("{runtime}::{}", ctx.emit_ident(*variant)?),
                None => format!("{}::{}", ctx.enum_name(*ty)?, ctx.emit_ident(*variant)?),
            };
            if args.is_empty() {
                Ok(path)
            } else {
                let mut subs = Vec::with_capacity(args.len());
                for sub in args {
                    subs.push(render_pat(ctx, sub)?);
                }
                Ok(format!("{path}({})", subs.join(", ")))
            }
        }
        Pat::Record(fields) => render_record_pat(ctx, fields),
        // A list / cons pattern renders as a native Rust slice pattern. A closed
        // (exact-length) pattern is `[p0, p1]`; an open cons tail is
        // `[p0, p1, rest @ ..]` (binding the rest) or `[p0, p1, ..]` (ignoring
        // it). The leading element patterns recurse through this same renderer.
        Pat::Slice { prefix, rest } => {
            let mut parts = Vec::with_capacity(prefix.len() + 1);
            for sub in prefix {
                parts.push(render_pat(ctx, sub)?);
            }
            match rest {
                Some(r) => {
                    parts.push(render_rest_pat(ctx, r)?);
                    Ok(format!("[{}]", parts.join(", ")))
                }
                None => Ok(format!("[{}]", parts.join(", "))),
            }
        }
    }
}

/// Render the open TAIL of a slice pattern — the `rest @ ..` / `..` suffix. A
/// variable binds the remaining slice (`name @ ..`); a wildcard ignores it
/// (`..`). The lowerer admits only these two rest shapes ([`crate`]-side
/// `lower_rest_pat` gates the rest), so the renderer is total over them.
fn render_rest_pat(ctx: &EmitCtx, pat: &Pat) -> DResult<String> {
    match pat {
        Pat::Var(s) => Ok(format!("{} @ ..", ctx.emit_ident(*s)?)),
        // A wildcard ignores the tail (`..`). No other rest shape is produced by
        // the lowerer, so the catch-all stays total — a bare `..` ignores the
        // tail rather than mis-rendering.
        _ => Ok("..".to_owned()),
    }
}

/// Render a record pattern `{ field0 = p0, … }` to a Rust struct pattern
/// `RecXY { field0: p0, …, .. }`.
///
/// The struct is resolved by the pattern's field-name set, exactly as a record
/// LITERAL resolves its struct (Rust names struct-pattern fields, so write order
/// is free). The lowerer surfaces the complete field set, so this exact-set
/// lookup is unambiguous; a miss is an upstream-contract violation surfaced as a
/// [`Diagnostic::CompilerBug`] rather than a silent mis-emit.
///
/// A trailing `..` is always emitted: it both matches the canonical struct-
/// pattern shape and makes the rendering robust to a field the pattern does not
/// bind (zero remaining fields under the complete-set contract — a legal,
/// no-op `..`). A field whose sub-pattern is a variable bound to the field's own
/// name renders in Rust shorthand (`x` rather than the lint-flagged `x: x`).
fn render_record_pat(ctx: &EmitCtx, fields: &[(Symbol, Pat)]) -> DResult<String> {
    // Resolve the struct by the (sorted) set of bound field names.
    let mut key = Vec::with_capacity(fields.len());
    for (sym, _) in fields {
        key.push(ctx.resolve_ident(*sym)?.to_owned());
    }
    let struct_name = ctx.record_name_for_literal(&key)?.to_owned();

    let mut parts = Vec::with_capacity(fields.len());
    for (sym, sub) in fields {
        let field_ident = ctx.emit_ident(*sym)?;
        // Field-pun shorthand: `Rec { x, .. }` instead of `Rec { x: x, .. }`
        // (the latter trips rustc's `non_shorthand_field_patterns` lint). Only
        // when the sub-pattern is a variable whose emitted name equals the
        // field's emitted name.
        if let Pat::Var(var) = sub
            && ctx.emit_ident(*var)? == field_ident
        {
            parts.push(field_ident);
        } else {
            let rendered = render_pat(ctx, sub)?;
            parts.push(format!("{field_ident}: {rendered}"));
        }
    }
    // An empty entry vector is degenerate (the lowerer never produces it), but
    // stay total: render `Rec { .. }` rather than the invalid `Rec { , .. }`.
    if parts.is_empty() {
        Ok(format!("{struct_name} {{ .. }}"))
    } else {
        Ok(format!("{struct_name} {{ {}, .. }}", parts.join(", ")))
    }
}

/// Emit a record literal `{ x = e1, ... }` as a named struct literal
/// `RecXY { x: <e1>, ... }`. `depth` is the literal's own IR-nesting level; its
/// field values are emitted one level deeper. Kept out of the `emit_expr_at`
/// match (`#[inline(never)]`) so its locals don't inflate the recursive frame.
#[inline(never)]
fn emit_record(
    ctx: &EmitCtx,
    fields: &[(Symbol, Expr)],
    indent: usize,
    depth: u16,
    generics: GenericScope,
) -> DResult<String> {
    let child = depth + 1;
    // The struct is resolved by the literal's field-name set (Rust names
    // struct-literal fields, so write order is free); the field idents are
    // keyword-mangled to match the struct definition.
    let mut key = Vec::with_capacity(fields.len());
    for (sym, _) in fields {
        key.push(ctx.resolve_ident(*sym)?.to_owned());
    }
    let struct_name = ctx.record_name_for_literal(&key)?.to_owned();
    let mut parts = Vec::with_capacity(fields.len());
    for (sym, value) in fields {
        let field_ident = ctx.emit_ident(*sym)?;
        let rendered = emit_expr_at(ctx, value, indent, child, generics)?;
        parts.push(format!("{field_ident}: {rendered}"));
    }
    Ok(format!("{struct_name} {{ {} }}", parts.join(", ")))
}

/// Emit a functional record update `{ record | f = v, ... }` as a clone-and-
/// reassign block: `{ let mut __sky_rec = (<record>).clone(); __sky_rec.f = v;
/// __sky_rec }`. This needs no struct name and leaves the source record
/// untouched; the block scope makes the temporary safe under nesting. Kept out
/// of the match (`#[inline(never)]`) for the same frame-size reason as
/// [`emit_record`].
#[inline(never)]
fn emit_update(
    ctx: &EmitCtx,
    record: &Expr,
    fields: &[(Symbol, Expr)],
    indent: usize,
    depth: u16,
    generics: GenericScope,
) -> DResult<String> {
    let child = depth + 1;
    let base = emit_expr_at(ctx, record, indent, child, generics)?;
    let mut assigns = Vec::with_capacity(fields.len());
    for (sym, value) in fields {
        let field_ident = ctx.emit_ident(*sym)?;
        let rendered = emit_expr_at(ctx, value, indent, child, generics)?;
        assigns.push(format!(" __sky_rec.{field_ident} = {rendered};"));
    }
    Ok(format!(
        "{{ let mut __sky_rec = ({base}).clone();{} __sky_rec }}",
        assigns.concat()
    ))
}

/// Emit an `Expr` in TAIL/STATEMENT context — the interior of a `TailLoop`'s
/// `loop { … }` (task #49). Every path ends in either a `return <expr>;` (a leaf
/// tail position) or a `continue;` (a `TailRecur` jump), so the `loop` types as
/// `!` and unifies with any `-> R` return type (no `break value`). The tail
/// propagators (`If` / `Match` / `Let` / `Destructure`) recurse in-tail; every
/// other node is a leaf whose VALUE is `return`ed. `loop_params` gives each
/// `TailRecur.args[i]` its destination parameter name.
///
/// The `other => return` arm is the intended value/statement split (the
/// reference's `walk True` leaf case), NOT a wildcard over `Expr` variants for
/// exhaustiveness purposes — `emit_expr_at` inside it is the exhaustive,
/// fail-closed walker: a stray `TailLoop`/`TailRecur` reaching it routes to the
/// `CompilerBug` arm (never a panic, never a silent swallow).
#[inline(never)]
fn emit_expr_tail(
    ctx: &EmitCtx,
    expr: &Expr,
    indent: usize,
    depth: u16,
    generics: GenericScope,
    loop_params: &[(Symbol, IrType)],
) -> DResult<String> {
    let pad = indent_of(indent);
    let child = depth + 1;
    match expr {
        Expr::If { cond, then_, else_ } => {
            let c = emit_expr_at(ctx, cond, indent, child, generics)?;
            let t = emit_expr_tail(ctx, then_, indent + 1, child, generics, loop_params)?;
            let e = emit_expr_tail(ctx, else_, indent + 1, child, generics, loop_params)?;
            Ok(format!(
                "{pad}if {c} {{\n{t}\n{pad}}} else {{\n{e}\n{pad}}}"
            ))
        }
        Expr::Match(m) => {
            let (scrut, str_mode, list_mode) =
                emit_match_scrutinee(ctx, m, indent, depth, generics)?;
            let arm_indent = indent_of(indent + 1);
            let close_indent = indent_of(indent);
            let mut arms = Vec::with_capacity(m.arms().len());
            for arm in m.arms() {
                let (patstr, prelude) = emit_arm_head(ctx, &arm.pat, str_mode, list_mode)?;
                // The arm body is a STATEMENT sequence ending in return/continue;
                // any binder-rebind prelude precedes it inside the arm's block.
                let body =
                    emit_expr_tail(ctx, &arm.body, indent + 2, child, generics, loop_params)?;
                let inner = if prelude.is_empty() {
                    body
                } else {
                    format!("{}{prelude}\n{body}", indent_of(indent + 2))
                };
                arms.push(format!(
                    "{arm_indent}{patstr} => {{\n{inner}\n{arm_indent}}}"
                ));
            }
            Ok(format!(
                "{pad}match {scrut} {{\n{}\n{close_indent}}}",
                arms.join("\n")
            ))
        }
        Expr::Let { name, value, body } => {
            let n = ctx.emit_ident(*name)?;
            let v = emit_expr_at(ctx, value, indent, child, generics)?;
            let b = emit_expr_tail(ctx, body, indent, child, generics, loop_params)?;
            Ok(format!("{pad}let {n} = {v};\n{b}"))
        }
        Expr::Destructure {
            binder,
            value,
            body,
        } => {
            let bnd = render_pat(ctx, binder)?;
            let v = emit_expr_at(ctx, value, indent, child, generics)?;
            let b = emit_expr_tail(ctx, body, indent, child, generics, loop_params)?;
            Ok(format!("{pad}let {bnd} = {v};\n{b}"))
        }
        // The jump: temporaries-first reassignment + `continue`. Reading EVERY
        // next-iteration argument into a fresh `__tco_<i>` temp BEFORE any
        // parameter write forecloses the arg-swap clobber (`go b a rest` must not
        // read an already-overwritten `a`); each temp reads the CURRENT params.
        Expr::TailRecur { args } => {
            if args.len() != loop_params.len() {
                // Invariant broken by the rewrite — fail closed, never panic.
                return Err(Diagnostic::CompilerBug {
                    where_: "sky_backend_rust::emit_expr_tail",
                    detail: format!(
                        "TailRecur has {} args but the enclosing TailLoop has {} params",
                        args.len(),
                        loop_params.len()
                    ),
                });
            }
            let mut temps = String::new();
            for (idx, arg) in args.iter().enumerate() {
                let a = emit_expr_at(ctx, arg, indent, child, generics)?;
                writeln!(temps, "{pad}let __tco_{idx} = {a};").map_err(|e| {
                    Diagnostic::CompilerBug {
                        where_: "sky_backend_rust::emit_expr_tail",
                        detail: format!("writing TCO jump temp failed: {e}"),
                    }
                })?;
            }
            let mut writes = String::new();
            for (idx, (name, _ty)) in loop_params.iter().enumerate() {
                let n = ctx.emit_ident(*name)?;
                writeln!(writes, "{pad}{n} = __tco_{idx};").map_err(|e| {
                    Diagnostic::CompilerBug {
                        where_: "sky_backend_rust::emit_expr_tail",
                        detail: format!("writing TCO param reassignment failed: {e}"),
                    }
                })?;
            }
            Ok(format!("{temps}{writes}{pad}continue;"))
        }
        // Every other node is a leaf tail position → return its value.
        other => {
            let v = emit_expr_at(ctx, other, indent, child, generics)?;
            Ok(format!("{pad}return {v};"))
        }
    }
}

/// Emit an application of a first-class function value, `(<func>)(<args>)`. The
/// callee is parenthesised so a boxed `dyn Fn` (or any expression value) is
/// applied uniformly — a `Box<dyn Fn(..)>` auto-derefs at the call. `depth` is
/// the application's own IR-nesting level; its callee and arguments are emitted
/// one level deeper. Kept out of the `emit_expr_at` match (`#[inline(never)]`)
/// so its `Vec`/`String` locals don't inflate the recursive frame.
#[inline(never)]
fn emit_apply(
    ctx: &EmitCtx,
    func: &Expr,
    args: &[Expr],
    indent: usize,
    depth: u16,
    generics: GenericScope,
) -> DResult<String> {
    let child = depth + 1;
    let f = emit_expr_at(ctx, func, indent, child, generics)?;
    let mut parts = Vec::with_capacity(args.len());
    for arg in args {
        parts.push(emit_expr_at(ctx, arg, indent, child, generics)?);
    }
    Ok(format!("({f})({})", parts.join(", ")))
}

/// Emit a top-level function (or kernel) named as a first-class *value* as a
/// type-pinned boxed closure: `{ let __sky_fn: Box<dyn Fn(..) -> R> =
/// Box::new(<name>); __sky_fn }`. The explicit binding type drives the unsized
/// coercion of the named `fn` item (a zero-sized `Fn` implementor) to the boxed
/// trait object, so the value fills a `Box<dyn Fn(..) -> R>` slot uniformly in
/// every position — argument, return, or let-binding — rather than relying on a
/// coercion site that an `if`/`match` branch or a bare `let` would not provide.
/// `ty` is the value's `Fun` IR type; [`render_type`] renders it as the boxed
/// trait object. Kept out of the `emit_expr_at` match (`#[inline(never)]`) for
/// the same frame-size reason as the neighbouring helpers.
#[inline(never)]
fn emit_func_value(
    ctx: &EmitCtx,
    callee: &Callee,
    ty: &IrType,
    generics: GenericScope,
) -> DResult<String> {
    let name = callee_name(ctx, callee)?;
    let boxed = render_type(ctx, ty, generics)?;
    Ok(format!(
        "{{ let __sky_fn: {boxed} = Box::new({name}); __sky_fn }}"
    ))
}

/// Emit a lambda `\p0 p1 ... -> body` as a boxed closure
/// `Box::new(move |p0: T0, ...| -> R { <body> })`. The `move` capture takes any
/// free locals by value; the explicit return type pins the closure's signature
/// so it coerces cleanly to the `Box<dyn Fn(..) -> R>` slot it fills. `depth` is
/// the lambda's own IR-nesting level; its body is emitted one level deeper. Kept
/// out of the `emit_expr_at` match (`#[inline(never)]`) for the same frame-size
/// reason as [`emit_record`] / [`emit_update`].
#[inline(never)]
fn emit_lambda(
    ctx: &EmitCtx,
    params: &[(Symbol, IrType)],
    ret: &IrType,
    body: &Expr,
    indent: usize,
    depth: u16,
    generics: GenericScope,
) -> DResult<String> {
    let child = depth + 1;
    let mut parts = Vec::with_capacity(params.len());
    for (param, ty) in params {
        parts.push(format!(
            "{}: {}",
            ctx.emit_ident(*param)?,
            render_type(ctx, ty, generics)?
        ));
    }
    let ret = render_type(ctx, ret, generics)?;
    let body = emit_expr_at(ctx, body, indent, child, generics)?;
    Ok(format!(
        "Box::new(move |{}| -> {ret} {{ {body} }})",
        parts.join(", ")
    ))
}

/// Render one type parameter's trailing bound clause for the generic list:
/// `: ::core::ops::Add<Output = T{n}> + Copy` and the like, or the empty string
/// for an unbounded variable (so an M2a structurally-parametric function emits a
/// bare `T{n}`, byte-identical to the pre-M2d golden).
///
/// `n` is the variable's 1-based position, which is also its own Rust name
/// `T{n}` — the arithmetic `::core::ops` traits take `Output = T{n}` so the
/// operation stays closed over the parameter's type (`x + x : T{n}`). The trait
/// order is fixed (`Add`, `Sub`, `Mul`, `PartialOrd`, `PartialEq`, `Ord`,
/// `Hash`, `Copy`, `Clone`) so the emission is deterministic regardless of how
/// the bound set was assembled.
fn render_bounds(bounds: BoundSet, n: usize) -> String {
    if bounds.is_unbounded() {
        return String::new();
    }
    let mut traits = Vec::new();
    if bounds.has_add() {
        traits.push(format!("::core::ops::Add<Output = T{n}>"));
    }
    if bounds.has_sub() {
        traits.push(format!("::core::ops::Sub<Output = T{n}>"));
    }
    if bounds.has_mul() {
        traits.push(format!("::core::ops::Mul<Output = T{n}>"));
    }
    if bounds.has_ord() {
        traits.push("PartialOrd".to_owned());
    }
    if bounds.has_eq() {
        traits.push("PartialEq".to_owned());
    }
    if bounds.has_ord_total() {
        // `Ord` (total order) for a `Set` element / sorted `Dict` op; carries
        // `Eq` + `PartialOrd` + `PartialEq` as supertraits, so a `Dict` key's
        // `HashMap` `Eq` requirement is met without a separate `Eq` bound.
        traits.push("Ord".to_owned());
    }
    if bounds.has_hash() {
        // `Hash` for a `Dict` key's `HashMap` backing. Fully qualified — the
        // trait (unlike its derive macro) is not in the Rust prelude.
        traits.push("::core::hash::Hash".to_owned());
    }
    if bounds.has_copy() {
        traits.push("Copy".to_owned());
    }
    if bounds.has_clone() {
        traits.push("Clone".to_owned());
    }
    format!(": {}", traits.join(" + "))
}

/// Emit a whole function item, including its trailing newline.
///
/// Shape: `pub fn <name>[<generics>](<params>) -> <ret> {\n    <body>\n}\n`. A
/// monomorphic function (empty `type_params`) emits no generic clause, so its
/// output is byte-identical to the M0 / M1 golden `main_update` / `sky_main`. A
/// fully-parametric function quantifying `[a, b]` emits `pub fn name<T1, T2>(..)`
/// and renders every [`IrType::Generic`] in its signature / body through the
/// matching scope (M2a). A variable carrying a [`BoundSet`] gains its
/// `: <bounds>` clause at its position (M2d). The body is an expression rendered
/// at indentation level 1; the closing brace sits at column 0.
pub fn emit_func(ctx: &EmitCtx, func: &Func) -> DResult<String> {
    let name = ctx.func_name(func.id)?.to_owned();
    // The generic scope resolves an `IrType::Generic` to its positional Rust
    // name; only the variable symbols participate, so project them out of the
    // `(Symbol, BoundSet)` pairs.
    let scope_syms: Vec<Symbol> = func.type_params.iter().map(|(sym, _)| *sym).collect();
    let generics = GenericScope::new(&scope_syms);

    // The generic clause `<T1, T2: <bounds>, ..>` — one entry per quantified
    // variable in declaration order, the position fixing its `T{i+1}` name. An
    // unbounded variable (M2a) emits a bare `T{i+1}`, so a function with no
    // bounds matches the pre-M2d golden exactly. Empty for a monomorphic
    // function, matching the pre-M2a golden.
    let generic_clause = if func.type_params.is_empty() {
        String::new()
    } else {
        let entries = func
            .type_params
            .iter()
            .enumerate()
            .map(|(i, (_, bounds))| {
                let n = i.saturating_add(1);
                let clause = render_bounds(*bounds, n);
                format!("T{n}{clause}")
            })
            .collect::<Vec<_>>()
            .join(", ");
        format!("<{entries}>")
    };

    let mut params = Vec::with_capacity(func.params.len());
    for (param, ty) in &func.params {
        params.push(format!(
            "{}: {}",
            ctx.emit_ident(*param)?,
            render_type(ctx, ty, generics)?
        ));
    }
    let ret = render_type(ctx, &func.ret, generics)?;

    // Phase-1a: M is inferred bottom-up from concrete element/attrs types
    // propagated by the region-type–sourced lowerer.  The old ui_msg_string /
    // with_ui_msg mechanism is removed; `generics` is used directly.
    // TCO (task #49): a `TailLoop` body emits `let mut`-shadowed params + a
    // `loop { … }` whose interior ends only in `return`/`continue`. Mutability is
    // introduced ONLY by the local `let mut p = p;` shadow, so the public `fn`
    // signature stays byte-identical to the non-TCO form (load-bearing for
    // `FuncValue` boxing / trait-object slots). The loop types as `!` (it never
    // falls through), so it unifies with any `-> R` — no `break value`. A
    // non-`TailLoop` body (the common case) routes to the ordinary value emitter,
    // which is exhaustive and fail-closed for any stray TCO node.
    let body = match &func.body {
        Expr::TailLoop {
            params: loop_params,
            body: loop_body,
        } => {
            let mut shadows = String::new();
            for (param, _ty) in loop_params {
                let p = ctx.emit_ident(*param)?;
                write!(shadows, "let mut {p} = {p};\n    ").map_err(|e| {
                    Diagnostic::CompilerBug {
                        where_: "sky_backend_rust::emit_func",
                        detail: format!("writing TCO param shadow failed: {e}"),
                    }
                })?;
            }
            let inner = emit_expr_tail(ctx, loop_body, 2, 1, generics, loop_params)?;
            format!("{shadows}loop {{\n{inner}\n    }}")
        }
        _ => emit_expr(ctx, &func.body, 1, generics)?,
    };
    Ok(format!(
        "pub fn {name}{generic_clause}({}) -> {ret} {{\n    {body}\n}}\n",
        params.join(", ")
    ))
}
