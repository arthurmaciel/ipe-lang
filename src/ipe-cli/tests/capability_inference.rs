//! `infer_package_capabilities` surfaces the real compiler diagnostic when a
//! package cannot be lowered, rather than a generic "nothing lowered" that hides
//! the actual cause (regression guard for the opaque failure that masked several
//! example-sweep reds).

use std::error::Error;
use std::fs;

/// A package whose only module fails to lower yields the module's real
/// diagnostic (`CliError::Pipeline`), naming the offending file — never the
/// generic `CliError::Usage` "no module could be lowered".
#[test]
fn a_package_that_cannot_lower_surfaces_the_real_diagnostic() -> Result<(), Box<dyn Error>> {
    let dir = std::env::temp_dir().join("ipe_capinfer_bad_entry");
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(dir.join("src"))?;
    fs::write(
        dir.join("package.ipe"),
        "module Package exposing (package)\n\n\npackage =\n    { name = \"badpkg\", version = \"0.1.0\" }\n",
    )?;
    // `Main` references a name that does not exist, so lowering fails.
    fs::write(
        dir.join("src/Main.ipe"),
        "module Main\n\nmain : Task ()\nmain = thisNameDoesNotExist\n",
    )?;

    let result = ipe::infer_package_capabilities(&dir.join("package.ipe"));

    // The entry's real diagnostic (Pipeline, naming Main.ipe) must surface —
    // never the generic Usage "no module could be lowered".
    let surfaced_entry_diagnostic = matches!(
        &result,
        Err(ipe::CliError::Pipeline { file, .. }) if file.ends_with("Main.ipe")
    );
    assert!(
        surfaced_entry_diagnostic,
        "expected the entry's real Pipeline diagnostic, got: {result:?}"
    );

    let _ = fs::remove_dir_all(&dir);
    Ok(())
}
