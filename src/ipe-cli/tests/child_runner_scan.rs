#![forbid(unsafe_code)]
//! Refuses every production child process in the `ipe` crate started outside its named runner.
//!
//! The scan reads the crate as a syntax tree. It walks the module graph from
//! the crate roots, following each `mod x;` to the one file it names, so every
//! `.rs` file under `src/` is classified as production or test code. A file no
//! declaration reaches, a `#[path]` module and an `include!` are refused, since
//! the scan could not say which code they hold. An item gated by `#[test]`,
//! `cfg(test)` or `cfg(all(.., test, ..))` is test code; `any(..)` and `not(..)`
//! never are.
//!
//! In production code it records, per file and enclosing function:
//!
//! - every `Command::new` site and the expression naming its program, held to
//!   [`SITES`]: each row names the runner that starts the child and is proved
//!   against the function's own calls (a `run_local` passing the row's ceiling,
//!   a `run_inherited` passing its role, and so on);
//! - every child start (`spawn`, `output`, `status`, `exec`, `wait_with_output`
//!   with no argument, the same names through `Command`, `CommandExt` or
//!   `Child`, and the runtime's `spawn_hardened`, `spawn_detached` and
//!   `exec_naming`), held to [`RUNNER_BODIES`] and the reviewed non-child calls
//!   in [`ADMITTED`].
//!
//! A macro body is parsed as a comma-separated expression list and scanned like
//! any other code; a body that does not parse is refused when its text holds a
//! child start. A rename of `Command`, `CommandExt`, `Child` or a runner, and a
//! type alias of the first three, are refused anywhere, since a site would then
//! escape the names above. An unbounded `ureq` body read (`into_json`,
//! `into_string`, or an `into_reader` not handed straight to `read_capped`) is
//! refused outside `remote_ingest`.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use syn::punctuated::Punctuated;
use syn::visit::{self, Visit};
use syn::{
    Attribute, Expr, ExprBlock, ExprCall, ExprMethodCall, ExprPath, FnArg, ImplItemConst,
    ImplItemFn, ItemConst, ItemFn, ItemImpl, ItemMacro, ItemMod, ItemStatic, ItemType, Lit, Local,
    Macro, Member, Meta, Pat, ReturnType, Signature, Token, TraitItemFn, Type, UseRename,
};

/// The crate roots the module walk starts from, relative to `src/`.
const ROOTS: &[&str] = &["lib.rs", "main.rs", "bin/gen_cli_docs.rs"];

/// The module holding the remote runners and every `LocalCeiling` constant.
const RUNNER_MODULE: &str = "remote_ingest.rs";

/// The module holding the cargo build runner.
const CARGO_MODULE: &str = "cargo_step.rs";

/// The type a local child's ceiling constant must have.
const CEILING_TYPE: &str = "LocalCeiling";

/// The enclosing-function name of code outside any function.
const ITEM_SCOPE: &str = "<item>";

/// Methods that start or wait on a child when called with no argument.
const ZERO_ARG_STARTS: &[&str] = &["spawn", "output", "status", "exec", "wait_with_output"];

/// The types a [`ZERO_ARG_STARTS`] name starts a child through when called by path.
const PROCESS_TYPES: &[&str] = &["Command", "CommandExt", "Child"];

/// The runtime's hardened starts, recognised by the last segment of a called path.
const PATH_STARTS: &[&str] = &[
    "spawn_hardened",
    "spawn_hardened_naming",
    "spawn_hardened_tokio",
    "spawn_detached",
    "exec_naming",
];

/// Names a `use .. as ..` must not rename: the process types and every runner.
const GUARDED_NAMES: &[&str] = &[
    "Command",
    "CommandExt",
    "Child",
    "run_local",
    "run_local_fed",
    "run_inherited",
    "run_probe",
    "spawn_hardened",
    "spawn_hardened_naming",
    "spawn_hardened_tokio",
    "spawn_detached",
    "exec_naming",
    "open_url",
];

/// The runner a child site hands its `Command` to.
#[derive(Clone, Copy, Debug)]
enum Runner {
    /// `run_local` under the named `LocalCeiling` constant.
    Local(&'static str),
    /// `run_local_fed` under the named `LocalCeiling` constant.
    LocalFed(&'static str),
    /// `run_probe`, the bounded signal-disposition read.
    Probe,
    /// A `Git` or `Curl` transfer built inside `remote_ingest`.
    Transfer,
    /// The `cargo build` child built inside `cargo_step`.
    Cargo,
    /// `run_inherited` under the named `InheritedRole`.
    Inherited(&'static str),
    /// `exec_naming`, which replaces the CLI with the program.
    Replace,
    /// The browser opener's `open_with`.
    Opener,
    /// The `ipe watch` dev app, built for `spawn_command`.
    DevApp,
}

/// A child site: file under `src/`, enclosing function, program expression, runner, count.
type Site = (&'static str, &'static str, &'static str, Runner, usize);

/// A child start: file under `src/`, enclosing function, start name, count.
type Start = (&'static str, &'static str, &'static str, usize);

/// Every reviewed production `Command::new` site.
///
/// None runs a remote-transfer tool outside `remote_ingest`. A literal names a
/// local tool; a variable names a program the CLI resolved itself (its own
/// binary, the toolchain's `cargo`, a wasm tool, the FFI inspector payload, a
/// `health` install `argv`, a platform opener) or a local tool's name its caller
/// passes so a test can stub it (`cargo_deny`, `rustup`, `rustc`).
const SITES: &[Site] = &[
    (
        "audit.rs",
        "detect_cargo_deny_minor",
        "cargo_deny",
        Runner::Local("TOOL_QUERY_LIMITS"),
        1,
    ),
    (
        "audit.rs",
        "run_cargo_deny",
        "cargo_deny",
        Runner::Local("SUPPLY_CHAIN_SCAN_LIMITS"),
        1,
    ),
    ("browser.rs", "open_with", "program", Runner::Opener, 1),
    (
        "build_plan.rs",
        "installed_targets_of",
        "rustup",
        Runner::Local("TOOL_QUERY_LIMITS"),
        1,
    ),
    (
        "cargo_step.rs",
        "target_directory_within",
        "cargo.path()",
        Runner::Local("METADATA_LIMITS"),
        1,
    ),
    ("cargo_step.rs", "build_command", "cargo", Runner::Cargo, 1),
    (
        "cargo_step.rs",
        "lock_dependencies_within",
        "build.get_program()",
        Runner::Local("LOCK_FETCH_LIMITS"),
        1,
    ),
    (
        "cargo_step.rs",
        "lock_offline_within",
        "cargo",
        Runner::Local("LOCK_RESOLVE_LIMITS"),
        1,
    ),
    (
        "coverage/probe.rs",
        "build_and_run",
        "&ipe_bin",
        Runner::Local("SELF_RUN_LIMITS"),
        1,
    ),
    (
        "doc.rs",
        "run_example_and_check",
        "&ipe_bin",
        Runner::Local("SELF_RUN_LIMITS"),
        1,
    ),
    (
        "driver/commands.rs",
        "bundle_wasm_pkg",
        "tools.bindgen",
        Runner::Local("WASM_TOOL_LIMITS"),
        1,
    ),
    (
        "driver/commands.rs",
        "bundle_wasm_pkg",
        "tools.opt",
        Runner::Local("WASM_TOOL_LIMITS"),
        1,
    ),
    (
        "driver/commands.rs",
        "run_run_with_args",
        "&bin",
        Runner::Replace,
        1,
    ),
    (
        "driver/commands.rs",
        "run_run_with_args",
        "&bin",
        Runner::Inherited("UserProgram"),
        1,
    ),
    ("driver/commands.rs", "run_exec", "&bin", Runner::Replace, 1),
    (
        "driver/commands.rs",
        "run_exec",
        "&bin",
        Runner::Inherited("UserProgram"),
        1,
    ),
    (
        "driver/commands_pkg.rs",
        "build_wasm_for_mobile",
        "&exe",
        Runner::Inherited("SelfBuild"),
        1,
    ),
    (
        "driver/commands_pkg.rs",
        "run_test_binary",
        "bin",
        Runner::Inherited("UserProgram"),
        1,
    ),
    (
        "driver/commands_pkg.rs",
        "run_installer",
        "\"sh\"",
        Runner::Inherited("InteractiveInstall"),
        1,
    ),
    (
        "ffi.rs",
        "run_inspector_job_unsandboxed",
        "program",
        Runner::Local("FFI_INSPECT_LIMITS"),
        1,
    ),
    (
        "health.rs",
        "run_link_probe_within",
        "rustc",
        Runner::LocalFed("LINK_PROBE_LIMITS"),
        1,
    ),
    (
        "health.rs",
        "run_install",
        "program",
        Runner::Inherited("InteractiveInstall"),
        1,
    ),
    ("remote_ingest.rs", "base", "\"git\"", Runner::Transfer, 1),
    ("remote_ingest.rs", "https", "\"curl\"", Runner::Transfer, 1),
    ("terminate.rs", "ps_masks", "\"/bin/ps\"", Runner::Probe, 1),
    (
        "toolchain.rs",
        "active",
        "\"rustc\"",
        Runner::Local("RUSTC_QUERY_LIMITS"),
        1,
    ),
    ("watch.rs", "child_command", "exe_path", Runner::DevApp, 1),
];

/// The runner bodies: the only production functions that start a child.
const RUNNER_BODIES: &[Start] = &[
    ("browser.rs", "open_with", "spawn_hardened", 1),
    ("cargo_step.rs", "run_to_exit", "spawn_hardened", 1),
    ("cargo_step.rs", "spawn", "spawn_hardened", 1),
    ("driver/commands.rs", "run_exec", "exec_naming", 1),
    ("driver/commands.rs", "run_run_with_args", "exec_naming", 1),
    ("remote_ingest.rs", "run_inherited", "spawn_hardened", 1),
    ("remote_ingest.rs", "spawn", "spawn_detached", 1),
    ("remote_ingest.rs", "spawn", "spawn_hardened", 1),
    ("remote_ingest.rs", "spawn_attached", "spawn_hardened", 2),
    ("remote_ingest.rs", "spawn_detached", "spawn_hardened", 2),
    ("watch.rs", "spawn_command", "spawn_hardened", 1),
];

/// Reviewed production calls spelled like a child start that start none: an
/// HTTP response's `status()`, and the cargo runner's own `WatchBuild::spawn`.
const ADMITTED: &[Start] = &[
    ("ssh_signing_key.rs", "post_signing_key", "status", 2),
    ("watch.rs", "spawn_cargo_build", "spawn", 1),
];

/// Directory nesting the source walk descends before refusing to go deeper.
const MAX_DEPTH: usize = 16;

/// Unbounded `ureq` body reads, whitespace removed.
const UNBOUNDED_BODY_READS: &[&str] = &[".into_json(", ".into_string()"];

/// The one bounded body read: a response reader handed straight to `read_capped`.
const READER: &str = ".into_reader()";

/// The only call an `into_reader` may appear as the first argument of.
const CAPPED_CALL: &str = "read_capped(";

/// A finding's key: file under `src/`, enclosing function, and what was found.
type Key = (String, String, String);

/// The path segments of each argument that is a plain path, `None` for any other.
type Args = Vec<Option<Vec<String>>>;

/// A production call: where it is, what it calls, and its path arguments.
struct Call {
    /// File under `src/`.
    file: String,
    /// The enclosing function.
    caller: String,
    /// The called path's last segment, or the method name.
    callee: String,
    /// The arguments, receiver excluded.
    args: Args,
}

/// What a production function's signature says.
#[derive(Default)]
struct FnFacts {
    /// Whether it returns a `Command`.
    returns_command: bool,
    /// Each `LocalCeiling` parameter's name and argument index.
    ceiling_params: BTreeMap<String, usize>,
}

/// Everything the scan saw in production code, and every refusal it made.
#[derive(Default)]
struct Findings {
    /// `Command::new` sites by program expression.
    sites: BTreeMap<Key, usize>,
    /// Child starts by name.
    starts: BTreeMap<Key, usize>,
    /// Signature facts by file and function name.
    fns: BTreeMap<(String, String), FnFacts>,
    /// Every production call.
    calls: Vec<Call>,
    /// The `LocalCeiling` constants of [`RUNNER_MODULE`].
    ceilings: BTreeSet<String>,
    /// Constructs refused outright.
    refusals: Vec<String>,
}

/// A `mod x;` waiting to be resolved to its file.
struct Pending {
    /// The declaring file.
    parent: String,
    /// The declared module's directory and stem, relative to `src/`.
    stem: String,
    /// Whether the declaration is test-only.
    test: bool,
}

/// The visitor over one file.
struct Scanner<'f> {
    /// File under `src/`.
    file: String,
    /// The current module's directory, relative to `src/`.
    dir: String,
    /// Whether the current node is test-only.
    test: bool,
    /// The enclosing functions, innermost last.
    fns: Vec<String>,
    /// Where findings go.
    found: &'f mut Findings,
    /// The `mod x;` declarations seen.
    pending: Vec<Pending>,
}

/// `name` inside `dir`, both relative to `src/`.
fn join(dir: &str, name: &str) -> String {
    if dir.is_empty() {
        name.to_owned()
    } else {
        format!("{dir}/{name}")
    }
}

/// The directory a crate root's `mod x;` resolves against.
fn root_dir(root: &str) -> String {
    root.rsplit_once('/')
        .map_or_else(String::new, |(dir, _)| dir.to_owned())
}

/// A path's segments as text.
fn segments(path: &syn::Path) -> Vec<String> {
    path.segments
        .iter()
        .map(|seg| seg.ident.to_string())
        .collect()
}

/// The last segment of a type's path, through references.
fn type_last(ty: &Type) -> Option<String> {
    match ty {
        Type::Path(path) => path.path.segments.last().map(|seg| seg.ident.to_string()),
        Type::Reference(reference) => type_last(&reference.elem),
        _ => None,
    }
}

/// Whether `meta`, a `cfg` predicate, holds only under `test`.
fn cfg_test_only(meta: &Meta) -> bool {
    match meta {
        Meta::Path(path) => path.is_ident("test"),
        Meta::List(list) if list.path.is_ident("all") => list
            .parse_args_with(Punctuated::<Meta, Token![,]>::parse_terminated)
            .is_ok_and(|operands| operands.iter().any(cfg_test_only)),
        Meta::List(_) | Meta::NameValue(_) => false,
    }
}

/// Whether `attrs` make their node test-only; `is_fn` admits a bare `#[test]`.
fn attrs_test_only(attrs: &[Attribute], is_fn: bool) -> bool {
    attrs.iter().any(|attr| match &attr.meta {
        Meta::Path(path) => is_fn && path.is_ident("test"),
        Meta::List(list) => {
            list.path.is_ident("cfg") && list.parse_args::<Meta>().is_ok_and(|p| cfg_test_only(&p))
        }
        Meta::NameValue(_) => false,
    })
}

/// A short, whitespace-free-where-possible description of a program expression.
fn describe(expr: &Expr) -> String {
    match expr {
        Expr::Lit(lit) => match &lit.lit {
            Lit::Str(text) => format!("{:?}", text.value()),
            _ => "<literal>".to_owned(),
        },
        Expr::Path(path) => segments(&path.path).join("::"),
        Expr::Reference(reference) => format!("&{}", describe(&reference.expr)),
        Expr::Field(field) => {
            let member = match &field.member {
                Member::Named(ident) => ident.to_string(),
                Member::Unnamed(index) => index.index.to_string(),
            };
            format!("{}.{member}", describe(&field.base))
        }
        Expr::MethodCall(call) => format!(
            "{}.{}({})",
            describe(&call.receiver),
            call.method,
            describe_args(&call.args)
        ),
        Expr::Call(call) => format!("{}({})", describe(&call.func), describe_args(&call.args)),
        Expr::Paren(paren) => format!("({})", describe(&paren.expr)),
        _ => "<expr>".to_owned(),
    }
}

/// Each argument described, joined by `, `.
fn describe_args(args: &Punctuated<Expr, Token![,]>) -> String {
    args.iter().map(describe).collect::<Vec<_>>().join(", ")
}

/// Each argument's path segments when it is a plain path.
fn path_args(args: &Punctuated<Expr, Token![,]>) -> Args {
    args.iter()
        .map(|arg| match arg {
            Expr::Path(path) => Some(segments(&path.path)),
            _ => None,
        })
        .collect()
}

/// Whether `segs` ends in `Command::new`.
fn is_command_new(segs: &[String]) -> bool {
    matches!(segs, [.., ty, func] if ty == "Command" && func == "new")
}

/// The child start `segs` names as a path, if any.
fn path_start(segs: &[String]) -> Option<&str> {
    match segs {
        [.., ty, func]
            if PROCESS_TYPES.contains(&ty.as_str()) && ZERO_ARG_STARTS.contains(&func.as_str()) =>
        {
            Some(func.as_str())
        }
        [.., func] if PATH_STARTS.contains(&func.as_str()) => Some(func.as_str()),
        _ => None,
    }
}

/// `source` with every whitespace character removed.
fn flatten(source: &str) -> String {
    source.chars().filter(|c| !c.is_whitespace()).collect()
}

/// The child starts spelled in flattened macro text.
fn text_starts(flat: &str) -> Vec<String> {
    let methods = ZERO_ARG_STARTS
        .iter()
        .map(|name| format!(".{name}()"))
        .filter(|needle| flat.contains(needle.as_str()));
    let paths = PATH_STARTS
        .iter()
        .chain(["Command::new"].iter())
        .filter(|needle| flat.contains(**needle))
        .map(|needle| (*needle).to_owned());
    methods.chain(paths).collect()
}

impl Scanner<'_> {
    /// The current file and enclosing function.
    fn here(&self) -> (String, String) {
        let func = self
            .fns
            .last()
            .cloned()
            .unwrap_or_else(|| ITEM_SCOPE.to_owned());
        (self.file.clone(), func)
    }

    /// Records a `Command::new` site in production code.
    fn site(&mut self, program: String) {
        if !self.test {
            let (file, func) = self.here();
            let seen = self.found.sites.entry((file, func, program)).or_default();
            *seen = seen.saturating_add(1);
        }
    }

    /// Records a child start in production code.
    fn start(&mut self, name: &str) {
        if !self.test {
            let (file, func) = self.here();
            let seen = self
                .found
                .starts
                .entry((file, func, name.to_owned()))
                .or_default();
            *seen = seen.saturating_add(1);
        }
    }

    /// Records a production call.
    fn call(&mut self, callee: String, args: &Punctuated<Expr, Token![,]>) {
        if !self.test {
            let (file, caller) = self.here();
            self.found.calls.push(Call {
                file,
                caller,
                callee,
                args: path_args(args),
            });
        }
    }

    /// Refuses a construct the scan cannot classify.
    fn refuse(&mut self, why: &str) {
        let at = self.file.clone();
        self.found.refusals.push(format!("{at}: {why}"));
    }

    /// Runs `inner` with the node test-only when `attrs` say so.
    fn gated(&mut self, attrs: &[Attribute], is_fn: bool, inner: impl FnOnce(&mut Self)) {
        let outer = self.test;
        self.test = outer || attrs_test_only(attrs, is_fn);
        inner(self);
        self.test = outer;
    }

    /// Runs `inner` inside the function `sig` declares.
    fn in_fn(&mut self, sig: &Signature, inner: impl FnOnce(&mut Self)) {
        let name = sig.ident.to_string();
        if !self.test {
            let facts = self
                .found
                .fns
                .entry((self.file.clone(), name.clone()))
                .or_default();
            facts.returns_command |= matches!(
                &sig.output,
                ReturnType::Type(_, ty) if type_last(ty).as_deref() == Some("Command")
            );
            let typed = sig.inputs.iter().filter_map(|arg| match arg {
                FnArg::Typed(typed) => Some(typed),
                FnArg::Receiver(_) => None,
            });
            for (at, typed) in typed.enumerate() {
                if let Pat::Ident(param) = &*typed.pat
                    && type_last(&typed.ty).as_deref() == Some(CEILING_TYPE)
                {
                    facts.ceiling_params.insert(param.ident.to_string(), at);
                }
            }
        }
        self.fns.push(name);
        inner(self);
        self.fns.pop();
    }

    /// Scans a parsed macro body.
    fn body(&mut self, exprs: &Punctuated<Expr, Token![,]>) {
        for expr in exprs {
            self.visit_expr(expr);
        }
    }

    /// Refuses an unparsed macro body whose text holds a child start.
    fn unparsed(&mut self, text: &str) {
        let starts = text_starts(&flatten(text));
        if !self.test && !starts.is_empty() {
            let (_, func) = self.here();
            self.refuse(&format!(
                "`{func}` holds {} in a macro body the scan cannot parse",
                starts.join(", ")
            ));
        }
    }
}

impl<'ast> Visit<'ast> for Scanner<'_> {
    fn visit_item_fn(&mut self, node: &'ast ItemFn) {
        self.gated(&node.attrs, true, |s| {
            s.in_fn(&node.sig, |s| visit::visit_item_fn(s, node));
        });
    }

    fn visit_impl_item_fn(&mut self, node: &'ast ImplItemFn) {
        self.gated(&node.attrs, true, |s| {
            s.in_fn(&node.sig, |s| visit::visit_impl_item_fn(s, node));
        });
    }

    fn visit_trait_item_fn(&mut self, node: &'ast TraitItemFn) {
        self.gated(&node.attrs, true, |s| {
            s.in_fn(&node.sig, |s| visit::visit_trait_item_fn(s, node));
        });
    }

    fn visit_item_impl(&mut self, node: &'ast ItemImpl) {
        self.gated(&node.attrs, false, |s| visit::visit_item_impl(s, node));
    }

    fn visit_item_static(&mut self, node: &'ast ItemStatic) {
        self.gated(&node.attrs, false, |s| visit::visit_item_static(s, node));
    }

    fn visit_item_macro(&mut self, node: &'ast ItemMacro) {
        self.gated(&node.attrs, false, |s| visit::visit_item_macro(s, node));
    }

    fn visit_impl_item_const(&mut self, node: &'ast ImplItemConst) {
        self.gated(&node.attrs, false, |s| {
            visit::visit_impl_item_const(s, node)
        });
    }

    fn visit_local(&mut self, node: &'ast Local) {
        self.gated(&node.attrs, false, |s| visit::visit_local(s, node));
    }

    fn visit_expr_block(&mut self, node: &'ast ExprBlock) {
        self.gated(&node.attrs, false, |s| visit::visit_expr_block(s, node));
    }

    fn visit_item_const(&mut self, node: &'ast ItemConst) {
        self.gated(&node.attrs, false, |s| {
            if !s.test
                && s.file == RUNNER_MODULE
                && type_last(&node.ty).as_deref() == Some(CEILING_TYPE)
            {
                s.found.ceilings.insert(node.ident.to_string());
            }
            visit::visit_item_const(s, node);
        });
    }

    fn visit_item_mod(&mut self, node: &'ast ItemMod) {
        let test = self.test || attrs_test_only(&node.attrs, false);
        if node.attrs.iter().any(|attr| attr.path().is_ident("path")) {
            self.refuse(&format!("`#[path]` on `mod {}`", node.ident));
        }
        let stem = join(&self.dir, &node.ident.to_string());
        if node.content.is_some() {
            let outer_dir = std::mem::replace(&mut self.dir, stem);
            let outer_test = std::mem::replace(&mut self.test, test);
            visit::visit_item_mod(self, node);
            self.dir = outer_dir;
            self.test = outer_test;
        } else {
            self.pending.push(Pending {
                parent: self.file.clone(),
                stem,
                test,
            });
        }
    }

    fn visit_use_rename(&mut self, node: &'ast UseRename) {
        let name = node.ident.to_string();
        if GUARDED_NAMES.contains(&name.as_str()) && node.rename != "_" {
            self.refuse(&format!("`{name}` renamed to `{}`", node.rename));
        }
        visit::visit_use_rename(self, node);
    }

    fn visit_item_type(&mut self, node: &'ast ItemType) {
        if let Some(name) = type_last(&node.ty)
            && PROCESS_TYPES.contains(&name.as_str())
        {
            self.refuse(&format!("`type {}` aliases `{name}`", node.ident));
        }
        visit::visit_item_type(self, node);
    }

    fn visit_macro(&mut self, node: &'ast Macro) {
        if node
            .path
            .segments
            .last()
            .is_some_and(|seg| seg.ident == "include")
        {
            self.refuse("`include!` splices code the scan cannot classify");
        }
        match node.parse_body_with(Punctuated::<Expr, Token![,]>::parse_terminated) {
            Ok(exprs) => self.body(&exprs),
            Err(_) => self.unparsed(&node.tokens.to_string()),
        }
    }

    fn visit_expr_call(&mut self, node: &'ast ExprCall) {
        let Expr::Path(func) = &*node.func else {
            visit::visit_expr_call(self, node);
            return;
        };
        let segs = segments(&func.path);
        if is_command_new(&segs) {
            let program = node
                .args
                .first()
                .map_or_else(|| "<none>".to_owned(), describe);
            self.site(program);
        } else if let Some(name) = path_start(&segs) {
            self.start(name);
        }
        if let Some(callee) = segs.last() {
            self.call(callee.clone(), &node.args);
        }
        for arg in &node.args {
            self.visit_expr(arg);
        }
    }

    fn visit_expr_path(&mut self, node: &'ast ExprPath) {
        let segs = segments(&node.path);
        if is_command_new(&segs) {
            self.site(String::new());
        } else if let Some(name) = path_start(&segs) {
            self.start(name);
        }
        visit::visit_expr_path(self, node);
    }

    fn visit_expr_method_call(&mut self, node: &'ast ExprMethodCall) {
        let method = node.method.to_string();
        if node.args.is_empty() && ZERO_ARG_STARTS.contains(&method.as_str()) {
            self.start(&method);
        }
        self.call(method, &node.args);
        visit::visit_expr_method_call(self, node);
    }
}

/// Scans `file`'s syntax tree, returning its `mod x;` declarations.
fn scan_file(
    found: &mut Findings,
    file: &str,
    dir: String,
    test: bool,
    ast: &syn::File,
) -> Vec<Pending> {
    let mut scanner = Scanner {
        file: file.to_owned(),
        dir,
        test: test || attrs_test_only(&ast.attrs, false),
        fns: Vec::new(),
        found,
        pending: Vec::new(),
    };
    scanner.visit_file(ast);
    scanner.pending
}

/// Walks the module graph of `sources` (keyed by path under `src/`) from `roots`.
fn scan(sources: &BTreeMap<String, String>, roots: &[&str]) -> Findings {
    let mut found = Findings::default();
    let mut queue: Vec<(String, String, bool)> = Vec::new();
    for root in roots {
        if sources.contains_key(*root) {
            queue.push(((*root).to_owned(), root_dir(root), false));
        } else {
            found.refusals.push(format!("{root}: crate root missing"));
        }
    }
    let mut reached = BTreeSet::new();
    while let Some((file, dir, test)) = queue.pop() {
        if !reached.insert(file.clone()) {
            found
                .refusals
                .push(format!("{file}: declared as a module twice"));
            continue;
        }
        let Some(source) = sources.get(&file) else {
            continue;
        };
        let ast = match syn::parse_file(source) {
            Ok(ast) => ast,
            Err(e) => {
                found.refusals.push(format!("{file}: does not parse: {e}"));
                continue;
            }
        };
        for pending in scan_file(&mut found, &file, dir, test, &ast) {
            let candidates: Vec<String> = [
                format!("{}.rs", pending.stem),
                format!("{}/mod.rs", pending.stem),
            ]
            .into_iter()
            .filter(|candidate| sources.contains_key(candidate))
            .collect();
            match candidates.as_slice() {
                [child] => queue.push((child.clone(), pending.stem, pending.test)),
                _ => found.refusals.push(format!(
                    "{}: `mod` for `{}` resolves to {} files",
                    pending.parent,
                    pending.stem,
                    candidates.len()
                )),
            }
        }
    }
    for file in sources.keys().filter(|file| !reached.contains(*file)) {
        found
            .refusals
            .push(format!("{file}: no `mod` declaration reaches it"));
    }
    found
}

/// The calls in `file` that `caller` makes to `callee`.
fn calls_of<'a>(found: &'a Findings, file: &str, caller: &str, callee: &str) -> Vec<&'a Call> {
    found
        .calls
        .iter()
        .filter(|call| call.file == file && call.caller == caller && call.callee == callee)
        .collect()
}

/// The production calls in `file` to `callee`, from any function.
fn callers_of<'a>(found: &'a Findings, file: &str, callee: &str) -> Vec<&'a Call> {
    found
        .calls
        .iter()
        .filter(|call| call.file == file && call.callee == callee)
        .collect()
}

/// Whether argument `at` of `call` is a path ending in `name`.
fn passes(call: &Call, at: usize, name: &str) -> bool {
    call.args
        .get(at)
        .and_then(Option::as_ref)
        .and_then(|path| path.last())
        .is_some_and(|last| last == name)
}

/// Proves every `runner` call `func` makes passes `ceiling` at argument `at`,
/// directly or through a `LocalCeiling` parameter every production caller fills with it.
fn prove_ceiling(
    found: &Findings,
    (file, func): (&str, &str),
    (runner, at): (&str, usize),
    ceiling: &str,
) -> Result<(), String> {
    if !found.ceilings.contains(ceiling) {
        return Err(format!(
            "`{ceiling}` is no `{CEILING_TYPE}` constant of {RUNNER_MODULE}"
        ));
    }
    let runs = calls_of(found, file, func, runner);
    if runs.is_empty() {
        return Err(format!("no `{runner}` call"));
    }
    for run in runs {
        if passes(run, at, ceiling) {
            continue;
        }
        let param =
            run.args
                .get(at)
                .and_then(Option::as_ref)
                .and_then(|path| match path.as_slice() {
                    [param] => found
                        .fns
                        .get(&(file.to_owned(), func.to_owned()))
                        .and_then(|facts| facts.ceiling_params.get(param)),
                    _ => None,
                });
        let Some(&index) = param else {
            return Err(format!(
                "a `{runner}` call passes neither `{ceiling}` nor a ceiling parameter"
            ));
        };
        let callers = callers_of(found, file, func);
        if callers.is_empty() || !callers.iter().all(|caller| passes(caller, index, ceiling)) {
            return Err(format!(
                "not every production caller passes `{ceiling}` to its ceiling parameter"
            ));
        }
    }
    Ok(())
}

/// Requires `func` in `file` to call `runner`.
fn calls_runner(found: &Findings, file: &str, func: &str, runner: &str) -> Result<(), String> {
    if calls_of(found, file, func, runner).is_empty() {
        Err(format!("no `{runner}` call"))
    } else {
        Ok(())
    }
}

/// Requires `file` to be `owner`.
fn owned_by(file: &str, owner: &str) -> Result<(), String> {
    if file == owner {
        Ok(())
    } else {
        Err(format!("only {owner} builds this runner's child"))
    }
}

/// Proves the dev-app builder `func` is called only from `spawn_command`, which
/// starts it through `spawn_hardened`.
fn prove_dev_app(found: &Findings, file: &str, func: &str) -> Result<(), String> {
    owned_by(file, "watch.rs")?;
    let builds = found
        .fns
        .get(&(file.to_owned(), func.to_owned()))
        .is_some_and(|facts| facts.returns_command);
    let callers = callers_of(found, file, func);
    if !builds || callers.is_empty() || !callers.iter().all(|call| call.caller == "spawn_command") {
        return Err(
            "the dev app must be a `Command` builder only `spawn_command` calls".to_owned(),
        );
    }
    calls_runner(found, file, "spawn_command", "spawn_hardened")
}

/// Proves the site row `(file, func)` hands its child to `runner`.
fn prove(found: &Findings, file: &str, func: &str, runner: Runner) -> Result<(), String> {
    match runner {
        Runner::Local(ceiling) => prove_ceiling(found, (file, func), ("run_local", 1), ceiling),
        Runner::LocalFed(ceiling) => {
            prove_ceiling(found, (file, func), ("run_local_fed", 2), ceiling)
        }
        Runner::Probe => calls_runner(found, file, func, "run_probe"),
        Runner::Transfer => owned_by(file, RUNNER_MODULE),
        Runner::Cargo => owned_by(file, CARGO_MODULE),
        Runner::Inherited(role) => {
            if calls_of(found, file, func, "run_inherited")
                .iter()
                .any(|call| passes(call, 1, role))
            {
                Ok(())
            } else {
                Err(format!("no `run_inherited` call passing `{role}`"))
            }
        }
        Runner::Replace => calls_runner(found, file, func, "exec_naming"),
        Runner::Opener => {
            owned_by(file, "browser.rs")?;
            if func == "open_with" {
                calls_runner(found, file, func, "spawn_hardened")
            } else {
                Err("the opener body is `open_with`".to_owned())
            }
        }
        Runner::DevApp => prove_dev_app(found, file, func),
    }
}

/// The keys whose counts differ between `found` and `expected`, one line each.
fn drift(kind: &str, found: &BTreeMap<Key, usize>, expected: &BTreeMap<Key, usize>) -> Vec<String> {
    let keys: BTreeSet<&Key> = found.keys().chain(expected.keys()).collect();
    keys.into_iter()
        .filter_map(|key| {
            let got = found.get(key).copied().unwrap_or(0);
            let want = expected.get(key).copied().unwrap_or(0);
            (got != want).then(|| {
                format!(
                    "{}: `{}` {kind} `{}` x{got} (inventory: {want})",
                    key.0, key.1, key.2
                )
            })
        })
        .collect()
}

/// Every way `found` departs from the `sites` and `starts` inventories.
fn check(found: &Findings, sites: &[Site], starts: &[Start]) -> Vec<String> {
    let mut offences = found.refusals.clone();
    let mut expected_sites: BTreeMap<Key, usize> = BTreeMap::new();
    for &(file, func, program, runner, count) in sites {
        let want = expected_sites
            .entry((file.to_owned(), func.to_owned(), program.to_owned()))
            .or_default();
        *want = want.saturating_add(count);
        if let Err(why) = prove(found, file, func, runner) {
            offences.push(format!(
                "{file}: `{func}` site `{program}` under {runner:?}: {why}"
            ));
        }
    }
    offences.extend(drift("child site", &found.sites, &expected_sites));
    let mut expected_starts: BTreeMap<Key, usize> = BTreeMap::new();
    for &(file, func, name, count) in starts {
        let want = expected_starts
            .entry((file.to_owned(), func.to_owned(), name.to_owned()))
            .or_default();
        *want = want.saturating_add(count);
    }
    offences.extend(drift("child start", &found.starts, &expected_starts));
    offences
}

/// Every `.rs` file under `dir`, recursively.
fn rust_files(dir: &Path, depth: usize, out: &mut Vec<PathBuf>) -> std::io::Result<()> {
    if depth > MAX_DEPTH {
        return Err(std::io::Error::other(format!(
            "source tree deeper than {MAX_DEPTH}: {}",
            dir.display()
        )));
    }
    for entry in std::fs::read_dir(dir)? {
        let path = entry?.path();
        if path.is_dir() {
            rust_files(&path, depth + 1, out)?;
        } else if path.extension().is_some_and(|ext| ext == "rs") {
            out.push(path);
        }
    }
    Ok(())
}

/// Every source of the `ipe` crate's `src/`, keyed by its `/`-separated path under it.
fn crate_sources() -> std::io::Result<BTreeMap<String, String>> {
    let root = e2e_support::manifest_dir!().join("src");
    let mut paths = Vec::new();
    rust_files(&root, 0, &mut paths)?;
    let mut sources = BTreeMap::new();
    for path in paths {
        let rel = path
            .strip_prefix(&root)
            .map_err(std::io::Error::other)?
            .to_string_lossy()
            .replace('\\', "/");
        sources.insert(rel, std::fs::read_to_string(&path)?);
    }
    Ok(sources)
}

/// Whether the `into_reader` ending at `before` is the first argument of `read_capped`.
fn reader_is_capped(before: &str) -> bool {
    let receiver = before.trim_end_matches(|c: char| c.is_ascii_alphanumeric() || c == '_');
    receiver.len() < before.len() && receiver.ends_with(CAPPED_CALL)
}

/// Every unbounded `ureq` body read in one flattened source.
fn ureq_violations(flat: &str) -> Vec<String> {
    let mut found = Vec::new();
    if flat.contains("ureq") {
        found.extend(
            UNBOUNDED_BODY_READS
                .iter()
                .filter(|needle| flat.contains(*needle))
                .map(|needle| (*needle).to_owned()),
        );
    }
    found.extend(
        flat.match_indices(READER)
            .filter(|(at, _)| !flat.get(..*at).is_some_and(reader_is_capped))
            .map(|_| READER.to_owned()),
    );
    found
}

#[test]
fn every_production_child_site_names_its_runner() {
    let sources = crate_sources().expect("source tree readable");
    assert!(
        sources.contains_key(RUNNER_MODULE),
        "the scan must see src/{RUNNER_MODULE}, or it is scanning the wrong tree"
    );
    let starts = [RUNNER_BODIES, ADMITTED].concat();
    let offences = check(&scan(&sources, ROOTS), SITES, &starts);
    assert!(
        offences.is_empty(),
        "every production child must start inside its named runner, and every site and start \
         must match SITES, RUNNER_BODIES and ADMITTED:\n{}",
        offences.join("\n")
    );
}

#[test]
fn no_unbounded_ureq_body_read() {
    let sources = crate_sources().expect("source tree readable");
    let offenders: Vec<String> = sources
        .iter()
        .filter(|(rel, _)| rel.as_str() != RUNNER_MODULE)
        .flat_map(|(rel, source)| {
            ureq_violations(&flatten(source))
                .into_iter()
                .map(move |needle| format!("{rel}: {needle}"))
        })
        .collect();
    assert!(
        offenders.is_empty(),
        "an HTTP body must be read through remote_ingest::read_capped:\n{}",
        offenders.join("\n")
    );
}

/// The ceilings and runners a sample crate scans against.
const SAMPLE_RUNNERS: &str = "\
    pub const Q: LocalCeiling = LocalCeiling::new();
    pub const R: u32 = 1;
    pub fn run_local(c: Command, l: LocalCeiling, s: u8) {}
";

/// The one admitted site of a sample crate: a `rustc` query under `Q`.
const SAMPLE_TOOL: &str = "\
    fn query() {
        let mut c = std::process::Command::new(\"rustc\");
        run_local(c, Q, S);
    }
";

/// The inventory row of [`SAMPLE_TOOL`].
const SAMPLE_SITES: &[Site] = &[("tool.rs", "query", "\"rustc\"", Runner::Local("Q"), 1)];

/// The offences of a sample crate whose `tool.rs` is [`SAMPLE_TOOL`] followed by `extra`.
fn sample(extra: &str, sites: &[Site], starts: &[Start]) -> Vec<String> {
    let sources: BTreeMap<String, String> = [
        ("lib.rs", "mod remote_ingest;\nmod tool;\n".to_owned()),
        ("remote_ingest.rs", SAMPLE_RUNNERS.to_owned()),
        ("tool.rs", format!("{SAMPLE_TOOL}\n{extra}")),
    ]
    .into_iter()
    .map(|(rel, source)| (rel.to_owned(), source))
    .collect();
    check(&scan(&sources, &["lib.rs"]), sites, starts)
}

#[test]
fn the_scan_admits_a_listed_site_and_its_test_code() {
    let admitted = [
        "",
        "#[cfg(test)] mod tests { fn t(c: &mut Command) { c.output(); Command::new(\"x\"); } }",
        "#[test] fn t(c: &mut Command) { c.status(); }",
        "#[cfg(all(unix, test))] fn t(c: &mut Command) { c.spawn(); }",
        "use std::os::unix::process::CommandExt as _;",
        "fn f() { let _ = format!(\"{}\", 1); }",
    ];
    for extra in admitted {
        let offences = sample(extra, SAMPLE_SITES, &[]);
        assert!(
            offences.is_empty(),
            "must admit `{extra}`:\n{}",
            offences.join("\n")
        );
    }
}

#[test]
fn the_scan_refuses_each_unlisted_child_start() {
    let refused = [
        "fn raw(c: &mut Command) { c.output(); }",
        "fn raw(c: &mut Command) { c.status(); }",
        "fn raw(c: &mut Command) { c.spawn(); }",
        "fn raw(c: &mut Command) { c.exec(); }",
        "fn raw(c: Child) { c.wait_with_output(); }",
        "fn raw(c: &mut Command) { Command::output(c); }",
        "fn raw(c: Child) { std::process::Child::wait_with_output(c); }",
        "fn raw(c: &mut Command) { <Command as CommandExt>::exec(c); }",
        "fn raw(c: &mut Command) { let _ = format!(\"{:?}\", c.output()); }",
        "fn raw(c: &mut Command) { let _ = vec![c.spawn()]; }",
        "fn raw(c: &mut Command) { let _ = vec![c.spawn(); 2]; }",
        "fn raw(c: Command) { ipe_runtime_rust::system::spawn_hardened(c); }",
        "fn raw(c: Command) { let start = spawn_hardened; start(c); }",
        "fn raw(c: &mut Command) { let _ = c.iter().map(Command::output); }",
        "#[cfg(any(unix, test))] fn raw(c: &mut Command) { c.spawn(); }",
        "#[cfg(not(test))] fn raw(c: &mut Command) { c.spawn(); }",
    ];
    for extra in refused {
        assert!(
            !sample(extra, SAMPLE_SITES, &[]).is_empty(),
            "must refuse `{extra}`"
        );
    }
}

#[test]
fn the_scan_refuses_each_unlisted_or_unproved_site() {
    let refused = [
        "fn other() { Command::new(\"git\"); }",
        "fn other() { let new = Command::new; }",
        "fn other() { Command::new(\"rustc\"); }",
        "use std::process::Command as Spawn;",
        "use std::process::{Command as Spawn, Stdio};",
        "use crate::remote_ingest::run_local as run;",
        "type Spawn = std::process::Command;",
        "fn other() { include!(\"elsewhere.rs\"); }",
    ];
    for extra in refused {
        assert!(
            !sample(extra, SAMPLE_SITES, &[]).is_empty(),
            "must refuse `{extra}`"
        );
    }
    let stale: &[Site] = &[
        ("tool.rs", "query", "\"rustc\"", Runner::Local("Q"), 1),
        ("tool.rs", "gone", "\"cargo\"", Runner::Local("Q"), 1),
    ];
    assert!(
        !sample("", stale, &[]).is_empty(),
        "must refuse a stale row"
    );
    let wrong_ceiling: &[Site] = &[("tool.rs", "query", "\"rustc\"", Runner::Local("R"), 1)];
    assert!(
        !sample("", wrong_ceiling, &[]).is_empty(),
        "must refuse a row naming another ceiling, or a constant not of type `LocalCeiling`"
    );
    let wrong_runner: &[Site] = &[(
        "tool.rs",
        "query",
        "\"rustc\"",
        Runner::Inherited("UserProgram"),
        1,
    )];
    assert!(
        !sample("", wrong_runner, &[]).is_empty(),
        "must refuse a row naming another runner"
    );
}

#[test]
fn a_ceiling_is_proved_through_one_hop_only() {
    let one_hop = "\
        fn hop(l: LocalCeiling) { let c = Command::new(\"cc\"); run_local(c, l, S); }
        fn top() { hop(Q); }
    ";
    let two_hops = "\
        fn hop(l: LocalCeiling) { let c = Command::new(\"cc\"); run_local(c, l, S); }
        fn mid(l: LocalCeiling) { hop(l); }
        fn top() { mid(Q); }
    ";
    let sites: &[Site] = &[
        ("tool.rs", "query", "\"rustc\"", Runner::Local("Q"), 1),
        ("tool.rs", "hop", "\"cc\"", Runner::Local("Q"), 1),
    ];
    let offences = sample(one_hop, sites, &[]);
    assert!(
        offences.is_empty(),
        "must admit one hop:\n{}",
        offences.join("\n")
    );
    assert!(
        !sample(two_hops, sites, &[]).is_empty(),
        "must refuse two hops"
    );
}

#[test]
fn an_admitted_start_holds_only_in_its_function() {
    let starts: &[Start] = &[("tool.rs", "post", "status", 1)];
    let offences = sample("fn post(r: Response) { r.status(); }", SAMPLE_SITES, starts);
    assert!(
        offences.is_empty(),
        "must admit the listed start:\n{}",
        offences.join("\n")
    );
    assert!(
        !sample(
            "fn other(r: Response) { r.status(); }",
            SAMPLE_SITES,
            starts
        )
        .is_empty(),
        "must refuse the start in an unlisted function"
    );
}

#[test]
fn the_module_walk_refuses_what_it_cannot_classify() {
    let base = [
        ("lib.rs", "mod remote_ingest;\nmod tool;\n"),
        ("remote_ingest.rs", SAMPLE_RUNNERS),
        ("tool.rs", SAMPLE_TOOL),
    ];
    let cases: [&[(&str, &str)]; 4] = [
        &[("stray.rs", "fn f(c: &mut Command) { c.spawn(); }")],
        &[(
            "lib.rs",
            "mod remote_ingest;\n#[path = \"tool.rs\"] mod tool;\n",
        )],
        &[
            ("lib.rs", "mod remote_ingest;\nmod tool;\nmod twin;\n"),
            ("twin.rs", ""),
            ("twin/mod.rs", ""),
        ],
        &[("tool.rs", "fn broken( {")],
    ];
    for case in cases {
        let mut sources: BTreeMap<String, String> = base
            .iter()
            .map(|(rel, source)| ((*rel).to_owned(), (*source).to_owned()))
            .collect();
        sources.extend(
            case.iter()
                .map(|(rel, source)| ((*rel).to_owned(), (*source).to_owned())),
        );
        assert!(
            !check(&scan(&sources, &["lib.rs"]), SAMPLE_SITES, &[]).is_empty(),
            "the walk must refuse {case:?}"
        );
    }
}

#[test]
fn matcher_refuses_each_unbounded_body_read() {
    let refused = [
        "use ureq; let v: Value = resp.into_json()?;",
        "use ureq; let s = resp.into_string()?;",
        "let r = resp.into_reader();",
        "read_capped(std::io::empty()).and(resp.into_reader())",
        "read_capped(\n  (resp).into_reader(), 4)",
    ];
    for sample in refused {
        assert!(
            !ureq_violations(&flatten(sample)).is_empty(),
            "matcher must refuse: {sample}"
        );
    }
}

#[test]
fn matcher_admits_the_bounded_forms() {
    let admitted = [
        "remote_ingest::read_capped(\n    response.into_reader(),\n    MAX,\n)",
        "Git::isolated(dir).args([\"fetch\"])",
        "let s = value.into_string();",
    ];
    for sample in admitted {
        assert!(
            ureq_violations(&flatten(sample)).is_empty(),
            "matcher must admit: {sample}"
        );
    }
}
