//! Drift guard: the shell installer (`scripts/install.sh`) cannot import the
//! Rust `style` module, so it hand-mirrors the palette, the repository URL, and
//! the "report bugs" footer. This test reads the script and asserts those
//! mirrored values EQUAL the `style` SSOT constants — an unmirrored change to
//! either side fails CI. That enforced equality is how the shell "reuses" the
//! single source of truth: by test, not by trust.

use ipe::style::{self, Palette};

/// Read `scripts/install.sh` from the repository root (two levels up from this
/// crate's manifest). A read failure surfaces as a plain assertion carrying the
/// path and error — the empty string it returns then fails every `contains`
/// check with a clear message, never a silent pass.
fn install_script() -> String {
    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/../../scripts/install.sh");
    let read = std::fs::read_to_string(path);
    assert!(read.is_ok(), "could not read {path}: {read:?}");
    read.unwrap_or_default()
}

/// The installer assigns each colour with `printf '\033[…m'`, whereas the Rust
/// palette carries the same escape as a literal `\x1b[…m`. Reduce a palette
/// field to the numeric+letter tail (`38;5;222m`) that both spellings share, so
/// the assertion compares the code itself, not its escape spelling.
fn ansi_tail(escape: &str) -> &str {
    escape.trim_start_matches('\x1b').trim_start_matches('[')
}

#[test]
fn installer_palette_mirrors_the_style_ssot() {
    let script = install_script();
    let color = &Palette::COLOR;

    // Each coloured field must appear in the script as a `printf '\033[<tail>'`
    // assignment. Checking the `\033[<tail>` fragment matches the script's own
    // spelling exactly while staying independent of which shell var holds it.
    for (name, escape) in [
        ("yellow", color.yellow),
        ("dim", color.dim),
        ("green", color.green),
        ("red", color.red),
        ("bold", color.bold),
        ("reset", color.reset),
    ] {
        let needle = format!("\\033[{}", ansi_tail(escape));
        assert!(
            script.contains(&needle),
            "install.sh must mirror the style `{name}` colour as `printf '{needle}'`"
        );
    }
}

#[test]
fn installer_mirrors_the_repo_url_and_bug_footer() {
    let script = install_script();

    // The installer builds the URL from `REPO="owner/repo"`; assert the SSOT
    // URL is exactly that GitHub base so the two cannot drift.
    let expected_repo = style::REPO_URL
        .strip_prefix("https://github.com/")
        .unwrap_or(style::REPO_URL);
    assert!(
        script.contains(&format!("REPO=\"{expected_repo}\"")),
        "install.sh REPO must equal the style REPO_URL path segment `{expected_repo}`"
    );

    // The success footer's fixed phrase must match the SSOT footer verbatim,
    // and the URL it points at must be the SSOT URL.
    let footer = style::report_bugs_footer();
    let phrase = "If you find any bugs, please report them at ";
    assert!(
        footer.starts_with(phrase),
        "the style footer phrase changed — update this test and install.sh"
    );
    assert!(
        script.contains(phrase),
        "install.sh must mirror the style footer phrase `{phrase}`"
    );
    assert!(
        footer.ends_with("/issues."),
        "the style footer must end at `{}/issues.`",
        style::REPO_URL
    );
    // The installer renders `.../$REPO/issues.` — assert the literal tail.
    assert!(
        script.contains("/issues."),
        "install.sh footer must point at the `/issues.` page"
    );
}

#[test]
fn installer_mirrors_the_spinner_frames() {
    let script = install_script();
    for frame in style::SPINNER_FRAMES {
        assert!(
            script.contains(frame),
            "install.sh spinner must include the style frame `{frame}`"
        );
    }
}

#[test]
fn installer_mirrors_the_status_glyphs_it_uses() {
    // The installer leads step lines with `•` and success lines with `✓`; its
    // failure format spells out "error:" instead of a glyph, so the `✗` fail
    // glyph is CLI-only. Assert the two the installer does render match the SSOT.
    let script = install_script();
    for (name, glyph) in [("step", style::glyph::STEP), ("ok", style::glyph::OK)] {
        assert!(
            script.contains(glyph),
            "install.sh must use the style `{name}` glyph `{glyph}`"
        );
    }
}
