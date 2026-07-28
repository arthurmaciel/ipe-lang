//! Static-compilation gates (design: `docs/architecture/static-compilation.md`).
//!
//! Ungated tests assert the emitted-project SHAPE of a static plan (allocator
//! feature spliced into the manifest, generated `.cargo/config.toml`, stale-
//! config hygiene, CLI refusal wiring). The full proof — the emitted crate
//! cargo-builds for `x86_64-unknown-linux-musl`, `ldd` reports it static, and
//! it runs — is gated behind `IPE_E2E_STATIC=1` (it needs the musl target, a
//! musl-capable C compiler, and a cold multi-minute dependency build).

use std::path::{Path, PathBuf};

use ipe::{BuildOptions, CliError, build_plan};
use ipe_backend_rust::static_build::{
    CARGO_CONFIG_MARKER, StaticAllocator, StaticPlan, StaticTriple,
};

/// The `ipe-lang` workspace root (two levels up from this crate's manifest).
fn repo_root() -> PathBuf {
    let joined = Path::new(env!("CARGO_MANIFEST_DIR")).join("..").join("..");
    std::fs::canonicalize(&joined).unwrap_or(joined)
}

fn write_hello(dir: &Path) -> std::io::Result<PathBuf> {
    std::fs::create_dir_all(dir)?;
    let entry = dir.join("Main.ipe");
    std::fs::write(
        &entry,
        "module Main exposing (main)\n\
         import Ipe.Prelude exposing (..)\n\
         import Ipe.Io as Io\n\
         \n\
         main =\n    Io.println \"static hello\"\n",
    )?;
    Ok(entry)
}

const fn dlmalloc_plan() -> StaticPlan {
    StaticPlan {
        triple: StaticTriple::X8664LinuxMusl,
        allocator: StaticAllocator::Dlmalloc,
    }
}

fn default_line(manifest: &str) -> String {
    manifest
        .lines()
        .find(|l| l.starts_with("default = ["))
        .unwrap_or("")
        .to_owned()
}

/// A static build emits the allocator feature + the generated cargo config;
/// a subsequent dynamic build of the SAME out-dir restores the baseline
/// byte-identically and removes the generated config (stale-config hygiene —
/// `+crt-static` must never leak into later dynamic builds).
#[test]
fn static_emit_activates_dlmalloc_and_dynamic_rebuild_restores_baseline() {
    let scratch = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("static_emit_shape");
    let _ = std::fs::remove_dir_all(&scratch);
    let entry = write_hello(&scratch.join("srcdir")).expect("write hello source");
    let out = scratch.join("out");
    let runtime = ipe::resolve_runtime().expect("runtime must resolve");

    let statik = BuildOptions {
        static_plan: Some(dlmalloc_plan()),
        ..BuildOptions::default()
    };
    ipe::build_with_options(&entry, &out, &runtime, statik).expect("static build");

    let manifest = std::fs::read_to_string(out.join("Cargo.toml")).expect("emitted manifest");
    let def = default_line(&manifest);
    assert!(def.contains(r#""alloc_dlmalloc""#), "{def}");
    assert_eq!(
        def.matches("alloc_").count(),
        1,
        "exactly one allocator: {def}"
    );

    let config_path = out.join(".cargo").join("config.toml");
    let config = std::fs::read_to_string(&config_path).expect("generated cargo config");
    assert!(config.starts_with(CARGO_CONFIG_MARKER));
    assert!(config.contains("[target.x86_64-unknown-linux-musl]"));
    assert!(config.contains(r#""target-feature=+crt-static""#));
    assert!(!config.contains("target-dir"));

    // Dynamic rebuild of the same out-dir: baseline restored, config gone.
    ipe::build_with_options(&entry, &out, &runtime, BuildOptions::default())
        .expect("dynamic rebuild");
    let manifest = std::fs::read_to_string(out.join("Cargo.toml")).expect("emitted manifest");
    assert!(
        !default_line(&manifest).contains("alloc_"),
        "dynamic default build must not activate an allocator"
    );
    assert!(
        !config_path.exists(),
        "the generated static config must be removed by a dynamic rebuild"
    );
}

/// A hand-written (non-generated) `.cargo/config.toml` is never touched by
/// the hygiene pass — only files starting with the generated marker are ours
/// to delete.
#[test]
fn dynamic_build_leaves_user_cargo_config_alone() {
    let scratch = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("static_emit_user_config");
    let _ = std::fs::remove_dir_all(&scratch);
    let entry = write_hello(&scratch.join("srcdir")).expect("write hello source");
    let out = scratch.join("out");
    let runtime = ipe::resolve_runtime().expect("runtime must resolve");

    let config_path = out.join(".cargo").join("config.toml");
    std::fs::create_dir_all(out.join(".cargo")).expect("mk .cargo");
    let user_config = "# hand-written by a user\n[net]\noffline = false\n";
    std::fs::write(&config_path, user_config).expect("write user config");

    ipe::build_with_options(&entry, &out, &runtime, BuildOptions::default())
        .expect("dynamic build");
    let after = std::fs::read_to_string(&config_path).expect("user config must survive");
    assert_eq!(after, user_config);
}

/// The mimalloc opt-in splices its own feature.
#[test]
fn static_emit_mimalloc_optin_activates_mimalloc() {
    let scratch = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("static_emit_mimalloc");
    let _ = std::fs::remove_dir_all(&scratch);
    let entry = write_hello(&scratch.join("srcdir")).expect("write hello source");
    let out = scratch.join("out");
    let runtime = ipe::resolve_runtime().expect("runtime must resolve");

    let statik = BuildOptions {
        static_plan: Some(StaticPlan {
            triple: StaticTriple::X8664LinuxMusl,
            allocator: StaticAllocator::Mimalloc,
        }),
        ..BuildOptions::default()
    };
    ipe::build_with_options(&entry, &out, &runtime, statik).expect("static build");
    let manifest = std::fs::read_to_string(out.join("Cargo.toml")).expect("emitted manifest");
    let def = default_line(&manifest);
    assert!(def.contains(r#""alloc_mimalloc""#), "{def}");
    assert_eq!(def.matches("alloc_").count(), 1, "{def}");
}

/// CLI flag refusals fire before any compilation or filesystem write.
#[test]
fn cli_refusals_are_typed_and_artifact_free() {
    // Unknown allocator: the closed `--allocator` enum is parsed at the CLI
    // boundary, so an unknown name is a typed command-usage refusal there — never
    // reaching the build plan. The dispatcher wraps it as `CommandUsage` so the
    // caller shows `build`'s help; the reason still names the bad allocator.
    let err = ipe::run_cli(&[
        "build".into(),
        "NoSuch.ipe".into(),
        "--static".into(),
        "--allocator".into(),
        "jemalloc".into(),
    ])
    .expect_err("unknown allocator must refuse");
    assert!(
        matches!(&err, CliError::CommandUsage { command: "build", reason } if reason.contains("jemalloc")),
        "got: {err:?}"
    );

    // --target without --static.
    let err = ipe::run_cli(&[
        "build".into(),
        "NoSuch.ipe".into(),
        "--target".into(),
        "x86_64-unknown-linux-musl".into(),
    ])
    .expect_err("--target without --static must refuse");
    assert!(
        matches!(
            err,
            CliError::StaticRefusal(build_plan::Refusal::TargetRequiresStatic { .. })
        ),
        "wrong refusal"
    );

    // Unsupported static target.
    let err = ipe::run_cli(&[
        "build".into(),
        "NoSuch.ipe".into(),
        "--static".into(),
        "--target".into(),
        "x86_64-apple-darwin".into(),
    ])
    .expect_err("mac static must refuse");
    assert!(
        matches!(
            err,
            CliError::StaticRefusal(build_plan::Refusal::UnknownStaticTarget { .. })
        ),
        "wrong refusal"
    );

    // The musl-malloc cliff needs the two-key acknowledgment.
    let err = ipe::run_cli(&[
        "build".into(),
        "NoSuch.ipe".into(),
        "--static".into(),
        "--allocator".into(),
        "system".into(),
    ])
    .expect_err("system-on-musl without ack must refuse");
    assert!(
        matches!(
            err,
            CliError::StaticRefusal(build_plan::Refusal::MuslMallocCliff)
        ),
        "wrong refusal"
    );

    // talc is refused until the arena design lands.
    let err = ipe::run_cli(&[
        "build".into(),
        "NoSuch.ipe".into(),
        "--static".into(),
        "--allocator".into(),
        "talc".into(),
    ])
    .expect_err("talc must refuse");
    assert!(
        matches!(
            err,
            CliError::StaticRefusal(build_plan::Refusal::TalcRequiresArenaDesign)
        ),
        "wrong refusal"
    );
}

/// The `run` subcommand carries the same static surface as `build` (one
/// shared flag parser + resolver) — refusals fire identically, before any
/// compilation or filesystem write.
#[test]
fn run_subcommand_refuses_like_build() {
    let err = ipe::run_cli(&[
        "run".into(),
        "NoSuch.ipe".into(),
        "--static".into(),
        "--allocator".into(),
        "talc".into(),
    ])
    .expect_err("talc must refuse on run too");
    assert!(
        matches!(
            err,
            CliError::StaticRefusal(build_plan::Refusal::TalcRequiresArenaDesign)
        ),
        "wrong refusal"
    );

    let err = ipe::run_cli(&[
        "run".into(),
        "NoSuch.ipe".into(),
        "--target".into(),
        "x86_64-unknown-linux-musl".into(),
    ])
    .expect_err("--target without --static must refuse on run too");
    assert!(
        matches!(
            err,
            CliError::StaticRefusal(build_plan::Refusal::TargetRequiresStatic { .. })
        ),
        "wrong refusal"
    );
}

/// `ipe.toml [rust]` parses into the typed request layer; malformed values
/// are refused at manifest-parse time.
#[test]
fn ipe_toml_rust_section_parses_and_rejects_typos() {
    let scratch = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("static_toml_rust");
    let _ = std::fs::remove_dir_all(&scratch);
    std::fs::create_dir_all(scratch.join("src")).expect("mk src");
    std::fs::write(
        scratch.join("src").join("Main.ipe"),
        "module Main exposing (main)\n",
    )
    .expect("write Main.ipe");

    let manifest_path = scratch.join("ipe.toml");
    std::fs::write(
        &manifest_path,
        "name = \"p\"\n[rust]\nstatic = true\nallocator = \"dlmalloc\"\nallowSlowAllocator = false\n",
    )
    .expect("write ipe.toml");
    let parsed = ipe::project::parse_manifest(&manifest_path).expect("parse");
    assert_eq!(
        parsed.static_request,
        build_plan::StaticRequestLayer {
            static_build: Some(true),
            target: None,
            allocator: Some(build_plan::AllocatorChoice::Dlmalloc),
            allow_slow_allocator: Some(false),
        }
    );

    std::fs::write(
        &manifest_path,
        "name = \"p\"\n[rust]\nallocator = \"jemallocc\"\n",
    )
    .expect("write ipe.toml");
    let err = ipe::project::parse_manifest(&manifest_path).expect_err("typo must refuse");
    assert!(
        matches!(
            err,
            CliError::StaticRefusal(build_plan::Refusal::UnknownAllocator { .. })
        ),
        "wrong error"
    );
}

/// TLS must stay rustls with the BUNDLED webpki roots in every manifest
/// source the emitted project is assembled from. A native-TLS or
/// native-roots backend links OpenSSL / reads the host cert store — either
/// silently breaks the fully-static musl artifact (dynamic libssl) or makes
/// it host-dependent (no `/etc/ssl` in a `scratch` container).
///
/// Three sources write dependency lines into an emitted `Cargo.toml`:
/// the golden base manifest, the vendored runtime's manifest, and the
/// surgery strings in the backend's `project.rs`. All three are scanned.
#[test]
fn tls_stays_rustls_with_bundled_roots_in_every_manifest_source() {
    fn read(path: &Path) -> String {
        let text = std::fs::read_to_string(path);
        assert!(
            text.is_ok(),
            "read {}: {:?}",
            path.display(),
            text.as_ref().err()
        );
        text.unwrap_or_default()
    }
    fn dep_line<'a>(text: &'a str, dep: &str, path: &Path) -> &'a str {
        let line = text.lines().find(|l| l.trim_start().starts_with(dep));
        assert!(line.is_some(), "{}: no {dep} dep line", path.display());
        line.unwrap_or_default()
    }

    // Scan only effective (non-comment) content: a comment DOCUMENTING that a
    // backend is deliberately excluded (e.g. "the `native-tls` feature is NOT
    // listed") is not a violation. TOML comments start with `#`, Rust with `//`.
    fn effective(text: &str) -> String {
        text.lines()
            .filter(|l| {
                let t = l.trim_start();
                !t.starts_with('#') && !t.starts_with("//")
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    let root = repo_root();
    let sources = [
        root.join("tests/golden/basics/Cargo.toml"),
        root.join("src/runtime/rust/Cargo.toml"),
        root.join("src/compiler/backend/rust/src/project.rs"),
    ];
    for path in &sources {
        let text = effective(&read(path));
        for forbidden in ["native-tls", "openssl", "rustls-tls-native-roots"] {
            assert!(
                !text.contains(forbidden),
                "{}: contains {forbidden:?} — TLS must stay rustls-only with bundled \
                 webpki roots (static-compilation contract)",
                path.display()
            );
        }
    }

    // The reqwest dep line itself: rustls backend, default features off (the
    // default feature set would pull no TLS at all — `rustls-tls` bundles
    // webpki-roots, keeping cert verification host-independent).
    for path in [
        root.join("tests/golden/basics/Cargo.toml"),
        root.join("src/runtime/rust/Cargo.toml"),
    ] {
        let text = read(&path);
        let reqwest = dep_line(&text, "reqwest", &path);
        assert!(
            reqwest.contains("default-features = false") && reqwest.contains(r#""rustls-tls""#),
            "{}: reqwest must be default-features = false + rustls-tls: {reqwest}",
            path.display()
        );
    }

    // The other TLS-capable deps are pinned to their rustls arms.
    let runtime_path = root.join("src/runtime/rust/Cargo.toml");
    let runtime = read(&runtime_path);
    let lettre = dep_line(&runtime, "lettre", &runtime_path);
    assert!(
        lettre.contains("default-features = false") && lettre.contains(r#""tokio1-rustls-tls""#),
        "lettre must be default-features = false + tokio1-rustls-tls: {lettre}"
    );
    let sqlx = dep_line(&runtime, "sqlx", &runtime_path);
    assert!(
        sqlx.contains(r#""runtime-tokio-rustls""#),
        "sqlx must use the runtime-tokio-rustls arm: {sqlx}"
    );
}

/// Full static proof (THE SEAL, end to end): emit `examples/sky/ipe/01-hello-world`
/// under the dlmalloc static plan, `cargo build` it standalone for musl with
/// CWD = the emitted crate dir (cargo discovers `.cargo/config.toml` from
/// CWD, not from `--manifest-path`), then assert the binary is genuinely
/// static (`ldd`) and runs. Gated: `IPE_E2E_STATIC=1`.
#[test]
fn end_to_end_static_binary_is_static_and_runs() {
    if std::env::var("IPE_E2E_STATIC").is_err() {
        return;
    }
    let root = repo_root();
    let entry = root
        .join("examples")
        .join("sky")
        .join("ipe")
        .join("01-hello-world")
        .join("src")
        .join("Main.ipe");
    let out = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("static_e2e");
    let _ = std::fs::remove_dir_all(&out);
    let runtime = ipe::resolve_runtime().expect("runtime must resolve");

    let plan = dlmalloc_plan();
    build_plan::preflight(&plan).expect("toolchain preflight (musl target + C compiler)");
    ipe::build_with_options(
        &entry,
        &out,
        &runtime,
        BuildOptions {
            static_plan: Some(plan),
            ..BuildOptions::default()
        },
    )
    .expect("static emit");

    // Standalone cargo build, CWD = emitted crate dir. The target dir honours
    // an ambient CARGO_TARGET_DIR (the repo's shared-warm-target convention,
    // same as the examples sweep) and falls back to an isolated dir inside
    // the crate so a bare CI runner stays hermetic.
    let target_dir =
        std::env::var_os("CARGO_TARGET_DIR").map_or_else(|| out.join("target"), PathBuf::from);
    let status = std::process::Command::new("cargo")
        .arg("build")
        .args(["--target", plan.triple.as_str()])
        .env("CARGO_TARGET_DIR", &target_dir)
        .current_dir(&out)
        .status()
        .expect("spawn cargo");
    assert!(status.success(), "cargo build --target musl failed (SEAL)");

    let bin = target_dir
        .join(plan.triple.as_str())
        .join("debug")
        .join("ipe-app");

    // Assert static-ness — never assume it. `ldd` exits non-zero for a
    // static binary on some platforms; the message is the contract.
    let ldd = std::process::Command::new("ldd")
        .arg(&bin)
        .output()
        .expect("run ldd");
    let ldd_text = format!(
        "{}{}",
        String::from_utf8_lossy(&ldd.stdout),
        String::from_utf8_lossy(&ldd.stderr)
    );
    assert!(
        ldd_text.contains("statically linked") || ldd_text.contains("not a dynamic executable"),
        "binary is not static: {ldd_text}"
    );

    // And it runs.
    let run = std::process::Command::new(&bin)
        .output()
        .expect("run binary");
    assert!(run.status.success(), "static binary exited non-zero");
    assert_eq!(String::from_utf8_lossy(&run.stdout), "Hello from Sky!\n");
}

/// `ipe run --static` end to end: the driver emits, cargo-builds for the
/// musl triple, resolves the relocated target dir, and execs a genuinely
/// static binary. Gated: `IPE_E2E_STATIC=1`.
#[test]
fn ipe_run_static_builds_and_executes_a_static_binary() {
    if std::env::var("IPE_E2E_STATIC").is_err() {
        return;
    }
    let root = repo_root();
    let entry = root
        .join("examples")
        .join("sky")
        .join("ipe")
        .join("01-hello-world")
        .join("src")
        .join("Main.ipe");
    let out = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("static_run_e2e");
    let _ = std::fs::remove_dir_all(&out);

    // Reuse an ambient warm target when the caller provides one, else stay
    // hermetic inside the scratch dir.
    let target_dir =
        std::env::var_os("CARGO_TARGET_DIR").map_or_else(|| out.join("target"), PathBuf::from);

    let run = std::process::Command::new(env!("CARGO_BIN_EXE_ipe"))
        .args(["run"])
        .arg(&entry)
        .args(["--static", "--out"])
        .arg(&out)
        .env("CARGO_TARGET_DIR", &target_dir)
        .output()
        .expect("spawn ipe run --static");
    assert!(
        run.status.success(),
        "ipe run --static failed\nstdout: {}\nstderr: {}",
        String::from_utf8_lossy(&run.stdout),
        String::from_utf8_lossy(&run.stderr)
    );
    assert!(
        String::from_utf8_lossy(&run.stdout).contains("Hello from Sky!"),
        "expected program output, got: {}",
        String::from_utf8_lossy(&run.stdout)
    );

    // The executed artifact must be genuinely static.
    let bin = target_dir
        .join("x86_64-unknown-linux-musl")
        .join("debug")
        .join("ipe-app");
    let ldd = std::process::Command::new("ldd")
        .arg(&bin)
        .output()
        .expect("run ldd");
    let ldd_text = format!(
        "{}{}",
        String::from_utf8_lossy(&ldd.stdout),
        String::from_utf8_lossy(&ldd.stderr)
    );
    assert!(
        ldd_text.contains("statically linked") || ldd_text.contains("not a dynamic executable"),
        "ipe run --static executed a non-static binary: {ldd_text}"
    );
}
