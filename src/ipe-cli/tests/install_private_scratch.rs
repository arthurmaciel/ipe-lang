//! Refusal tests for the installer's private-scratch helpers.
//!
//! `install.sh` cannot call the Rust scratch primitive, so it carries the same
//! property in shell between the `private-scratch helpers` markers. These tests
//! extract that block and drive every refusal: a symlinked or group/world
//! accessible directory, a non-sticky world-writable base, and a tag file that
//! is a planted symlink (whose target must stay untouched). The pure verdicts
//! are driven with synthetic `ls -l` facts, so the foreign-owner and
//! foreign-group refusals need no second account.
#![cfg(unix)]

use std::io;
use std::os::unix::fs::PermissionsExt as _;
use std::path::{Path, PathBuf};
use std::process::Command;

use ipe_sandbox::scratch::{ScratchDir, ScratchLeaf};

const BEGIN: &str = "# >>> private-scratch helpers";
const END: &str = "# <<< private-scratch helpers";

/// The helper block of `install.sh`, markers included.
fn helpers() -> io::Result<String> {
    let script = std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/../../install.sh"))?;
    let block = script
        .find(BEGIN)
        .and_then(|start| {
            script
                .get(start..)
                .and_then(|tail| tail.find(END).map(|end| (start, start + end + END.len())))
        })
        .and_then(|(start, end)| script.get(start..end));
    block.map(str::to_owned).ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::NotFound,
            "install.sh lost its private-scratch helper markers",
        )
    })
}

/// Whether `sh` running the helper `function` on `arg` succeeds.
fn helper_accepts(function: &str, arg: &Path) -> io::Result<bool> {
    let script = format!("{}\n{function} \"$1\"\n", helpers()?);
    let status = Command::new("sh")
        .arg("-c")
        .arg(script)
        .arg("sh")
        .arg(arg)
        .stdout(std::process::Stdio::null())
        .status()?;
    Ok(status.success())
}

/// Whether `sh` running the helper `function` on the literal `args` succeeds.
fn verdict_accepts(function: &str, args: &[&str]) -> io::Result<bool> {
    let script = format!("{}\n{function} \"$@\"\n", helpers()?);
    let status = Command::new("sh")
        .arg("-c")
        .arg(script)
        .arg("sh")
        .args(args)
        .stdout(std::process::Stdio::null())
        .status()?;
    Ok(status.success())
}

/// A test root under the per-binary target temp dir.
fn root(label: &str) -> io::Result<ScratchDir> {
    ScratchDir::new_under(Path::new(env!("CARGO_TARGET_TMPDIR")), label)
}

/// The path of the entry `name` directly inside `r`.
fn child(r: &ScratchDir, name: &str) -> io::Result<PathBuf> {
    Ok(r.child(&ScratchLeaf::new(name)?))
}

fn mkdir_mode(path: &Path, mode: u32) -> io::Result<()> {
    std::fs::create_dir(path)?;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode))
}

#[test]
fn private_dir_ok_accepts_only_an_owned_0700_directory() -> io::Result<()> {
    let r = root("install-private-dir")?;
    let private = child(&r, "private")?;
    mkdir_mode(&private, 0o700)?;
    assert!(helper_accepts("private_dir_ok", &private)?);

    for (name, mode) in [("world", 0o777), ("group", 0o750), ("other", 0o705)] {
        let dir = child(&r, name)?;
        mkdir_mode(&dir, mode)?;
        assert!(
            !helper_accepts("private_dir_ok", &dir)?,
            "mode {mode:o} must be refused"
        );
    }

    let link = child(&r, "link")?;
    std::os::unix::fs::symlink(&private, &link)?;
    assert!(
        !helper_accepts("private_dir_ok", &link)?,
        "a symlink to a private directory must be refused"
    );
    Ok(())
}

#[test]
fn trusted_tmp_base_refuses_a_non_sticky_world_writable_base() -> io::Result<()> {
    let r = root("install-base")?;
    let open = child(&r, "open")?;
    mkdir_mode(&open, 0o777)?;
    assert!(!helper_accepts("trusted_tmp_base", &open)?);

    let nested = open.join("nested");
    mkdir_mode(&nested, 0o700)?;
    assert!(
        !helper_accepts("trusted_tmp_base", &nested)?,
        "a base under a non-sticky world-writable ancestor must be refused"
    );

    let sticky = child(&r, "sticky")?;
    mkdir_mode(&sticky, 0o1777)?;
    assert!(helper_accepts("trusted_tmp_base", &sticky)?);
    Ok(())
}

#[test]
fn tag_file_ok_refuses_a_planted_symlink_and_leaves_its_target() -> io::Result<()> {
    let r = root("install-tag")?;
    let canary = child(&r, "canary")?;
    std::fs::write(&canary, b"canary")?;

    let private = child(&r, "private")?;
    mkdir_mode(&private, 0o700)?;
    let link = private.join("tag");
    std::os::unix::fs::symlink(&canary, &link)?;
    assert!(!helper_accepts("tag_file_ok", &link)?);
    assert_eq!(std::fs::read(&canary)?, b"canary");

    let real = private.join("real");
    std::fs::write(&real, b"")?;
    std::fs::set_permissions(&real, std::fs::Permissions::from_mode(0o600))?;
    assert!(helper_accepts("tag_file_ok", &real)?);

    std::fs::set_permissions(&real, std::fs::Permissions::from_mode(0o644))?;
    assert!(
        !helper_accepts("tag_file_ok", &real)?,
        "a group/other-readable tag file must be refused"
    );

    let open = child(&r, "open")?;
    mkdir_mode(&open, 0o755)?;
    let exposed = open.join("tag");
    std::fs::write(&exposed, b"")?;
    std::fs::set_permissions(&exposed, std::fs::Permissions::from_mode(0o600))?;
    assert!(
        !helper_accepts("tag_file_ok", &exposed)?,
        "a tag file outside a private directory must be refused"
    );
    Ok(())
}

#[test]
fn the_installer_routes_its_scratch_through_the_helpers() -> io::Result<()> {
    let script = std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/../../install.sh"))?;
    for needle in [
        "tag_file_ok \"$TAG_FILE\"",
        "trusted_tmp_base \"${TMPDIR:-/tmp}\"",
        "mktemp -d \"$scratch_base/ipe-install.XXXXXX\"",
        "private_dir_ok \"$tmp\"",
        "mktemp \"$IPE_HOME/.env.XXXXXX\"",
    ] {
        assert!(
            script.contains(needle),
            "install.sh must contain `{needle}`"
        );
    }
    assert!(
        !script.contains("mktemp -d)"),
        "install.sh must not create an unverified `mktemp -d` scratch dir"
    );
    Ok(())
}

/// The uid and primary gid the synthetic facts are judged against.
const ME: &str = "1000";
const MY_GID: &str = "1000";

#[test]
fn scratch_base_verdict_accepts_only_trusted_components() -> io::Result<()> {
    for (mode, owner, group) in [
        ("drwx------", ME, MY_GID),
        ("drwxr-xr-x", "0", "0"),
        ("drwxr-xr-x.", "0", "0"),
        ("drwxrwxrwt", "0", "0"),
        ("drwxrwxrwT", "0", "0"),
        ("drwxrwxr-x", ME, MY_GID),
    ] {
        assert!(
            verdict_accepts("scratch_base_verdict", &[mode, owner, group, ME, MY_GID])?,
            "{mode} {owner}:{group} must be accepted"
        );
    }
    Ok(())
}

#[test]
fn scratch_base_verdict_refuses_every_untrusted_component() -> io::Result<()> {
    for (mode, owner, group, why) in [
        ("drwxr-xr-x", "1001", "1001", "a foreign owner"),
        (
            "drwxrwxr-x",
            "0",
            MY_GID,
            "a group-writable root-owned directory",
        ),
        (
            "drwxrwxr-x",
            ME,
            "1001",
            "a group-writable directory in a foreign group",
        ),
        (
            "drwxrwxr-x+",
            ME,
            MY_GID,
            "an own-group-writable directory carrying an ACL",
        ),
        (
            "drwxrwxrwx",
            ME,
            MY_GID,
            "a non-sticky world-writable directory",
        ),
        (
            "drwxrwxrwx",
            "0",
            "0",
            "a non-sticky world-writable root directory",
        ),
        ("-rw-------", ME, MY_GID, "a regular file"),
        ("lrwxrwxrwx", ME, MY_GID, "a symlink"),
        ("", ME, MY_GID, "an empty mode"),
    ] {
        assert!(
            !verdict_accepts("scratch_base_verdict", &[mode, owner, group, ME, MY_GID])?,
            "{why} ({mode} {owner}:{group}) must be refused"
        );
    }
    assert!(
        !verdict_accepts("scratch_base_verdict", &["drwx------", "", "", "", ""])?,
        "an unknown identity must be refused"
    );
    Ok(())
}

#[test]
fn scratch_private_verdict_refuses_foreign_or_open_entries() -> io::Result<()> {
    const DIR: &str = "d???------*";
    const FILE: &str = "-???------*";
    assert!(verdict_accepts(
        "scratch_private_verdict",
        &["drwx------", ME, ME, DIR]
    )?);
    assert!(verdict_accepts(
        "scratch_private_verdict",
        &["drwx------.", ME, ME, DIR]
    )?);
    assert!(verdict_accepts(
        "scratch_private_verdict",
        &["-rw-------", ME, ME, FILE]
    )?);
    for (mode, owner, pattern, why) in [
        ("drwx------", "1001", DIR, "a foreign-owned directory"),
        ("drwxr-x---", ME, DIR, "a group-readable directory"),
        ("drwx-----x", ME, DIR, "an other-searchable directory"),
        (
            "-rw-------",
            ME,
            DIR,
            "a file where a directory is required",
        ),
        (
            "drwx------",
            ME,
            FILE,
            "a directory where a file is required",
        ),
        ("-rw-r--r--", ME, FILE, "a group/other-readable file"),
        ("-rw-------", "1001", FILE, "a foreign-owned file"),
    ] {
        assert!(
            !verdict_accepts("scratch_private_verdict", &[mode, owner, ME, pattern])?,
            "{why} ({mode} {owner}) must be refused"
        );
    }
    assert!(
        !verdict_accepts("scratch_private_verdict", &["drwx------", "", "", DIR])?,
        "an unknown identity must be refused"
    );
    Ok(())
}

#[test]
fn trusted_tmp_base_refuses_a_non_directory_or_missing_base() -> io::Result<()> {
    let r = root("install-nondir")?;
    let file = child(&r, "file")?;
    std::fs::write(&file, b"")?;
    assert!(!helper_accepts("trusted_tmp_base", &file)?);
    assert!(!helper_accepts("trusted_tmp_base", &child(&r, "missing")?)?);
    Ok(())
}
