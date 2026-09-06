use super::*;
use core::fmt::Write as _;

/// Field names of the `HttpRequest` runtime struct, sorted alphabetically.
/// Used by [`emit_record`] as a FALLBACK to detect `HttpRequest` literals and
/// bypass the synthesised-struct lookup (the type is defined in
/// `ipe_runtime::http_client`, not emitted by the backend) — consulted only
/// when [`EmitCtx::has_record_struct_for`] finds no registered struct for the
/// literal's field-name set. See that method's doc comment for why the two
/// checks must run in THIS order (registry first, name-only fallback
/// second): `ipe_backend_rust` has no access to `ipe_lower`'s `Ty` /
/// `canon::Type` (no cross-crate dependency), so it cannot re-run the
/// lowerer's now-TYPE-AWARE `HttpRequest`-shape test
/// (`ipe_lower::lower::is_http_request_shape`) directly here — deferring to
/// the registry is how this call site stays in sync with that test without
/// duplicating it.
pub(crate) const HTTP_REQUEST_FIELDS: &[&str] = &["body", "headers", "method", "redirects", "timeout", "url"];

/// the sorted `Ipe.Process.runWith` input record field-name set — a record
/// literal with exactly these names (and no registered synthesised struct,
/// because the lowerer folded the shape to `IrType::ProcessRunWithCfg`)
/// constructs the runtime `ipe_runtime::system::ProcessRunWithCfg` struct.
/// Mirrors [`CACHE_CFG_FIELDS`]; kept in sync with
/// `ipe_lower::lower::PROCESS_RUN_WITH_CFG_FIELDS`.
pub(crate) const PROCESS_RUN_WITH_CFG_FIELDS: &[&str] = &["args", "command", "cwd", "env"];

/// the sorted `Ipe.Process.runInPty` config field-name set — a record literal
/// with exactly these names (and no registered synthesised struct, because the
/// lowerer folded the shape to `IrType::ProcessRunInPtyCfg`) constructs the
/// runtime `ipe_runtime::system::ProcessRunInPtyCfg` struct. Kept in sync with
/// `ipe_lower::lower::PROCESS_RUN_IN_PTY_CFG_FIELDS`.
pub(crate) const PROCESS_RUN_IN_PTY_CFG_FIELDS: &[&str] = &["args", "cols", "command", "cwd", "env", "rows"];

/// the sorted `Ipe.Cache.CacheCfg` field-name set — a record literal with
/// exactly these names (and no registered synthesised struct, because the
/// lowerer folded the shape to `IrType::CacheCfg`) constructs the runtime
/// `ipe_runtime::cache::CacheCfg` struct. Mirrors [`HTTP_REQUEST_FIELDS`]; kept
/// in sync with `ipe_lower::lower::CACHE_CFG_FIELDS`.
pub(crate) const CACHE_CFG_FIELDS: &[&str] = &["maxBytes", "maxEntries", "ttlMs"];

/// the sorted `Ipe.Csv.Csv` field-name set — a record literal with exactly
/// these names (and no registered synthesised struct, because the lowerer
/// folded the shape to `IrType::CsvDoc`) constructs the runtime
/// `ipe_runtime::csv::CsvDoc` struct. Mirrors [`CACHE_CFG_FIELDS`]; kept in
/// sync with `ipe_lower::lower::CSV_DOC_FIELDS`.
pub(crate) const CSV_DOC_FIELDS: &[&str] = &["header", "rows"];

/// the sorted `Ipe.WebSocket.WebSocketCfg` field-name set — a record
/// literal with exactly these names (and no registered synthesised struct,
/// because the lowerer folded the shape to `IrType::WebSocketClientCfg`)
/// constructs the runtime `ipe_runtime::ws_client::WsClientCfg` struct. Mirrors
/// [`CACHE_CFG_FIELDS`]; kept in sync with
/// `ipe_lower::lower::WEBSOCKET_CFG_FIELD_TYPES`.
pub(crate) const WEBSOCKET_CFG_FIELDS: &[&str] = &["headers", "pingInterval", "timeout", "url"];

/// the sorted `Ipe.Http.Server.Response` field-name set. A record literal
/// with exactly these names (and no registered synthesised struct, because the
/// lowerer folded the shape to `IrType::ServerResponse`) constructs the runtime
/// `ipe_runtime::server::ServerResponse` struct. That struct carries one EXTRA
/// runtime-only field, `cookies: Vec<String>` (multi-`Set-Cookie` support),
/// which the Ipê record alias does not expose — so the literal must default it
/// to `Vec::new()`. Kept in sync with `ipe_lower::lower::SERVER_RESPONSE_FIELD_TYPES`.
pub(crate) const SERVER_RESPONSE_FIELDS: &[&str] = &["body", "contentType", "headers", "status"];

/// the sorted `Ipe.Email` record field-name sets. A record literal with exactly
/// one of these name-sets (and no registered synthesised struct, because the
/// lowerer folded the shape to the matching `IrType::Email*`) constructs the
/// runtime struct (re-exported bare via `pub use email::*`). Mirror of the
/// `CsvDoc` fall-through; kept in sync with `ipe_lower::lower::EMAIL_*_FIELDS`.
/// The four name-sets are mutually distinct, so the name-only match is exact
/// (soundness note: a genuine `Ipe.Email` literal never gets a registered
/// struct because the lowerer intercepts it into the `IrType::Email*` fold
/// first — the same rationale as `CsvDoc`).
pub(crate) const EMAIL_MESSAGE_FIELDS: &[&str] = &[
    "attachments",
    "bcc",
    "cc",
    "from",
    "htmlBody",
    "replyTo",
    "subject",
    "textBody",
    "to",
];
pub(crate) const EMAIL_ATTACHMENT_FIELDS: &[&str] = &["content", "filename", "mimeType"];
pub(crate) const EMAIL_SES_FIELDS: &[&str] = &["key", "region", "secret"];
pub(crate) const EMAIL_SMTP_FIELDS: &[&str] = &["host", "pass", "port", "user"];

/// Emit a record literal `{ x = e1, ... }` as a named struct literal
/// `RecXY { x: <e1>, ... }`. `depth` is the literal's own IR-nesting level; its
/// field values are emitted one level deeper. Kept out of the `emit_expr_at`
/// match (`#[inline(never)]`) so its locals don't inflate the recursive frame.
#[inline(never)]
pub(crate) fn emit_record(
    ctx: &EmitCtx,
    fields: &[(Symbol, Expr)],
    ty: Option<&IrType>,
    indent: usize,
    depth: u16,
    generics: GenericScope,
) -> DResult<String> {
    let child = depth + 1;
    let (struct_name, is_server_response) = record_struct_name(ctx, fields, ty)?;
    let mut parts = Vec::with_capacity(fields.len() + usize::from(is_server_response));
    for (sym, value) in fields {
        let field_ident = ctx.emit_ident(*sym)?;
        let rendered = emit_expr_at(ctx, value, indent, child, generics)?;
        parts.push(format!("{field_ident}: {rendered}"));
    }
    if is_server_response {
        // The runtime struct's multi-`Set-Cookie` field is not part of the Ipê
        // record alias; default it so the struct literal is complete.
        parts.push("cookies: Vec::new()".to_owned());
    }
    Ok(format!("{struct_name} {{ {} }}", parts.join(", ")))
}

/// Resolve the Rust struct name a record literal constructs. The literal's
/// field-name set (Rust names struct-literal fields, so field write order is
/// free) picks the candidate struct(s); when the set is shared by two distinct
/// shapes, the literal's solved `ty` (an [`IrType::Record`], threaded from the
/// lowerer) disambiguates to the exact one. Returns the struct name and whether
/// it folds to the runtime `ServerResponse` struct (which carries an extra
/// `cookies: Vec<String>` field the Ipê record alias omits, so the caller
/// appends a `cookies: Vec::new()` field). Shared by [`emit_record`] and the
/// native Doc emitter so the two agree on the struct name exactly.
pub fn record_struct_name(
    ctx: &EmitCtx,
    fields: &[(Symbol, Expr)],
    ty: Option<&IrType>,
) -> DResult<(String, bool)> {
    // The struct is resolved by the literal's field-name set (Rust names
    // struct-literal fields, so write order is free); the field idents are
    // keyword-mangled to match the struct definition.
    let mut key = Vec::with_capacity(fields.len());
    for (sym, _) in fields {
        key.push(ctx.resolve_ident(*sym)?.to_owned());
    }
    // `true` when the shape folds to the runtime `ServerResponse` struct, which
    // carries an extra `cookies: Vec<String>` field the Ipê record alias omits.
    let mut is_server_response = false;
    let struct_name: String = {
        // Prefer an actual synthesised struct when one is registered for
        // this exact field-name set — that reflects `ipe_lower`'s
        // authoritative, TYPE-AWARE decision (see
        // `EmitCtx::has_record_struct_for`'s doc comment). Only fall back to
        // the field-NAME-only `HttpRequest` heuristic when NO struct is
        // registered, which is precisely the signature of a genuine
        // `HttpRequest` literal (the lowerer intercepts it into the opaque
        // `IrType::HttpRequest` before it ever reaches the struct registry).
        // This ordering closes the false-positive class where an unrelated
        // record sharing the 7 canonical field NAMES with unrelated field
        // TYPES (e.g. all-`Int`) would be mislabelled `HttpRequest` here
        // even after `ipe_lower` had already registered a correctly-typed
        // struct for it — a two-path divergence the registry check avoids.
        if ctx.has_record_struct_for(&key) {
            ctx.record_name_for_literal(&key, ty)?.to_owned()
        } else {
            let mut sorted = key.clone();
            sorted.sort();
            let is_http_request = sorted.len() == HTTP_REQUEST_FIELDS.len()
                && sorted
                    .iter()
                    .zip(HTTP_REQUEST_FIELDS.iter())
                    .all(|(a, b)| a.as_str() == *b);
            // same fall-through as HttpRequest — a `ProcessRunWithCfg`-shaped
            // literal has no registered struct (folded to
            // `IrType::ProcessRunWithCfg`), so it constructs the runtime
            // `ProcessRunWithCfg` (re-exported bare via the glob).
            let is_process_run_with_cfg = sorted.len() == PROCESS_RUN_WITH_CFG_FIELDS.len()
                && sorted
                    .iter()
                    .zip(PROCESS_RUN_WITH_CFG_FIELDS.iter())
                    .all(|(a, b)| a.as_str() == *b);
            // same fall-through as `ProcessRunWithCfg` — a `ProcessRunInPtyCfg`-shaped
            // literal has no registered struct (folded to
            // `IrType::ProcessRunInPtyCfg`), so it constructs the runtime
            // `ProcessRunInPtyCfg` (re-exported bare via the glob).
            let is_process_run_in_pty_cfg = sorted.len() == PROCESS_RUN_IN_PTY_CFG_FIELDS.len()
                && sorted
                    .iter()
                    .zip(PROCESS_RUN_IN_PTY_CFG_FIELDS.iter())
                    .all(|(a, b)| a.as_str() == *b);
            // same fall-through as HttpRequest — a `CacheCfg`-shaped literal
            // has no registered struct (folded to `IrType::CacheCfg`), so it
            // constructs the runtime `CacheCfg` (re-exported bare via the glob).
            let is_cache_cfg = sorted.len() == CACHE_CFG_FIELDS.len()
                && sorted
                    .iter()
                    .zip(CACHE_CFG_FIELDS.iter())
                    .all(|(a, b)| a.as_str() == *b);
            // same fall-through — a `Csv`-shaped literal has no registered
            // struct (folded to `IrType::CsvDoc`), so it constructs the runtime
            // `CsvDoc` (re-exported bare via the `pub use csv::*` glob).
            let is_csv_doc = sorted.len() == CSV_DOC_FIELDS.len()
                && sorted
                    .iter()
                    .zip(CSV_DOC_FIELDS.iter())
                    .all(|(a, b)| a.as_str() == *b);
            // same fall-through — a `WebSocketCfg`-shaped literal has no
            // registered struct (folded to `IrType::WebSocketClientCfg`), so it
            // constructs the runtime `WsClientCfg` (re-exported bare via the
            // `pub use ws_client::*` glob).
            let is_websocket_cfg = sorted.len() == WEBSOCKET_CFG_FIELDS.len()
                && sorted
                    .iter()
                    .zip(WEBSOCKET_CFG_FIELDS.iter())
                    .all(|(a, b)| a.as_str() == *b);
            // same fall-through — a `Response`-shaped literal has no
            // registered struct (folded to `IrType::ServerResponse`), so it
            // constructs the runtime `ServerResponse` (re-exported bare via the
            // `pub use server::*` glob).
            is_server_response = sorted.len() == SERVER_RESPONSE_FIELDS.len()
                && sorted
                    .iter()
                    .zip(SERVER_RESPONSE_FIELDS.iter())
                    .all(|(a, b)| a.as_str() == *b);
            // Ipe.Email fall-throughs — same rationale as `CsvDoc`: a
            // `defaultMessage`/`defaultAttachment`/… built literal has no
            // registered struct (folded to the matching `IrType::Email*`), so it
            // constructs the runtime struct (re-exported bare via `pub use
            // email::*`). The Ipê `Attachment` alias maps to `EmailAttachment`.
            let name_set_is = |expected: &[&str]| {
                sorted.len() == expected.len()
                    && sorted
                        .iter()
                        .zip(expected.iter())
                        .all(|(a, b)| a.as_str() == *b)
            };
            if is_http_request {
                "HttpRequest".to_owned()
            } else if is_process_run_with_cfg {
                "ProcessRunWithCfg".to_owned()
            } else if is_process_run_in_pty_cfg {
                "ProcessRunInPtyCfg".to_owned()
            } else if is_cache_cfg {
                "CacheCfg".to_owned()
            } else if is_csv_doc {
                "CsvDoc".to_owned()
            } else if is_websocket_cfg {
                "WsClientCfg".to_owned()
            } else if is_server_response {
                "ServerResponse".to_owned()
            } else if name_set_is(EMAIL_MESSAGE_FIELDS) {
                "EmailMessage".to_owned()
            } else if name_set_is(EMAIL_ATTACHMENT_FIELDS) {
                "EmailAttachment".to_owned()
            } else if name_set_is(EMAIL_SES_FIELDS) {
                "SesConfig".to_owned()
            } else if name_set_is(EMAIL_SMTP_FIELDS) {
                "SmtpConfig".to_owned()
            } else {
                ctx.record_name_for_literal(&key, ty)?.to_owned()
            }
        }
    };
    Ok((struct_name, is_server_response))
}

/// Emit a functional record update `{ record | f = v, ... }` as a
/// bind-fields-then-move-and-reassign block:
/// `{ let __ipe_upd_0 = v0; …; let mut __ipe_rec = <base>; __ipe_rec.f = __ipe_upd_0; …; __ipe_rec }`.
///
/// Each field value is bound to a positional temporary BEFORE the base is moved
/// into `__ipe_rec`. This lets a field value read the base itself — the
/// canonical functional-update idiom `{ record | count = record.count + 1 }` —
/// on a non-`Clone` base: the read happens while the base is still owned, and
/// the move follows. Evaluating the field values into `let` bindings in source
/// order runs each value expression exactly once, in order, so a side-effecting
/// value is not duplicated or reordered.
///
/// The base expression is emitted by [`emit_expr_at`], which already inserts
/// `.clone()` when the base variable appears in multiple positions — the reuse
/// gate rewrites such variables to [`Expr::CloneVar`] before emission. No extra
/// `.clone()` is added here:
///
/// * If the base is a bare [`Expr::Var`] (single use), moving it into
///   `__ipe_rec` is correct for both `Clone`-able and non-`Clone` record types.
///   A non-`Clone` effect-carrier (`Task`/`Cmd`/`Sub`-bearing record) can be
///   moved but not cloned; a single-use `Clone`-able record is equally well
///   moved.
/// * If the base is a [`Expr::CloneVar`] (multi-use), `emit_expr_at` emits
///   `base.clone()`, and the assignment binds that single clone.
///
/// A base reused OUTSIDE the update (a later borrow or move of a non-`Clone`
/// base) has no sound rewrite and is rejected fail-closed at lower time
/// (`IPE-L0135`); it never reaches this emitter.
///
/// Kept `#[inline(never)]` for the same frame-size reason as [`emit_record`].
#[inline(never)]
pub(crate) fn emit_update(
    ctx: &EmitCtx,
    record: &Expr,
    fields: &[(Symbol, Expr)],
    indent: usize,
    depth: u16,
    generics: GenericScope,
) -> DResult<String> {
    let child = depth + 1;

    // G2 (update-through-row): when the base record is a row-generic parameter
    // (type `R{n}`, a rustc generic bound by `IpeHasF + IpeWithF`), direct
    // field-mutation is unsound — rustc does not know the concrete struct layout
    // behind `R{n}`. Emit a chain of setter-witness calls instead:
    //   `rec.ipe_with_f1(v1).ipe_with_f2(v2)`
    // Each setter consumes `self` and returns `Self`, so the chain preserves
    // all untouched fields through the `..self` impl body without naming the
    // concrete struct. The base record is moved into the first call and the
    // chain returns `R{n}` — exactly the return type required by G1.
    let record_sym = match record {
        Expr::Var(s) | Expr::CloneVar(s) => Some(*s),
        _ => None,
    };
    if let Some(sym) = record_sym
        && generics.is_row(sym)
    {
        // Evaluate each new field value as a binding first so evaluation
        // order is left-to-right and matches the concrete-struct path.
        let mut binds = Vec::with_capacity(fields.len());
        let mut chain = emit_expr_at(ctx, record, indent, child, generics)?;
        for (i, (field_sym, value)) in fields.iter().enumerate() {
            let field_name = ctx.resolve_ident(*field_sym)?;
            let setter = crate::naming::field_setter_witness_method_name(field_name);
            let rendered = emit_expr_at(ctx, value, indent, child, generics)?;
            binds.push(format!(" let __ipe_upd_{i} = {rendered};"));
            chain = format!("{chain}.{setter}(__ipe_upd_{i})");
        }
        return Ok(format!("{{{} {chain} }}", binds.concat()));
    }

    // Concrete struct path: the record type is a known struct, so direct field
    // mutation via a `let mut __ipe_rec` shadow is sound.
    let mut binds = Vec::with_capacity(fields.len());
    let mut assigns = Vec::with_capacity(fields.len());
    for (i, (sym, value)) in fields.iter().enumerate() {
        let field_ident = ctx.emit_ident(*sym)?;
        let rendered = emit_expr_at(ctx, value, indent, child, generics)?;
        binds.push(format!(" let __ipe_upd_{i} = {rendered};"));
        assigns.push(format!(" __ipe_rec.{field_ident} = __ipe_upd_{i};"));
    }
    let base = emit_expr_at(ctx, record, indent, child, generics)?;
    Ok(format!(
        "{{{} let mut __ipe_rec = {base};{} __ipe_rec }}",
        binds.concat(),
        assigns.concat()
    ))
}

/// Lay a match-arm rebind `prelude` out one statement per line at `indent`.
///
/// The prelude is a run of `let …; ` binder-rebind statements the clone-split
/// helpers build joined by `"; "`; `rustfmt` puts each on its own line. Split on
/// the separator, re-indent each, and return the block (with its trailing
/// newline) — a trailing empty segment is skipped.
pub(crate) fn tail_arm_prelude_lines(prelude: &str, indent: usize) -> DResult<String> {
    let pad = indent_of(indent);
    let mut out = String::new();
    for stmt in prelude.split_inclusive("; ") {
        let stmt = stmt.trim_end();
        if stmt.is_empty() {
            continue;
        }
        writeln!(out, "{pad}{stmt}").map_err(|e| Diagnostic::CompilerBug {
            where_: "ipe_backend_rust::tail_arm_prelude_lines",
            detail: format!("writing TCO arm prelude failed: {e}"),
        })?;
    }
    Ok(out)
}
