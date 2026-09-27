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
    /// `ipe lsp` given any arguments.
    lsp_takes_no_arguments = "lsp-takes-no-arguments";
    /// `ipe lint` given more than one path.
    lint_single_path = "lint-single-path";
    /// `ipe package audit --advisory-db` with `--no-advisory-db`.
    audit_advisory_db_exclusive = "audit-advisory-db-exclusive";
    /// `ipe package audit` given more than one path.
    audit_single_path = "audit-single-path";
    /// `ipe package audit` given a repeated output-format flag.
    audit_format_repeated = "audit-format-repeated";
    /// `ipe doc --type` with `--check-examples` or `--list`.
    doc_type_exclusive = "doc-type-exclusive";
    /// `ipe doc --type` given an unexpected positional.
    doc_type_unexpected_positional = "doc-type-unexpected-positional";
    /// `ipe doc serve --port` without a value.
    doc_serve_port_needs_number = "doc-serve-port-needs-number";
    /// `ipe doc <key>` given more than one key.
    doc_single_key = "doc-single-key";
    /// `ipe doc` given more than one path.
    doc_single_path = "doc-single-path";
    /// `ipe add`'s inspector binary is missing.
    ffi_inspector_not_found = "ffi-inspector-not-found";
    /// `ipe add` cannot make a safe scratch directory without `HOME`.
    ffi_add_home_unset = "ffi-add-home-unset";
    /// `ipe add` has no bubblewrap isolation available.
    ffi_no_bubblewrap = "ffi-no-bubblewrap";
    /// `ipe add`'s inspector payload was empty.
    ffi_add_no_payload = "ffi-add-no-payload";
    /// A confirmation prompt was declined.
    install_aborted = "install-aborted";
    /// A legacy `[[rust.define.*]]` TOML table is no longer supported.
    ffi_legacy_define_removed = "ffi-legacy-define-removed";
    /// Bare `ipe rust`.
    rust_usage = "rust-usage";
    /// `ipe rust add` misuse.
    rust_add_usage = "rust-add-usage";
    /// `ipe rust add`'s confirmation prompt was declined.
    rust_add_aborted = "rust-add-aborted";
    /// `ipe rust remove` misuse.
    rust_remove_usage = "rust-remove-usage";
    /// `ipe rust install` misuse.
    rust_install_usage = "rust-install-usage";
    /// `ipe rust install` found only a `package.ipe` manifest.
    rust_install_package_ipe_unsupported = "rust-install-package-ipe-unsupported";
    /// `ipe rust install` found no legacy manifest.
    rust_install_no_manifest = "rust-install-no-manifest";
    /// `ipe add`/`ipe remove` outside a package.
    pkg_no_manifest = "pkg-no-manifest";
    /// `ipe package publish` given more than one path.
    publish_single_path = "publish-single-path";
    /// `ipe watch`/`ipe build` given a directory with no manifest inside it.
    watch_dir_no_manifest = "watch-dir-no-manifest";
    /// `ipe diff` misuse.
    diff_usage = "diff-usage";
    /// `ipe init --shape` without its value.
    init_shape_needs_value = "init-shape-needs-value";
    /// A directory carries a legacy `ipe.toml` but no `package.ipe`.
    legacy_toml_hint = "legacy-toml-hint";
    /// `build`/`run`/`watch` found no entry and none could be discovered.
    no_entry = "no-entry";
    /// The entry module was not found in the source map (an internal invariant).
    internal_entry_not_in_source_map = "internal-entry-not-in-source-map";
    /// A module in topo order was not found in the source map (an internal invariant).
    internal_module_not_in_source_map = "internal-module-not-in-source-map";
    /// A library package (only `exposedModules`) has no runnable entry.
    library_package_no_entry = "library-package-no-entry";
    /// A packager could not find a `package.ipe` from the given root.
    pkg_not_found_in_dir = "pkg-not-found-in-dir";
    /// Bare `ipe package`.
    package_usage = "package-usage";
    /// `ipe package validate-entry` without its entry-file path.
    package_validate_entry_usage = "package-validate-entry-usage";
    /// `ipe package audit-entry` given more than one path.
    package_audit_entry_single_path = "package-audit-entry-single-path";
    /// `ipe package audit-entry` without its entry-file path.
    package_audit_entry_usage = "package-audit-entry-usage";
    /// No module in the package could be lowered for capability inference.
    package_capability_inference_failed = "package-capability-inference-failed";
    /// `package.ipe` with no `name` field.
    package_manifest_name_required = "package-manifest-name-required";
    /// `package.ipe`'s source root does not exist.
    package_manifest_src_root_missing = "package-manifest-src-root-missing";
    /// `package.ipe` with no top-level `package` binding.
    package_manifest_no_package_binding = "package-manifest-no-package-binding";
    /// `ipe add`/`ipe remove` found no top-level `package` binding to edit.
    package_manifest_no_package_binding_edit = "package-manifest-no-package-binding-edit";
    /// `ipe add`/`ipe remove` found a non-record `package` value.
    package_manifest_package_not_record = "package-manifest-package-not-record";
    /// `ipe add`/`ipe remove` found a non-list `dependencies` field.
    package_manifest_deps_not_list = "package-manifest-deps-not-list";
    /// `ipe add` could not locate the `package` record's closing brace.
    package_manifest_deps_brace_not_found = "package-manifest-deps-brace-not-found";
    /// `ipe add` found the `package` record's closing brace out of range.
    package_manifest_deps_brace_out_of_range = "package-manifest-deps-brace-out-of-range";
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
    /// The heading of a help page's argument description.
    help_arguments_label = "help-arguments-label";
    /// The heading of a help page's option list.
    help_options_label = "help-options-label";
    /// The heading of a help page's output-location note.
    help_output_label = "help-output-label";
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
    /// The CLI positional shape disagrees with `main`'s own shape.
    delivery_shape_mismatch(stated, pinned) = "delivery-shape-mismatch";
    /// The literal token `served` was written.
    delivery_served_not_a_word = "delivery-served-not-a-word";
    /// A runtime word was given for a non-web shape.
    delivery_runtime_on_non_web(shape) = "delivery-runtime-on-non-web";
    /// A host word was given for a non-web shape.
    delivery_host_on_non_web(shape, host) = "delivery-host-on-non-web";
    /// A mobile host was given for the served runtime.
    delivery_served_host_not_mobile(host) = "delivery-served-host-not-mobile";
    /// `--static` was requested for `web desktop` (a webview host).
    delivery_static_not_allowed_webview = "delivery-static-not-allowed-webview";
    /// `--static` was requested for a delivery with no static form.
    delivery_static_not_allowed(delivery) = "delivery-static-not-allowed";
    /// An unknown token appeared where a runtime or host was expected.
    delivery_unknown_token(got) = "delivery-unknown-token";
    /// A runtime or host token was given more than once.
    delivery_duplicate_token(kind, got) = "delivery-duplicate-token";
    /// A `solo` delivery resolved to a native compile target.
    delivery_solo_requires_wasm_target = "delivery-solo-requires-wasm-target";
    /// A wasm compile target resolved without a `solo` delivery.
    delivery_wasm_target_requires_solo = "delivery-wasm-target-requires-solo";
    /// A WASM triple was requested for the native engine.
    delivery_native_engine_refuses_wasm_triple(triple) = "delivery-native-engine-refuses-wasm-triple";
    /// A `web solo` client asked for a non-browser triple.
    delivery_solo_requires_browser_triple(triple) = "delivery-solo-requires-browser-triple";
    /// A `web solo` client asked for the WASI triple.
    delivery_solo_refuses_wasi_triple = "delivery-solo-refuses-wasi-triple";
    /// A musl static triple was requested for a webview-native delivery.
    delivery_webview_has_no_static_triple(delivery) = "delivery-webview-has-no-static-triple";
    /// A co-located WASI build was asked to carry a `solo` delivery.
    delivery_wasi_refuses_solo_delivery = "delivery-wasi-refuses-solo-delivery";
    /// A co-located WASI build was asked for a non-`Direct` shape.
    delivery_wasi_requires_direct_shape(shape) = "delivery-wasi-requires-direct-shape";
    /// A co-located WASI engine was asked for a non-WASI triple.
    delivery_wasi_requires_wasi_triple(triple) = "delivery-wasi-requires-wasi-triple";
    /// A static-build request was refused.
    cli_static_refusal(refusal) = "cli-static-refusal";
    /// The Ipe runtime module tree could not be located.
    cli_runtime_not_found = "cli-runtime-not-found";
    /// `$IPE_RUNTIME_DIR` does not name a runtime crate root.
    cli_runtime_dir_invalid(path) = "cli-runtime-dir-invalid";
    /// The invalid runtime dir looks like the inner module directory.
    cli_runtime_dir_invalid_inner_hint = "cli-runtime-dir-invalid-inner-hint";
    /// No directory could be resolved to materialize the embedded runtime.
    cli_runtime_home_unknown = "cli-runtime-home-unknown";
    /// Writing the embedded runtime source failed.
    cli_runtime_materialize_failed(detail) = "cli-runtime-materialize-failed";
    /// The resolved runtime crate declares a different version than the compiler.
    cli_runtime_version_mismatch(path, found, expected) = "cli-runtime-version-mismatch";
    /// `cargo` failed because the runtime lacks a feature the emitted project needs.
    cli_emitted_build_feature_missing(what, feature) = "cli-emitted-build-feature-missing";
    /// The runtime root/version context appended to a feature-gap message.
    cli_emitted_build_feature_context(root, version) = "cli-emitted-build-feature-context";
    /// The stale-runtime remediation hint appended to a feature-gap message.
    cli_emitted_build_stale_runtime_hint = "cli-emitted-build-stale-runtime-hint";
    /// `cargo` could not reach the registry while fetching crates.
    cli_cargo_fetch_failed(code, what) = "cli-cargo-fetch-failed";
    /// `cargo` could not reach the registry while fetching crates, with detail.
    cli_cargo_fetch_failed_detail(code, what, trimmed) = "cli-cargo-fetch-failed-detail";
    /// `cargo` produced no output while failing to compile the emitted project.
    cli_cargo_compile_failed(code, what) = "cli-cargo-compile-failed";
    /// `cargo` failed to compile the emitted project, with detail.
    cli_cargo_compile_failed_detail(code, what, trimmed) = "cli-cargo-compile-failed-detail";
    /// The declared-vs-inferred capability mismatch headline.
    cli_capability_mismatch_header = "cli-capability-mismatch-header";
    /// The used-but-not-declared capability list line.
    cli_capability_mismatch_missing(list) = "cli-capability-mismatch-missing";
    /// The declared-but-not-used capability list line.
    cli_capability_mismatch_extra(list) = "cli-capability-mismatch-extra";
    /// A fetched package's content hash did not match the index's pinned hash.
    cli_hash_mismatch(package, expected, actual) = "cli-hash-mismatch";
    /// `ipe doc <query>` named no documentation entry.
    cli_doc_not_found(query) = "cli-doc-not-found";
    /// The header over a doc-not-found suggestion list.
    cli_doc_suggestions_header = "cli-doc-suggestions-header";
    /// One suggested documentation entry.
    cli_doc_suggestion_line(key, title, kind) = "cli-doc-suggestion-line";
    /// `ipe explain <CODE>` was given a string that is not a taxonomy code.
    cli_unknown_code(input) = "cli-unknown-code";
    /// The first suggested code for an unknown-code error.
    cli_unknown_code_did_you_mean(first) = "cli-unknown-code-did-you-mean";
    /// The verify mode found the proposed version does not clear the required floor.
    cli_semver_rejected(required, floor, proposed) = "cli-semver-rejected";
    /// `ipe package publish` declined to proceed.
    cli_publish_refused(refusal) = "cli-publish-refused";
    /// A command group was followed by a token that is not one of its verbs.
    cli_unknown_group_verb(group, attempted) = "cli-unknown-group-verb";
    /// The near-miss suggestion offered for an unknown group verb.
    cli_unknown_group_suggestion(group, sugg) = "cli-unknown-group-suggestion";
    /// A stage of `ipe verify` failed.
    cli_verify_failed(stage) = "cli-verify-failed";
    /// The project's test runner exited non-zero.
    cli_test_failed_suffix(code) = "cli-test-failed-suffix";
    /// `ipe upgrade` could not find a prebuilt binary for the requested version.
    cli_upgrade_no_prebuilt(glyph, version, platform) = "cli-upgrade-no-prebuilt";
    /// `ipe health` found a critical prerequisite missing.
    cli_health_critical = "cli-health-critical";
    /// `ipe eject` was asked to eject a program it cannot make self-contained.
    cli_eject_unsupported(reason) = "cli-eject-unsupported";
    /// `ipe lint` found one or more findings at or above the gate severity.
    cli_lint_gate_failed = "cli-lint-gate-failed";
    /// A file exceeded the per-surface read ceiling.
    cli_file_too_large(path, max) = "cli-file-too-large";
    /// A manifest path escaped the project directory.
    cli_path_escape(raw, reason) = "cli-path-escape";
    /// A build-output location was refused.
    cli_output_refused(refusal) = "cli-output-refused";
    /// The module-discovery walk hit its depth ceiling or a symlink cycle.
    cli_discovery_limit_reached(detail) = "cli-discovery-limit-reached";
    /// A locked dependency falls within an advisory's affected range.
    cli_advisory_vulnerable(package, version, severity, id, description, fixed_in) =
        "cli-advisory-vulnerable";
    /// The fixed-in-version line appended to an advisory message.
    cli_advisory_fixed_in(v) = "cli-advisory-fixed-in";
    /// An advisory DB file could not be read.
    cli_advisory_db_unreachable(detail) = "cli-advisory-db-unreachable";
    /// An advisory DB file was present but malformed.
    cli_advisory_db_malformed(path, detail) = "cli-advisory-db-malformed";
    /// `ipe run --target wasi` on a binary built without the `wasi_run` feature.
    cli_wasi_run_feature_disabled = "cli-wasi-run-feature-disabled";
    /// The embedded wasmtime engine could not run the emitted WASI module.
    cli_wasi_run_failed(detail) = "cli-wasi-run-failed";
    /// The emitted WASI module ran to completion with a non-zero exit code.
    cli_wasi_run_exited(code) = "cli-wasi-run-exited";
    /// The "unknown command" line shown above the top-level help screen.
    cli_unknown_command_line(attempted) = "cli-unknown-command-line";
    /// The near-miss suggestion offered for an unknown command.
    cli_unknown_command_suggestion(sugg) = "cli-unknown-command-suggestion";
    /// A missing-file `Io` error.
    cli_io_not_found(path) = "cli-io-not-found";
    /// A non-missing-file `Io` error.
    cli_io_other(path, kind) = "cli-io-other";
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

    /// Every `.rs` file under this crate's `src/`, skipping hidden directories
    /// and `target/`.
    fn rs_files_under(root: &std::path::Path) -> Vec<std::path::PathBuf> {
        let mut out = Vec::new();
        let mut stack = vec![root.to_owned()];
        while let Some(dir) = stack.pop() {
            let Ok(entries) = std::fs::read_dir(&dir) else {
                continue;
            };
            for entry in entries.flatten() {
                let path = entry.path();
                if path.is_dir() {
                    let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
                    if !name.starts_with('.') && name != "target" {
                        stack.push(path);
                    }
                } else if path.extension().and_then(|e| e.to_str()) == Some("rs") {
                    out.push(path);
                }
            }
        }
        out
    }

    /// Whether `src` calls `CliError::Usage` with a literal string — the exact
    /// defect class this catalog exists to close (a message spelled at the call
    /// site instead of declared here). A `CliError::UsageOwned`/variable/function
    /// argument is untouched: only a literal `"` immediately after `Usage(`
    /// (across any whitespace) counts.
    fn has_literal_usage_call(src: &str) -> bool {
        let mut rest = src;
        while let Some(at) = rest.find("CliError::Usage(") {
            let after = rest[at.saturating_add("CliError::Usage(".len())..].trim_start();
            if after.starts_with('"') {
                return true;
            }
            rest = &rest[at.saturating_add("CliError::Usage(".len())..];
        }
        false
    }

    /// No call site outside this catalog spells a `CliError::Usage` message as a
    /// Rust string literal — every message is declared once above and reached
    /// through its `text::` function, so the rendered text and its catalog entry
    /// cannot drift. Test modules (`#[cfg(test)]`, and anything under a `tests/`
    /// directory) are exempt: a test fixture is not user-facing text.
    #[test]
    fn no_literal_cli_error_usage_outside_the_catalog() {
        let src_root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        let mut offenders = Vec::new();
        for path in rs_files_under(&src_root) {
            if path.file_name().and_then(|n| n.to_str()) == Some("text.rs") {
                continue;
            }
            if path.components().any(|c| c.as_os_str() == "tests") {
                continue;
            }
            let Ok(src) = std::fs::read_to_string(&path) else {
                continue;
            };
            // Only the production prefix counts — an inline `#[cfg(test)] mod
            // tests { ... }` block at the end of an otherwise-production file is
            // exempt, same as a dedicated `tests/` file.
            let production = src.split("#[cfg(test)]").next().unwrap_or(&src);
            if has_literal_usage_call(production) {
                offenders.push(path.display().to_string());
            }
        }
        assert!(
            offenders.is_empty(),
            "CliError::Usage(\"...\") outside text.rs — declare the message in \
             text/messages.md and call its text::fn instead: {offenders:?}"
        );
    }
}
