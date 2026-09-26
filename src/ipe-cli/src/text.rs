//! The `.md` message catalog — the single source of the CLI's user-facing messages.
//!
//! `text/messages.md` holds every message as a `## <key>` section whose body is
//! the message; `{name}` marks a value filled in when it is shown. Rust never
//! spells a message: it calls the function declared for the key below, whose
//! parameters are exactly the message's placeholders, so a call site supplies
//! the values the text interpolates and nothing else. The catalog tests pin every
//! declaration to a section with exactly its placeholders, and every section to a
//! declaration, so the text and its callers cannot drift. Edit the `.md`, never a
//! Rust string.

use std::fmt::{self, Write as _};

/// The message catalog.
pub const CATALOG: &str = include_str!("../text/messages.md");

/// The body of the catalog's `## <key>` section, without its blank edge lines.
///
/// The body runs to the next `#` or `##` heading. `None` when the catalog has no
/// such section.
#[must_use]
pub fn template(key: &str) -> Option<&'static str> {
    let heading = format!("\n## {key}\n");
    let start = CATALOG.find(&heading)?.saturating_add(heading.len());
    let body = CATALOG.get(start..)?;
    let end = [body.find("\n# "), body.find("\n## ")]
        .into_iter()
        .flatten()
        .min()
        .unwrap_or(body.len());
    body.get(..end).map(|body| body.trim_matches('\n'))
}

/// Fill `template`'s `{name}` placeholders from `args`.
///
/// Brace text that names no argument is kept as written.
#[must_use]
pub fn fill(template: &str, args: &[(&str, &dyn fmt::Display)]) -> String {
    let mut out = String::with_capacity(template.len());
    let mut rest = template;
    while let Some(open) = rest.find('{') {
        out.push_str(rest.get(..open).unwrap_or(""));
        let from_brace = rest.get(open..).unwrap_or("");
        let after = from_brace.get(1..).unwrap_or("");
        let filled = after.find('}').and_then(|close| {
            let name = after.get(..close)?;
            let (_, value) = args.iter().find(|(arg, _)| *arg == name)?;
            Some((close, *value))
        });
        let Some((close, value)) = filled else {
            out.push('{');
            rest = after;
            continue;
        };
        let _ = write!(out, "{value}");
        rest = after.get(close.saturating_add(1)..).unwrap_or("");
    }
    out.push_str(rest);
    out
}

/// The placeholder names `template` interpolates, in order of first use.
///
/// A placeholder is `{name}` where `name` is lowercase letters, digits, and `_`,
/// starting with a letter.
#[must_use]
pub fn placeholders(template: &str) -> Vec<&str> {
    let mut names: Vec<&str> = Vec::new();
    let mut rest = template;
    while let Some(open) = rest.find('{') {
        let after = rest.get(open.saturating_add(1)..).unwrap_or("");
        let name = after.find('}').and_then(|close| after.get(..close));
        if let Some(name) = name.filter(|name| is_placeholder_name(name))
            && !names.contains(&name)
        {
            names.push(name);
        }
        rest = after;
    }
    names
}

/// Whether `name` is a well-formed placeholder name.
fn is_placeholder_name(name: &str) -> bool {
    name.chars()
        .next()
        .is_some_and(|first| first.is_ascii_lowercase())
        && name
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')
}

/// Declare one catalog message as a function.
///
/// A message without placeholders is its `&'static str` text; one with
/// placeholders takes one value per placeholder and returns the filled text.
macro_rules! message_fn {
    ($(#[$meta:meta])* $name:ident = $key:literal) => {
        $(#[$meta])*
        #[must_use]
        pub fn $name() -> &'static str {
            template($key).unwrap_or($key)
        }
    };
    ($(#[$meta:meta])* $name:ident($($param:ident),+) = $key:literal) => {
        $(#[$meta])*
        #[must_use]
        pub fn $name($($param: &dyn fmt::Display),+) -> String {
            fill(template($key).unwrap_or($key), &[$((stringify!($param), $param)),+])
        }
    };
}

/// Declare the catalog's messages: a function per message, plus [`DECLARED`].
macro_rules! messages {
    ($($(#[$meta:meta])* $name:ident $(($($param:ident),+))? = $key:literal;)*) => {
        $(message_fn!($(#[$meta])* $name $(($($param),+))? = $key);)*

        /// Every declared message: its catalog key and its placeholders.
        pub const DECLARED: &[(&str, &[&str])] = &[$(($key, &[$($(stringify!($param)),+)?])),*];
    };
}

messages! {
    /// A command's refusal, behind the `ipe <command>:` prefix.
    command_refusal(command, reason) = "command-refusal";
    /// A command was given a flag it does not recognise.
    unknown_flag(command, flag) = "unknown-flag";
    /// A parent command was given a subcommand it does not recognise.
    unknown_subcommand(command, sub, expected) = "unknown-subcommand";
    /// A command was given a positional it does not take.
    unexpected_argument(command, arg) = "unexpected-argument";
    /// A single-valued flag was given twice.
    flag_repeated(command, flag) = "flag-repeated";
    /// `--plain` and `--json` were both given.
    plain_json_exclusive(command) = "plain-json-exclusive";
    /// A value-taking flag ended the command line.
    flag_needs_value(command, flag) = "flag-needs-value";
    /// A `--target` value outside the vocabulary.
    unsupported_target(target, supported) = "unsupported-target";
    /// `--static` / `--allocator` with a wasm `--target`.
    static_flags_with_wasm(target) = "static-flags-with-wasm";
    /// `--cfree` with a wasm `--target`.
    cfree_with_wasm(target) = "cfree-with-wasm";
    /// `--emit-ir` with `--out`.
    emit_ir_with_out = "emit-ir-with-out";
    /// `--emit-ir` with `--static`.
    emit_ir_with_static = "emit-ir-with-static";
    /// `--emit-ir` with `--target`.
    emit_ir_with_target = "emit-ir-with-target";
    /// `--emit-ir` with `--allocator`.
    emit_ir_with_allocator = "emit-ir-with-allocator";
    /// `--emit-ir` with `--cfree`.
    emit_ir_with_cfree = "emit-ir-with-cfree";
    /// `ipe run --target wasm`, which has no native artifact.
    run_wasm_target = "run-wasm-target";
    /// `ipe run --target wasi` with a native-only flag.
    run_wasi_native_flags = "run-wasi-native-flags";
    /// `ipe eject` without `--out`.
    eject_out_required = "eject-out-required";
    /// `ipe release --target wasi`.
    release_no_wasi = "release-no-wasi";
    /// `ipe release --embed --bundle`.
    release_embed_bundle_exclusive = "release-embed-bundle-exclusive";
    /// `--port 0`.
    port_zero(command) = "port-zero";
    /// A non-numeric `--port`.
    port_invalid(command, value) = "port-invalid";
    /// `ipe fix` without its path.
    fix_usage = "fix-usage";
    /// `ipe health --yes` with a data form.
    health_yes_with_format = "health-yes-with-format";
    /// `ipe fmt` with two paths.
    fmt_single_path = "fmt-single-path";
    /// `ipe fmt --stdin` with a path.
    fmt_stdin_and_path = "fmt-stdin-and-path";
    /// `ipe fmt --stdin` with a data form.
    fmt_format_with_stdin = "fmt-format-with-stdin";
    /// `ipe fmt` with a data form but no `--check`.
    fmt_format_needs_check = "fmt-format-needs-check";
    /// A bare delivery word that also names a path on disk.
    delivery_word_shadows_path(word) = "delivery-word-shadows-path";
    /// The label over a command group's verb list.
    verbs_label = "verbs-label";
    /// An allocator name outside the closed set.
    unknown_allocator(allocator) = "unknown-allocator";
    /// A triple that is not a supported static target.
    unknown_static_target(target, supported) = "unknown-static-target";
    /// `--target` without `--static`.
    target_requires_static(target) = "target-requires-static";
    /// A non-default allocator for a dynamic build.
    allocator_requires_static(allocator) = "allocator-requires-static";
    /// The talc allocator, not yet wired.
    talc_requires_arena_design = "talc-requires-arena-design";
    /// A webview app asked to build static.
    webview_static = "webview-static";
    /// The rustup target is not installed.
    target_not_installed(triple) = "target-not-installed";
    /// No musl-capable C compiler for the triple.
    musl_c_compiler_missing(triple, triple_env) = "musl-c-compiler-missing";
    /// `mimalloc` with `--cfree`.
    mimalloc_requires_c(allocator) = "mimalloc-requires-c";
    /// The libc allocator with `--cfree`.
    libc_allocator_requires_c(allocator) = "libc-allocator-requires-c";
    /// `--cfree`, not yet wired.
    cfree_not_yet_wired = "cfree-not-yet-wired";
    /// A malformed boolean request value.
    invalid_bool(source, value) = "invalid-bool";
    /// Publish from a dirty working tree.
    publish_dirty_tree(source_root) = "publish-dirty-tree";
    /// Publish of an unpushed HEAD.
    publish_unpushed_head(rev) = "publish-unpushed-head";
    /// Publish of an already published version.
    publish_duplicate_version(name, version) = "publish-duplicate-version";
    /// Publish without a determinable source URL.
    publish_no_source = "publish-no-source";
    /// Publish without a commit-signing key.
    publish_unsigned_commit = "publish-unsigned-commit";
    /// Publish without a resolvable GitHub identity.
    publish_unresolvable_identity = "publish-unresolvable-identity";
    /// The documentation site's skip-to-content link.
    site_skip_link = "site-skip-link";
    /// The accessible name of the site navigation.
    site_nav_label = "site-nav-label";
    /// The site title, full form.
    site_title_full = "site-title-full";
    /// The site title, short (mobile) form.
    site_title_short = "site-title-short";
    /// The accessible name of the mobile menu toggle.
    site_menu_label = "site-menu-label";
    /// The Guides section.
    site_guides = "site-guides";
    /// The Topics section.
    site_topics = "site-topics";
    /// The Idioms section.
    site_idioms = "site-idioms";
    /// The Constructs section.
    site_constructs = "site-constructs";
    /// The Reference section.
    site_reference = "site-reference";
    /// The Diagnostics section.
    site_diagnostics = "site-diagnostics";
    /// The CLI section.
    site_cli = "site-cli";
    /// The site landing page.
    site_documentation = "site-documentation";
    /// The search box placeholder.
    site_search_placeholder = "site-search-placeholder";
    /// The search box accessible name.
    site_search_label = "site-search-label";
    /// The search results accessible name.
    site_search_results_label = "site-search-results-label";
    /// The theme toggle accessible name.
    site_theme_toggle_label = "site-theme-toggle-label";
    /// The scroll-to-top button accessible name.
    site_scroll_top_label = "site-scroll-top-label";
    /// The module filter placeholder.
    site_filter_modules = "site-filter-modules";
    /// The module filter accessible name.
    site_filter_modules_label = "site-filter-modules-label";
    /// The project modules group.
    site_project_modules = "site-project-modules";
    /// The standard library group.
    site_standard_library = "site-standard-library";
    /// A module page's types heading.
    site_types = "site-types";
    /// A module page's values heading.
    site_values = "site-values";
    /// An entry with no body.
    site_no_documentation = "site-no-documentation";
    /// The landing page's pointer to the reference when no guide exists (HTML).
    site_reference_fallback = "site-reference-fallback";
    /// The diagnostics page's key to the code letters (HTML).
    site_code_families_intro = "site-code-families-intro";
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every `## <key>` the catalog defines.
    fn catalog_keys() -> Vec<&'static str> {
        CATALOG
            .lines()
            .filter_map(|line| line.trim_end().strip_prefix("## "))
            .collect()
    }

    #[test]
    fn every_declared_message_has_its_section_with_exactly_its_placeholders() {
        for (key, params) in DECLARED {
            let body = template(key);
            assert!(body.is_some_and(|b| !b.is_empty()), "no text for `{key}`");
            let mut found = placeholders(body.unwrap_or(""));
            found.sort_unstable();
            let mut declared = params.to_vec();
            declared.sort_unstable();
            assert_eq!(found, declared, "`{key}` placeholders drifted");
        }
    }

    #[test]
    fn every_section_is_declared_exactly_once() {
        let keys = catalog_keys();
        for (i, key) in keys.iter().enumerate() {
            assert!(
                !keys.iter().skip(i.saturating_add(1)).any(|k| k == key),
                "`{key}` is defined twice"
            );
            assert!(
                DECLARED.iter().any(|(declared, _)| declared == key),
                "`{key}` has no declaration"
            );
        }
        for (i, (key, _)) in DECLARED.iter().enumerate() {
            assert!(
                !DECLARED
                    .iter()
                    .skip(i.saturating_add(1))
                    .any(|(k, _)| k == key),
                "`{key}` is declared twice"
            );
        }
    }

    #[test]
    fn a_section_body_stops_at_the_next_heading() {
        assert_eq!(
            template("emit-ir-with-out"),
            Some("--emit-ir does not compose with --out")
        );
        assert_eq!(template("verbs-label"), Some("Verbs:"));
        assert_eq!(template("no-such-message"), None);
    }

    #[test]
    fn fill_replaces_named_placeholders_and_keeps_other_braces() {
        let args: [(&str, &dyn fmt::Display); 2] = [("x", &1), ("y", &"two")];
        assert_eq!(fill("a {x} b {y} {z} {", &args), "a 1 b two {z} {");
        assert_eq!(
            unknown_flag(&"build", &"--nope"),
            "ipe build: unknown flag `--nope`"
        );
        assert_eq!(placeholders("{a} {b_2} {a} {Nope} {}"), vec!["a", "b_2"]);
    }
}
