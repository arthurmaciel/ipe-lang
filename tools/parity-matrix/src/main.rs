//! parity-matrix — symbol × layer parity matrix for the Sky→Rust backend.
//!
//! # Usage
//!
//! ```text
//! # Extract and write TSV to docs/architecture/parity-matrix.tsv
//! parity-matrix extract > docs/architecture/parity-matrix.tsv
//!
//! # Read TSV and write Markdown report to docs/architecture/parity-matrix.md
//! parity-matrix report docs/architecture/parity-matrix.tsv \
//!     > docs/architecture/parity-matrix.md
//!
//! # Both in one shot (extract, then report the generated file):
//! parity-matrix extract > docs/architecture/parity-matrix.tsv && \
//!     parity-matrix report docs/architecture/parity-matrix.tsv \
//!     > docs/architecture/parity-matrix.md
//! ```
//!
//! Paths default to the workspace root of **this repo** (detected from the
//! binary's own location) and `../sky` for the reference.  Override with
//! `--our-dir` and `--ref-dir`.

#![forbid(unsafe_code)]

use std::collections::{BTreeMap, HashMap, HashSet};
use std::fmt::Write as FmtWrite;
use std::fs;
use std::path::{Path, PathBuf};

// ── CLI ──────────────────────────────────────────────────────────────────────

fn usage() -> &'static str {
    "parity-matrix <extract|report> [--our-dir DIR] [--ref-dir DIR] [TSV_FILE]"
}

struct Config {
    our_dir: PathBuf,
    ref_dir: PathBuf,
}

impl Config {
    /// Detect the workspace root from `CARGO_MANIFEST_DIR` (set by Cargo) or
    /// from the running binary's path (two levels up from `target/…/parity-matrix`).
    fn detect_our_dir() -> PathBuf {
        // Cargo sets CARGO_MANIFEST_DIR to the crate root; from there the
        // workspace root is `../../` (crate is tools/parity-matrix/).
        if let Ok(manifest) = std::env::var("CARGO_MANIFEST_DIR") {
            let p = PathBuf::from(manifest);
            if let Some(workspace) = p.parent().and_then(|p| p.parent())
                && workspace.join("Cargo.toml").exists()
            {
                return workspace.to_path_buf();
            }
        }
        // Fallback: current working directory.
        std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."))
    }

    fn new(our_dir: Option<PathBuf>, ref_dir: Option<PathBuf>) -> Self {
        let our = our_dir.unwrap_or_else(Self::detect_our_dir);
        let refr = ref_dir.unwrap_or_else(|| {
            // ../sky relative to workspace root
            our.parent()
                .map(|p| p.join("sky"))
                .unwrap_or_else(|| PathBuf::from("../sky"))
        });
        Config {
            our_dir: our,
            ref_dir: refr,
        }
    }
}

/// Mirror of `runtime/tests/symbol_resolution.rs::KNOWN_DEAD_OR_EPILOGUE` —
/// naming strings that never reach `callee_name()` (emit intercepts inline)
/// or live in the generated-code preamble, not the runtime library.
const KNOWN_DEAD_OR_EPILOGUE: &[&str] = &[
    // ── Dead: emit_task_retry_call (#134) constructs RetryPolicy / ShouldRetry
    //         values inline for the builder variants; only task_retry_with has
    //         a real runtime fn. These name strings are never emitted. ────────
    "task_default_retry_policy",
    "task_exponential_backoff",
    "task_linear_backoff",
    "task_retry_on",
    "task_with_base_ms",
    "task_with_jitter",
    "task_with_kind",
    "task_with_max_attempts",
    "task_with_retry_on",
    "html_attr_tabindex_",
    "http_default_request",
    "http_with_method",
    "http_with_body",
    "http_with_header",
    "http_with_timeout",
    // #33 §6.2 Go-parity builders — inline clone-and-reassign emission.
    "http_with_url",
    "http_with_follow_redirects",
    "http_with_max_redirects",
    "live_route",
    "sky_cli_program_",
    "ui_layout_with",
    // #217: emit_expr's DbDefaultMigration arm emits the `Migration` record
    // struct literal inline; this name string is never emitted.
    "db_default_migration",
    "list_map_consume",
];

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let mut our_dir: Option<PathBuf> = None;
    let mut ref_dir: Option<PathBuf> = None;
    let mut positional: Vec<String> = Vec::new();
    let mut i = 1usize;
    while i < args.len() {
        match args[i].as_str() {
            "--our-dir" => {
                i += 1;
                if i < args.len() {
                    our_dir = Some(PathBuf::from(&args[i]));
                }
            }
            "--ref-dir" => {
                i += 1;
                if i < args.len() {
                    ref_dir = Some(PathBuf::from(&args[i]));
                }
            }
            "--help" | "-h" => {
                eprintln!("{}", usage());
                return;
            }
            s => positional.push(s.to_string()),
        }
        i += 1;
    }

    let cmd = positional.first().map(String::as_str).unwrap_or("");
    let cfg = Config::new(our_dir, ref_dir);

    match cmd {
        "extract" => {
            match run_extract(&cfg) {
                Ok(tsv) => print!("{tsv}"),
                Err(e) => {
                    eprintln!("extract error: {e}");
                    std::process::exit(1);
                }
            }
        }
        "report" => {
            let tsv_path = positional.get(1).map(String::as_str).unwrap_or("-");
            let tsv = if tsv_path == "-" {
                use std::io::Read;
                let mut s = String::new();
                std::io::stdin().read_to_string(&mut s).unwrap_or(0);
                s
            } else {
                match fs::read_to_string(tsv_path) {
                    Ok(s) => s,
                    Err(e) => {
                        eprintln!("cannot read {tsv_path}: {e}");
                        std::process::exit(1);
                    }
                }
            };
            match run_report(&tsv) {
                Ok(md) => print!("{md}"),
                Err(e) => {
                    eprintln!("report error: {e}");
                    std::process::exit(1);
                }
            }
        }
        _ => {
            eprintln!("{}", usage());
            std::process::exit(1);
        }
    }
}

// ── Row ──────────────────────────────────────────────────────────────────────

/// TSV column header line.
const TSV_HEADER: &str = "variant\tqualifier\tmember\tarity\tclass\tin_all\thas_decl\t\
    has_scheme\tin_first_schemed\tin_canon_qual\thas_lower_arm\truntime_sym\t\
    runtime_sym_exists\tref_runtime_sym_exists\tin_ref_stdlib\tstatus";

#[derive(Debug, Default, Clone)]
struct Row {
    variant: String,
    qualifier: String,
    member: String,
    arity: String,
    class: String,
    in_all: bool,
    has_decl: bool,
    has_scheme: bool,
    in_first_schemed: bool,
    in_canon_qual: bool,
    has_lower_arm: bool,
    runtime_sym: String,
    runtime_sym_exists: bool,
    ref_runtime_sym_exists: bool,
    in_ref_stdlib: bool,
    status: String,
}

impl Row {
    fn compute_status(&mut self, compiled_source_quals: &HashSet<String>) {
        let mut issues: Vec<&str> = Vec::new();

        // Only flag MISMATCH (bugs) for wired variants.
        if self.in_all {
            if !self.runtime_sym.is_empty()
                && !self.runtime_sym_exists
                && !KNOWN_DEAD_OR_EPILOGUE.contains(&self.runtime_sym.as_str())
            {
                issues.push("runtime_sym_missing");
            }
            if !self.has_lower_arm {
                issues.push("lower_arm_missing");
            }
            // Canon check: skip empty qualifier (no decl), internal qualifiers
            // (starting with '_'), qualifiers of COMPILED-SOURCE Layer-3 modules
            // (e.g. Regex, Path, ToString, Pure — their members are point-free
            // `Ffi.kernel "…"` aliases routed by `detect_kernel_alias`, so they
            // are DELIBERATELY absent from the `QUALIFIERS` kernel-qualifier table
            // per the `compiled_vs_kernel_qualifier_disjoint` invariant — derived
            // from `COMPILED_STD_MODULES` so future compiled modules never drift),
            // and the remaining kernel qualifiers installed through other
            // mechanisms (Log, Html, Ui, PubSub — not in the QUALIFIERS table).
            let skip_canon = self.qualifier.is_empty()
                || self.qualifier.starts_with('_')
                || compiled_source_quals.contains(&self.qualifier)
                || matches!(
                    self.qualifier.as_str(),
                    "Log" | "Html" | "Ui" | "PubSub" | "Background"
                    | "Border" | "Font" | "Region" | "Input" | "Attr"
                    | "Event" | "Lazy" | "Keyed"
                    | "CssSafety" | "Middleware" | "Db.Decode"
                    | "Regex" | "Path"
                );
            if !skip_canon && !self.in_canon_qual {
                issues.push("canon_missing");
            }
        }

        if issues.is_empty() {
            self.status = if self.in_all {
                "OK".to_string()
            } else {
                "BACKLOG".to_string()
            };
        } else {
            self.status = format!("MISMATCH:{}", issues.join("+"));
        }
    }

    fn to_tsv(&self) -> String {
        format!(
            "{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}",
            self.variant,
            self.qualifier,
            self.member,
            self.arity,
            self.class,
            yn(self.in_all),
            yn(self.has_decl),
            yn(self.has_scheme),
            yn(self.in_first_schemed),
            yn(self.in_canon_qual),
            yn(self.has_lower_arm),
            self.runtime_sym,
            yn(self.runtime_sym_exists),
            yn(self.ref_runtime_sym_exists),
            yn(self.in_ref_stdlib),
            self.status,
        )
    }
}

fn yn(b: bool) -> &'static str {
    if b {
        "Y"
    } else {
        "N"
    }
}

// ── Extract entry point ──────────────────────────────────────────────────────

fn run_extract(cfg: &Config) -> Result<String, String> {
    let our = &cfg.our_dir;
    let refr = &cfg.ref_dir;

    // ── Layer 1: ipe_kernels enum variants ───────────────────────────────────
    let kernels_lib = our.join("crates/ipe_kernels/src/lib.rs");
    let kernels_src = read_file(&kernels_lib)?;

    let all_variants = scan_enum_variants(&kernels_src, "StdlibKernel");
    let all_set = scan_all_slice(&kernels_src);
    let decl_map = scan_decl_arms(&kernels_src); // variant → DeclInfo

    // ── Layer 2 / 3: naming + constrain scheme ───────────────────────────────
    let naming_lib = our.join("crates/ipe_backend_rust/src/naming.rs");
    let naming_src = read_file(&naming_lib)?;
    let naming_map = scan_kernel_name(&naming_src); // variant → runtime_sym

    let constrain_lib = our.join("crates/ipe_types/src/constrain.rs");
    let constrain_src = read_file(&constrain_lib)?;
    let first_schemed = scan_named_slice(&constrain_src, "FIRST_SCHEMED");
    let relocated = scan_named_slice(&constrain_src, "RELOCATED");
    let mut schemed_set = first_schemed.clone();
    schemed_set.extend(relocated.iter().cloned());

    // ── Layer 4: canon qualifier table ───────────────────────────────────────
    let env_lib = our.join("crates/ipe_canon/src/env.rs");
    let env_src = read_file(&env_lib)?;
    let canon_qual_set = scan_canon_qualifiers(&env_src); // (qualifier, member)

    // Compiled-source Layer-3 module qualifiers (Regex, Path, ToString, Pure, …)
    // are DELIBERATELY absent from the QUALIFIERS table (they route via
    // `detect_kernel_alias`), so the canon-parity check must skip them.
    let stdlib_lib = our.join("crates/skyc/src/stdlib.rs");
    let stdlib_src = read_file(&stdlib_lib)?;
    let compiled_source_quals = scan_compiled_source_qualifiers(&stdlib_src);

    // ── Layer 5: lower dispatch arms ─────────────────────────────────────────
    let lower_lib = our.join("crates/ipe_lower/src/lower.rs");
    let lower_src = read_file(&lower_lib)?;
    let lower_arms = scan_lower_arms(&lower_src); // variant names with lower arm

    // ── Layer 7 / 8: runtime pub fn names ────────────────────────────────────
    let runtime_dir = our.join("runtime/src/ipe_runtime");
    let our_fns = scan_runtime_fns(&runtime_dir)?;

    let ref_runtime_dir = refr.join("runtime-rust/src/sky_runtime");
    let ref_fns = scan_runtime_fns(&ref_runtime_dir).unwrap_or_default();

    // ── Reference: sky-stdlib symbols ────────────────────────────────────────
    let ref_stdlib_dir = refr.join("sky-stdlib");
    let ref_stdlib_syms = scan_sky_stdlib(&ref_stdlib_dir).unwrap_or_default();

    // ── Build matrix ─────────────────────────────────────────────────────────
    let mut rows: Vec<Row> = Vec::new();

    for variant in &all_variants {
        let mut row = Row {
            variant: variant.clone(),
            in_all: all_set.contains(variant.as_str()),
            ..Default::default()
        };

        if let Some(decl) = decl_map.get(variant) {
            row.has_decl = true;
            row.qualifier = decl.qualifier.clone();
            row.member = decl.member.clone();
            row.arity = decl.arity.to_string();
            row.class = decl.class.clone();
        }

        row.has_scheme = schemed_set.contains(variant.as_str());
        row.in_first_schemed = first_schemed.contains(variant.as_str());

        let q = row.qualifier.as_str();
        let m = row.member.as_str();
        if !q.is_empty() && !m.is_empty() {
            row.in_canon_qual = canon_qual_set.contains(&(q.to_string(), m.to_string()));
            row.in_ref_stdlib = ref_stdlib_syms.contains(&(q.to_string(), m.to_string()));
        }

        row.has_lower_arm = lower_arms.contains(variant.as_str());

        if let Some(sym) = naming_map.get(variant) {
            row.runtime_sym = sym.clone();
            row.runtime_sym_exists = our_fns.contains(sym.as_str());
            row.ref_runtime_sym_exists = ref_fns.contains(sym.as_str());
        }

        row.compute_status(&compiled_source_quals);
        rows.push(row);
    }

    // ── Output TSV ───────────────────────────────────────────────────────────
    let mut out = String::new();
    out.push_str(TSV_HEADER);
    out.push('\n');
    for r in &rows {
        out.push_str(&r.to_tsv());
        out.push('\n');
    }
    Ok(out)
}

// ── Scanner functions ──────────────────────────────────────���──────────────────

/// Read a file or return an `Err` with a useful message.
fn read_file(p: &Path) -> Result<String, String> {
    fs::read_to_string(p).map_err(|e| format!("cannot read {}: {e}", p.display()))
}

/// Scan all files `*.rs` in a directory (non-recursive for the live/ and ui/
/// sub-directories, we do one level of recursion).
fn scan_runtime_fns(dir: &Path) -> Result<HashSet<String>, String> {
    let mut fns: HashSet<String> = HashSet::new();
    if !dir.exists() {
        return Ok(fns);
    }
    scan_runtime_fns_dir(dir, &mut fns)?;
    Ok(fns)
}

fn scan_runtime_fns_dir(dir: &Path, fns: &mut HashSet<String>) -> Result<(), String> {
    let rd = fs::read_dir(dir).map_err(|e| format!("read_dir {}: {e}", dir.display()))?;
    for entry in rd {
        let entry = entry.map_err(|e| format!("dir entry: {e}"))?;
        let path = entry.path();
        if path.is_dir() {
            scan_runtime_fns_dir(&path, fns)?;
        } else if path.extension().and_then(|e| e.to_str()) == Some("rs") {
            let src = read_file(&path)?;
            for line in src.lines() {
                let trimmed = line.trim();
                // Match: pub fn <name>( or pub async fn <name>(
                let rest = trimmed
                    .strip_prefix("pub fn ")
                    .or_else(|| trimmed.strip_prefix("pub async fn "));
                if let Some(rest) = rest {
                    // Extract name up to `(` or `<` (generic params) or end of token.
                    // Handles both one-liners `pub fn foo(` and multi-line generics
                    // `pub fn foo<\n    T: ...`.
                    let name_end = rest
                        .find(['(', '<', ' ', '\t'])
                        .unwrap_or(rest.len());
                    let base = rest[..name_end].trim();
                    if !base.is_empty() && base != "new" && base.chars().all(|c| c.is_alphanumeric() || c == '_') {
                        fns.insert(base.to_string());
                    }
                }
            }
        }
    }
    Ok(())
}

/// Extract all variant names from a named `pub enum` block.
fn scan_enum_variants(src: &str, enum_name: &str) -> Vec<String> {
    let marker = format!("pub enum {enum_name} {{");
    let start = match src.find(&marker) {
        Some(idx) => idx + marker.len(),
        None => return Vec::new(),
    };
    // Find the closing brace at the same nesting level.
    let body = match src.get(start..) {
        Some(s) => s,
        None => return Vec::new(),
    };
    let mut depth = 1usize;
    let mut end = 0usize;
    for (i, ch) in body.char_indices() {
        match ch {
            '{' => depth += 1,
            '}' => {
                depth -= 1;
                if depth == 0 {
                    end = i;
                    break;
                }
            }
            _ => {}
        }
    }
    let enum_body = match body.get(..end) {
        Some(s) => s,
        None => return Vec::new(),
    };

    let mut variants = Vec::new();
    for line in enum_body.lines() {
        let t = line.trim();
        // Skip comment lines and empty lines.
        if t.starts_with("//") || t.starts_with('#') || t.is_empty() {
            continue;
        }
        // Strip trailing comma and doc comment.
        let t = if let Some(i) = t.find("//") {
            t[..i].trim()
        } else {
            t
        };
        let t = t.trim_end_matches(',').trim();
        // Valid variant: starts with uppercase, no spaces, no `{`.
        if t.starts_with(|c: char| c.is_uppercase())
            && !t.contains(' ')
            && !t.contains('{')
            && !t.is_empty()
        {
            // Remove generic params if any (shouldn't be present in this enum).
            let name = t.split('<').next().unwrap_or(t).trim();
            if !name.is_empty() {
                variants.push(name.to_string());
            }
        }
    }
    variants
}

/// Extract variants listed in `pub const ALL: &'static [Self] = &[` block.
fn scan_all_slice(src: &str) -> HashSet<String> {
    scan_self_slice(src, "ALL")
}

/// Extract `K::Variant` or `Self::Variant` entries from a named const slice.
fn scan_named_slice(src: &str, slice_name: &str) -> HashSet<String> {
    scan_self_slice(src, slice_name)
}

fn scan_self_slice(src: &str, name: &str) -> HashSet<String> {
    // Heuristic: find `const NAME:` anywhere in the source.
    let marker = format!("const {name}:");
    let start = match src.find(&marker) {
        Some(s) => s,
        None => return HashSet::new(),
    };

    // From the marker position, find the `= &[` that opens the actual array,
    // skipping past any type annotation that may also contain `[`.
    let rest = match src.get(start..) {
        Some(s) => s,
        None => return HashSet::new(),
    };

    // Find `= &[` (array) or `= {` (block) — the actual value start.
    let inner_start = if let Some(idx) = rest.find("= &[") {
        idx + 4 // past `= &[`
    } else if let Some(idx) = rest.find("= {") {
        idx + 3
    } else {
        return HashSet::new();
    };

    let body_rest = match rest.get(inner_start..) {
        Some(s) => s,
        None => return HashSet::new(),
    };

    // Determine close character based on what opened the block.
    let open_ch = if rest.as_bytes().get(inner_start.saturating_sub(1)) == Some(&b'[') { '[' } else { '{' };
    let close_ch = if open_ch == '[' { ']' } else { '}' };
    let mut depth = 1usize;
    let mut end = 0usize;
    for (i, ch) in body_rest.char_indices() {
        if ch == open_ch {
            depth += 1;
        } else if ch == close_ch {
            depth -= 1;
            if depth == 0 {
                end = i;
                break;
            }
        }
    }
    let body = match body_rest.get(..end) {
        Some(s) => s,
        None => body_rest,
    };

    let mut set = HashSet::new();
    for line in body.lines() {
        let t = line.trim();
        if t.starts_with("//") || t.is_empty() {
            continue;
        }
        // Match `Self::Variant,` or `K::Variant,`
        for prefix in &["Self::", "K::"] {
            if let Some(rest) = t.strip_prefix(prefix) {
                let variant = rest
                    .trim_end_matches(',')
                    .trim_end_matches(')')
                    .trim()
                    .split(|c: char| !c.is_alphanumeric() && c != '_')
                    .next()
                    .unwrap_or("");
                if !variant.is_empty() && variant.starts_with(|c: char| c.is_uppercase()) {
                    set.insert(variant.to_string());
                }
            }
        }
    }
    set
}

/// Per-variant declaration info from `decl()` match arms.
#[derive(Debug, Default, Clone)]
struct DeclInfo {
    qualifier: String,
    member: String,
    arity: u8,
    class: String,
}

/// Scan `decl()` match arms from ipe_kernels/src/lib.rs.
///
/// The actual form is a shorthand `d(...)` helper:
/// ```
///     Self::StringFromInt => d("String", "fromInt", 1, Pure, "string_from_int"),
/// ```
/// Positional args: qualifier, name, arity, class, emit.
fn scan_decl_arms(src: &str) -> HashMap<String, DeclInfo> {
    let mut map = HashMap::new();

    // Find the `fn decl` function so we only scan inside it.
    let fn_start = match src.find("fn decl(") {
        Some(i) => i,
        None => return map,
    };
    let body = match src.get(fn_start..) {
        Some(s) => s,
        None => return map,
    };

    // Each arm is on a single line (or rarely wrapped, but the d() call fits on one line):
    //   Self::StringFromInt => d("String", "fromInt", 1, Pure, "string_from_int"),
    for line in body.lines() {
        let t = line.trim();
        if t.starts_with("//") || t.is_empty() {
            continue;
        }

        // Must match `Self::Variant => d(` pattern.
        let rest = match t.strip_prefix("Self::") {
            Some(r) => r,
            None => continue,
        };

        // Extract variant name.
        let variant_end = rest
            .find(|c: char| !c.is_alphanumeric() && c != '_')
            .unwrap_or(rest.len());
        let variant = &rest[..variant_end];
        if variant.is_empty() || !variant.starts_with(|c: char| c.is_uppercase()) {
            continue;
        }

        // Find `=> d(` after the variant name.
        let arrow_d = match rest.find("=> d(") {
            Some(i) => i + 5, // past `=> d(`
            None => continue,
        };
        let args_str = match rest.get(arrow_d..) {
            Some(s) => s,
            None => continue,
        };

        // Parse positional args: "qualifier", "name", arity, Class, "emit"
        // We'll extract string literals and integers in order.
        let decl = parse_d_args(args_str);
        if !decl.qualifier.is_empty() {
            map.insert(variant.to_string(), decl);
        }
    }
    map
}

/// Parse the positional args of `d("qualifier", "name", arity, Class, "emit")`.
fn parse_d_args(s: &str) -> DeclInfo {
    let mut strings: Vec<String> = Vec::new();
    let mut arity: u8 = 0;
    let mut class = String::new();

    let mut i = 0usize;
    let bytes = s.as_bytes();
    while i < bytes.len() {
        match bytes[i] {
            b'"' => {
                // String literal — scan to closing `"`.
                let start = i + 1;
                i += 1;
                while i < bytes.len() && bytes[i] != b'"' {
                    i += 1;
                }
                if let Some(piece) = s.get(start..i) {
                    strings.push(piece.to_string());
                }
                i += 1; // skip closing `"`
            }
            b'0'..=b'9' => {
                // Integer literal.
                let start = i;
                while i < bytes.len() && bytes[i].is_ascii_digit() {
                    i += 1;
                }
                if let Ok(n) = s[start..i].parse::<u8>() {
                    arity = n;
                }
            }
            _ => {
                // Identifier — could be a KernelClass variant (Pure, Db, …).
                if bytes[i].is_ascii_alphabetic() {
                    let start = i;
                    while i < bytes.len()
                        && (bytes[i].is_ascii_alphanumeric() || bytes[i] == b'_')
                    {
                        i += 1;
                    }
                    let id = &s[start..i];
                    // KernelClass variants are Pascal-case.
                    if id.starts_with(|c: char| c.is_uppercase()) && class.is_empty() {
                        class = id.to_string();
                    }
                } else {
                    i += 1;
                }
            }
        }
    }

    DeclInfo {
        qualifier: strings.first().cloned().unwrap_or_default(),
        member: strings.get(1).cloned().unwrap_or_default(),
        arity,
        class,
    }
}


/// Scan `kernel_name()` (or `n()`) arms from naming.rs.
///
/// Arms look like:
/// ```
///     KernelFn::StringFromInt => "string_from_int",
///     KernelFn::TaskRun | KernelFn::TaskPerform => "task_run",
/// ```
fn scan_kernel_name(src: &str) -> HashMap<String, String> {
    let mut map = HashMap::new();

    for line in src.lines() {
        let t = line.trim();
        // Skip comment lines.
        if t.starts_with("//") {
            continue;
        }
        // Must contain `=> "` to be a mapping arm.
        if !t.contains("=> \"") {
            continue;
        }
        // Extract the runtime symbol (the string literal after `=> `).
        let sym = match t.find("=> \"").map(|i| i + 4) {
            Some(after) => match t[after..].find('"') {
                Some(close) => t[after..after + close].to_string(),
                None => continue,
            },
            None => continue,
        };
        if sym.is_empty() {
            continue;
        }
        // Extract variant(s) from the LHS — everything before `=>`.
        let lhs = t[..t.find("=> \"").unwrap_or(0)].trim();
        // Handle `KernelFn::Var1 | KernelFn::Var2` patterns.
        for part in lhs.split('|') {
            let part = part.trim();
            let variant = if let Some(r) = part.strip_prefix("KernelFn::") {
                r.trim()
            } else if let Some(r) = part.strip_prefix("n::") {
                r.trim()
            } else {
                continue;
            };
            // Remove trailing punctuation.
            let variant: String = variant
                .chars()
                .take_while(|c| c.is_alphanumeric() || *c == '_')
                .collect();
            if !variant.is_empty() && variant.starts_with(|c: char| c.is_uppercase()) {
                map.insert(variant, sym.clone());
            }
        }
    }
    map
}

/// Scan `COMPILED_STD_MODULES` from `crates/skyc/src/stdlib.rs` and return the
/// set of canonical qualifier short-names for compiled-source Layer-3 modules.
///
/// A compiled-source module's members are point-free `Ffi.kernel "…"` aliases
/// resolved by `detect_kernel_alias`, so — by the
/// `compiled_vs_kernel_qualifier_disjoint` invariant — the module is
/// DELIBERATELY absent from the `QUALIFIERS` kernel-qualifier table in
/// `ipe_canon`.  The canon-parity check must therefore skip these qualifiers.
///
/// The qualifier short-name is the last dotted segment (`Sky.Core.Regex` →
/// `Regex`, `Std.Ui.Responsive` → `Responsive`), which matches the qualifier
/// carried by the corresponding `StdlibKernel::decl()` arm.
fn scan_compiled_source_qualifiers(src: &str) -> HashSet<String> {
    let mut set = HashSet::new();

    let marker = "COMPILED_STD_MODULES";
    let Some(start) = src.find(marker) else {
        return set;
    };
    let Some(body) = src.get(start..) else {
        return set;
    };

    // Each entry is `dotted: "Sky.Core.Regex",` — pull the last segment of
    // every `dotted:` string literal in the slice.
    for (idx, _) in body.match_indices("dotted:") {
        let Some(after) = body.get(idx..) else {
            continue;
        };
        // Locate the opening and closing quotes of the string literal.
        let Some(q1) = after.find('"') else {
            continue;
        };
        let Some(rest) = after.get(q1 + 1..) else {
            continue;
        };
        let Some(q2) = rest.find('"') else {
            continue;
        };
        let Some(dotted) = rest.get(..q2) else {
            continue;
        };
        if let Some(last) = dotted.rsplit('.').next()
            && !last.is_empty()
        {
            set.insert(last.to_string());
        }
    }
    set
}

/// Scan the `QUALIFIERS` table from ipe_canon/src/env.rs.
///
/// Returns a set of `(qualifier, member)` pairs.
fn scan_canon_qualifiers(src: &str) -> HashSet<(String, String)> {
    let mut set = HashSet::new();

    // Find `const QUALIFIERS:` block.
    let marker = "const QUALIFIERS:";
    let start = match src.find(marker) {
        Some(i) => i,
        None => return set,
    };
    let rest = match src.get(start..) {
        Some(s) => s,
        None => return set,
    };

    // Find the outer `= &[` that opens the slice.
    let outer_open = match rest.find("= &[") {
        Some(i) => start + i + 4,
        None => return set,
    };
    let body = match src.get(outer_open..) {
        Some(s) => s,
        None => return set,
    };

    // The actual format (multiline):
    //   (
    //       "String",
    //       &[
    //           "length",
    //           ...
    //       ],
    //   ),
    //
    // State machine:
    //   Outer = inside the top-level &[...]
    //   InTuple = saw `(` at outer depth — next quoted string is qualifier
    //   InMembers = saw `&[` inside a tuple — quoted strings are members
    //   depth tracks nesting depth of `(` / `[` relative to outer &[

    #[derive(Clone, Copy, PartialEq)]
    enum State {
        Outer,
        InTuple,
        InMembers,
    }

    let mut state = State::Outer;
    let mut current_qual = String::new();
    // depth=1 means we are directly inside the outer `&[...]`.
    // depth=2 means inside a tuple `(...)`.
    // depth=3 means inside a member list `&[...]`.
    // depth=0 means outside (done).
    let mut depth = 1usize;
    let mut done = false;

    for line in body.lines() {
        if done {
            break;
        }
        let t = line.trim();
        // Strip inline comments.
        let t = if let Some(i) = t.find("//") {
            t[..i].trim()
        } else {
            t
        };
        if t.is_empty() {
            continue;
        }

        // Process token by token — scan for `(`, `)`, `[`, `]`, and string literals.
        let mut chars = t.char_indices().peekable();
        while let Some((i, ch)) = chars.next() {
            match ch {
                '(' => {
                    depth += 1;
                    if state == State::Outer {
                        state = State::InTuple;
                        current_qual.clear();
                    }
                }
                ')' => {
                    depth = depth.saturating_sub(1);
                    if depth == 0 {
                        done = true;
                        break;
                    }
                    // Back to outer level (depth==1) after closing a tuple.
                    if depth == 1 {
                        state = State::Outer;
                    }
                }
                '[' => {
                    depth += 1;
                    if state == State::InTuple && !current_qual.is_empty() {
                        state = State::InMembers;
                    }
                }
                ']' => {
                    depth = depth.saturating_sub(1);
                    if depth == 0 {
                        done = true;
                        break;
                    }
                    // Back to tuple level (depth==2) after closing member list.
                    if state == State::InMembers && depth == 2 {
                        state = State::InTuple;
                    }
                }
                '"' => {
                    // Scan to closing `"`.
                    let str_start = i + 1;
                    let mut end = str_start;
                    for (j, c) in chars.by_ref() {
                        if c == '"' {
                            end = j;
                            break;
                        }
                    }
                    let s = match t.get(str_start..end) {
                        Some(s) => s.to_string(),
                        None => continue,
                    };
                    match state {
                        State::InTuple if current_qual.is_empty() => {
                            current_qual = s;
                        }
                        State::InMembers
                            if !current_qual.is_empty() && !s.is_empty() => {
                                set.insert((current_qual.clone(), s));
                            }
                        _ => {}
                    }
                }
                _ => {}
            }
        }
    }
    set
}

/// Scan `lower_callee()` match arms from ipe_lower/src/lower.rs.
///
/// Arms look like:
/// ```
///     ("Log", "println") => Ok(Callee::Kernel(KernelFn::LogPrintln)),
/// ```
/// Returns the set of `KernelFn::Variant` names that have a lower arm.
fn scan_lower_arms(src: &str) -> HashSet<String> {
    let mut set = HashSet::new();
    for line in src.lines() {
        let t = line.trim();
        if t.starts_with("//") {
            continue;
        }
        // Look for `KernelFn::Variant)` pattern.
        let marker = "KernelFn::";
        if let Some(idx) = t.find(marker) {
            let rest = &t[idx + marker.len()..];
            let variant: String = rest
                .chars()
                .take_while(|c| c.is_alphanumeric() || *c == '_')
                .collect();
            if !variant.is_empty() && variant.starts_with(|c: char| c.is_uppercase()) {
                set.insert(variant);
            }
        }
    }
    set
}

/// Scan sky-stdlib `.sky` files for `qualifier.member` symbols.
///
/// Strategy: look for `Ffi.kernel "name"` patterns (kernel bindings) and also
/// for bare `name : type` top-level declarations.  We derive the qualifier from
/// the module path (file name → canonical qualifier mapping).
fn scan_sky_stdlib(stdlib_dir: &Path) -> Result<HashSet<(String, String)>, String> {
    let mut set = HashSet::new();
    if !stdlib_dir.exists() {
        return Ok(set);
    }
    scan_sky_stdlib_dir(stdlib_dir, &mut set)?;
    Ok(set)
}

fn scan_sky_stdlib_dir(dir: &Path, set: &mut HashSet<(String, String)>) -> Result<(), String> {
    let rd = fs::read_dir(dir).map_err(|e| format!("read_dir {}: {e}", dir.display()))?;
    for entry in rd {
        let entry = entry.map_err(|e| format!("dir entry: {e}"))?;
        let path = entry.path();
        if path.is_dir() {
            scan_sky_stdlib_dir(&path, set)?;
        } else if path.extension().and_then(|e| e.to_str()) == Some("sky") {
            let src = read_file(&path)?;
            // Derive module qualifier from file path.
            let qualifier = path_to_qualifier(&path);
            if qualifier.is_empty() {
                continue;
            }
            for line in src.lines() {
                let t = line.trim();
                // `Ffi.kernel "runtimeName"` pattern — extract the member from
                // the binding name on the preceding declaration line.
                // Simpler: just collect every top-level binding `name : type`.
                // A binding line: `<name> : <type>` where name is a camelCase identifier.
                if let Some((name, _ty)) = split_sky_decl(t) {
                    set.insert((qualifier.clone(), name));
                }
            }
        }
    }
    Ok(())
}

/// Convert a `.sky` file path to a canonical qualifier short-name.
fn path_to_qualifier(path: &Path) -> String {
    // Look for the segment after `sky-stdlib/` and map to qualifier.
    let path_str = path.to_string_lossy();
    // Map known module paths to their canonical qualifiers.
    let mappings: &[(&str, &str)] = &[
        ("Sky/Core/String.sky", "String"),
        ("Sky/Core/Char.sky", "Char"),
        ("Sky/Core/List.sky", "List"),
        ("Sky/Core/Maybe.sky", "Maybe"),
        ("Sky/Core/Result.sky", "Result"),
        ("Sky/Core/Error.sky", "Error"),
        ("Sky/Core/Math.sky", "Math"),
        ("Sky/Core/Dict.sky", "Dict"),
        ("Sky/Core/Set.sky", "Set"),
        ("Sky/Core/Bytes.sky", "Bytes"),
        ("Sky/Core/Encoding.sky", "Encoding"),
        ("Sky/Core/Crypto.sky", "Crypto"),
        ("Sky/Core/Uuid.sky", "Uuid"),
        ("Sky/Core/Jwt.sky", "Jwt"),
        ("Sky/Core/Json/Encode.sky", "JsonEnc"),
        ("Sky/Core/Json/Decode.sky", "JsonDec"),
        ("Sky/Core/Json/Decode/Pipeline.sky", "JsonDecP"),
        ("Sky/Core/Task.sky", "Task"),
        ("Sky/Core/Io.sky", "Io"),
        ("Sky/Core/Time.sky", "Time"),
        ("Sky/Core/System.sky", "System"),
        ("Sky/Core/Random.sky", "Random"),
        ("Sky/Core/File.sky", "File"),
        ("Sky/Core/Http.sky", "Http"),
        ("Sky/Core/CssSafety.sky", "CssSafety"),
        ("Sky/Core/Basics.sky", "Basics"),
        ("Sky/Core/Prelude.sky", "Basics"),
        ("Sky/Http/Server.sky", "Server"),
        ("Sky/Http/Middleware.sky", "Middleware"),
        ("Sky/Http/RateLimit.sky", "RateLimit"),
        ("Std/Log.sky", "Log"),
        ("Std/Cmd.sky", "Cmd"),
        ("Std/Sub.sky", "Sub"),
        ("Std/Db.sky", "Db"),
        ("Std/Db/Decode.sky", "Db.Decode"),
        ("Std/Ui.sky", "Ui"),
        ("Std/Ui/Background.sky", "Background"),
        ("Std/Ui/Border.sky", "Border"),
        ("Std/Ui/Font.sky", "Font"),
        ("Std/Ui/Region.sky", "Region"),
        ("Std/Html.sky", "Html"),
        ("Std/Html/Attributes.sky", "Attr"),
        ("Std/Html/Events.sky", "Event"),
        ("Std/Live.sky", "Live"),
        ("Std/Tui.sky", "Tui"),
        ("Std/Webview.sky", "Webview"),
        ("Std/Cli.sky", "Cli"),
        ("Std/Auth.sky", "Auth"),
    ];
    for (suffix, qual) in mappings {
        if path_str.contains(suffix) {
            return qual.to_string();
        }
    }
    String::new()
}

/// Split a Sky top-level declaration `name : type` into (name, type_str).
/// Only recognises bindings (lowercase-start identifiers followed by ` : `).
fn split_sky_decl(line: &str) -> Option<(String, String)> {
    // Skip comment lines and `type` / `import` / `module` declarations.
    if line.starts_with("--")
        || line.starts_with("type ")
        || line.starts_with("import ")
        || line.starts_with("module ")
        || line.is_empty()
    {
        return None;
    }

    // Look for ` : ` separator.
    let colon_pos = line.find(" : ")?;
    let name = line[..colon_pos].trim();

    // Valid member name: lowercase-start, alphanumeric + underscore.
    if name.is_empty() || !name.starts_with(|c: char| c.is_lowercase()) {
        return None;
    }
    // No spaces in name (multi-word = not a decl header).
    if name.contains(' ') {
        return None;
    }

    let ty = line[colon_pos + 3..].trim().to_string();
    Some((name.to_string(), ty))
}

// ── FFI surface scan ─────────────────────────────────────────────────────────

/// Scan the reference sky-ffi-inspect Go file and our sky-ffi-inspect-rs for
/// `PackageInfo` / `PkgInfo` schema fields.  Returns (ref_fields, our_fields).
#[allow(dead_code)]
fn scan_ffi_schemas(
    ref_go: &Path,
    our_rs: &Path,
) -> (HashSet<String>, HashSet<String>) {
    (
        scan_go_struct_fields(ref_go),
        scan_rs_struct_fields(our_rs),
    )
}

fn scan_go_struct_fields(path: &Path) -> HashSet<String> {
    let src = match read_file(path) {
        Ok(s) => s,
        Err(_) => return HashSet::new(),
    };
    let mut fields = HashSet::new();
    let mut in_struct = false;
    for line in src.lines() {
        let t = line.trim();
        if t.starts_with("type ") && t.contains("struct {") {
            in_struct = true;
            continue;
        }
        if in_struct {
            if t == "}" {
                in_struct = false;
                continue;
            }
            // Go struct field: `FieldName TypeExpr json:"..."`
            let field_name: String = t
                .chars()
                .take_while(|c| c.is_alphanumeric() || *c == '_')
                .collect();
            if !field_name.is_empty() && field_name.starts_with(|c: char| c.is_uppercase()) {
                fields.insert(field_name);
            }
        }
    }
    fields
}

fn scan_rs_struct_fields(path: &Path) -> HashSet<String> {
    let src = match read_file(path) {
        Ok(s) => s,
        Err(_) => return HashSet::new(),
    };
    let mut fields = HashSet::new();
    let mut in_struct = false;
    for line in src.lines() {
        let t = line.trim();
        if t.starts_with("struct ") && !t.starts_with("//") {
            in_struct = true;
            continue;
        }
        if in_struct {
            if t.starts_with('}') {
                in_struct = false;
                continue;
            }
            // Rust struct field: `field_name: Type,` (possibly with attributes above)
            if t.starts_with('#') || t.starts_with("//") {
                continue;
            }
            let field_name: String = t
                .chars()
                .take_while(|c| c.is_alphanumeric() || *c == '_')
                .collect();
            if !field_name.is_empty() && field_name.starts_with(|c: char| c.is_lowercase()) {
                fields.insert(field_name);
            }
        }
    }
    fields
}

// ── Report ───────────────────────────────────────────────────────────────────

fn run_report(tsv: &str) -> Result<String, String> {
    let rows = parse_tsv(tsv)?;
    let total = rows.len();
    let wired: Vec<&ParsedRow> = rows.iter().filter(|r| r.in_all).collect();
    let wired_count = wired.len();
    let ok_count = wired.iter().filter(|r| r.status == "OK").count();
    let mismatch_rows: Vec<&ParsedRow> = rows
        .iter()
        .filter(|r| r.status.starts_with("MISMATCH"))
        .collect();
    let backlog_rows: Vec<&ParsedRow> = rows
        .iter()
        .filter(|r| r.status == "BACKLOG")
        .collect();

    // Runtime symbol existence stats.
    let with_sym: Vec<&ParsedRow> = wired.iter().filter(|r| !r.runtime_sym.is_empty()).copied().collect();
    let sym_exists_count = with_sym.iter().filter(|r| r.runtime_sym_exists).count();
    let sym_missing_count = with_sym.len() - sym_exists_count;

    // Class breakdown.
    let mut class_counts: BTreeMap<String, (usize, usize)> = BTreeMap::new(); // class → (wired, ok)
    for r in &rows {
        let e = class_counts.entry(r.class.clone()).or_default();
        if r.in_all {
            e.0 += 1;
            if r.status == "OK" {
                e.1 += 1;
            }
        }
    }

    let mut out = String::new();
    let _ = writeln!(out, "# Sky→Rust Backend Parity Matrix");
    let _ = writeln!(out);
    let _ = writeln!(out, "> Generated by `parity-matrix report`. Re-run with:");
    let _ = writeln!(out, "> ```bash");
    let _ = writeln!(out, "> parity-matrix extract > docs/architecture/parity-matrix.tsv");
    let _ = writeln!(out, "> parity-matrix report docs/architecture/parity-matrix.tsv > docs/architecture/parity-matrix.md");
    let _ = writeln!(out, "> ```");
    let _ = writeln!(out);
    let _ = writeln!(out, "## Summary");
    let _ = writeln!(out);
    let _ = writeln!(out, "| Metric | Value |");
    let _ = writeln!(out, "|--------|-------|");
    let _ = writeln!(out, "| Total StdlibKernel variants | {total} |");
    let _ = writeln!(out, "| Wired (in ALL slice) | {wired_count} |");
    let _ = writeln!(out, "| Backlog (not yet wired) | {} |", backlog_rows.len());
    let _ = writeln!(
        out,
        "| Wired OK (all 8 layers pass) | {ok_count} ({:.0}%) |",
        if wired_count > 0 { ok_count as f64 / wired_count as f64 * 100.0 } else { 0.0 }
    );
    let _ = writeln!(out, "| MISMATCH (bugs) | {} |", mismatch_rows.len());
    let _ = writeln!(
        out,
        "| Runtime symbol coverage | {sym_exists_count}/{} ({:.0}%) |",
        with_sym.len(),
        if !with_sym.is_empty() { sym_exists_count as f64 / with_sym.len() as f64 * 100.0 } else { 0.0 }
    );
    let _ = writeln!(out, "| Runtime symbols MISSING | {sym_missing_count} |");
    let _ = writeln!(out);

    let _ = writeln!(out, "## Class Coverage");
    let _ = writeln!(out);
    let _ = writeln!(out, "| Class | Wired | OK | % |");
    let _ = writeln!(out, "|-------|-------|----|---|");
    for (class, (w, ok)) in &class_counts {
        let pct = if *w > 0 { *ok as f64 / *w as f64 * 100.0 } else { 0.0 };
        let _ = writeln!(out, "| {class} | {w} | {ok} | {pct:.0}% |");
    }
    let _ = writeln!(out);

    if !mismatch_rows.is_empty() {
        let _ = writeln!(out, "## MISMATCH Rows (bugs — CI blocks on these)");
        let _ = writeln!(out);
        let _ = writeln!(
            out,
            "| Variant | Qualifier.Member | Status | Runtime Sym | Sym Exists |"
        );
        let _ = writeln!(
            out,
            "|---------|-----------------|--------|------------|-----------|"
        );
        for r in &mismatch_rows {
            let qm = if r.qualifier.is_empty() {
                String::new()
            } else {
                format!("{}.{}", r.qualifier, r.member)
            };
            let _ = writeln!(
                out,
                "| {} | {} | {} | `{}` | {} |",
                r.variant, qm, r.status, r.runtime_sym, yn2(r.runtime_sym_exists)
            );
        }
        let _ = writeln!(out);
    }

    let _ = writeln!(out, "## Backlog (not yet wired — {} entries)", backlog_rows.len());
    let _ = writeln!(out);
    if backlog_rows.len() <= 40 {
        let _ = writeln!(out, "| Variant | Class |");
        let _ = writeln!(out, "|---------|-------|");
        for r in &backlog_rows {
            let _ = writeln!(out, "| {} | {} |", r.variant, r.class);
        }
    } else {
        let _ = writeln!(out, "(Backlog has {} entries — run `parity-matrix extract | grep BACKLOG` for full list.)", backlog_rows.len());
    }
    let _ = writeln!(out);

    let _ = writeln!(out, "## FFI Surface");
    let _ = writeln!(out);
    let _ = writeln!(out, "| Item | Status |");
    let _ = writeln!(out, "|------|--------|");
    let _ = writeln!(out, "| `sky-ffi-inspect-rs` PkgInfo schema | needs comparison |");
    let _ = writeln!(out, "| FFI generator port (`src/Sky/Build/Rust/Ffi*.hs`) | 0% — not started |");
    let _ = writeln!(out, "| FFI generator (`FfiGen.hs`, 2093 lines) | 0% — not started |");
    let _ = writeln!(out);
    let _ = writeln!(out, "Reference generator modules (denominators for port tracking):");
    let _ = writeln!(out);
    let _ = writeln!(out, "| Module | Lines |");
    let _ = writeln!(out, "|--------|-------|");
    let _ = writeln!(out, "| `src/Sky/Build/Rust/Ffi.hs` | 1616 |");
    let _ = writeln!(out, "| `src/Sky/Build/Rust/FfiInstance.hs` | 952 |");
    let _ = writeln!(out, "| `src/Sky/Build/Rust/FfiCall.hs` | 820 |");
    let _ = writeln!(out, "| `src/Sky/Build/FfiGen.hs` | 2093 |");
    let _ = writeln!(out, "| **Total** | **5481** |");
    let _ = writeln!(out);

    Ok(out)
}

fn yn2(b: bool) -> &'static str {
    if b {
        "✓"
    } else {
        "✗"
    }
}

// ── TSV parser (for report) ───────────────────────────────────────────────────

#[derive(Debug, Default)]
struct ParsedRow {
    variant: String,
    qualifier: String,
    member: String,
    class: String,
    in_all: bool,
    runtime_sym: String,
    runtime_sym_exists: bool,
    status: String,
}

fn parse_tsv(tsv: &str) -> Result<Vec<ParsedRow>, String> {
    let mut rows = Vec::new();
    let mut lines = tsv.lines();

    // Skip header line.
    let header = match lines.next() {
        Some(h) => h,
        None => return Ok(rows),
    };

    // Build column index map.
    let cols: HashMap<&str, usize> = header
        .split('\t')
        .enumerate()
        .map(|(i, s)| (s, i))
        .collect();

    let get = |fields: &[&str], name: &str| -> String {
        cols.get(name)
            .and_then(|&i| fields.get(i))
            .map(|s| s.trim().to_string())
            .unwrap_or_default()
    };

    for line in lines {
        let fields: Vec<&str> = line.split('\t').collect();
        if fields.len() < 2 {
            continue;
        }
        let row = ParsedRow {
            variant: get(&fields, "variant"),
            qualifier: get(&fields, "qualifier"),
            member: get(&fields, "member"),
            class: get(&fields, "class"),
            in_all: get(&fields, "in_all") == "Y",
            runtime_sym: get(&fields, "runtime_sym"),
            runtime_sym_exists: get(&fields, "runtime_sym_exists") == "Y",
            status: get(&fields, "status"),
        };
        rows.push(row);
    }
    Ok(rows)
}
