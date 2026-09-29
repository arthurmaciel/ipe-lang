use std::ffi::OsStr;

use super::*;

/// A fresh, empty scratch directory for one test.
fn test_dir(name: &str) -> PathBuf {
    let base = std::env::temp_dir()
        .canonicalize()
        .expect("canonical temp dir");
    let dir = base.join(format!(
        "ipe-secret-file-{name}-{}-{:?}",
        std::process::id(),
        std::thread::current().id()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("create test dir");
    dir
}

/// The names in `dir`, sorted.
fn entries(dir: &Path) -> Vec<String> {
    let mut names: Vec<String> = std::fs::read_dir(dir)
        .expect("readdir")
        .filter_map(Result::ok)
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .collect();
    names.sort();
    names
}

/// The entry name `text`, which the test knows to be one plain component.
fn name(text: &str) -> EntryName {
    EntryName::new(OsStr::new(text)).expect("a plain component")
}

#[test]
fn only_a_mode_without_group_or_other_bits_owned_by_the_invoker_is_owner_only() {
    assert!(is_owner_only(0o100_600, 1000, 1000));
    assert!(is_owner_only(0o100_400, 1000, 1000));
    assert!(is_owner_only(0o100_700, 1000, 1000));
    assert!(!is_owner_only(0o100_644, 1000, 1000), "world-readable");
    assert!(!is_owner_only(0o100_640, 1000, 1000), "group-readable");
    assert!(!is_owner_only(0o100_604, 1000, 1000), "other-readable");
    assert!(!is_owner_only(0o100_620, 1000, 1000), "group-writable");
    assert!(!is_owner_only(0o100_777, 1000, 1000), "mode-ignoring mount");
    assert!(!is_owner_only(0o100_600, 1001, 1000), "foreign owner");
    assert!(!is_owner_only(0o100_600, 0, 1000), "root-owned for a user");
}

#[test]
fn an_unsupported_store_is_refused_up_front() {
    assert!(matches!(
        require(SecretStore::Unsupported),
        Err(SecretFileError::Unsupported)
    ));
    assert!(matches!(require(SecretStore::OwnerOnlyFile), Ok(())));
}

#[test]
fn an_unsupported_store_creates_nothing() {
    let dir = test_dir("unsupported");
    let housed = create_owner_dir(SecretStore::Unsupported, &dir.join("sub"));
    assert!(
        matches!(housed, Err(SecretFileError::Unsupported)),
        "an unsupported store must refuse a secret dir, got {housed:?}"
    );
    let opened = open_owner_dir(SecretStore::Unsupported, &dir);
    assert!(
        matches!(opened, Err(SecretFileError::Unsupported)),
        "an unsupported store must hold no secret dir, got {opened:?}"
    );
    assert!(entries(&dir).is_empty(), "a refusal must create nothing");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn an_unsupported_store_reads_no_existing_file() {
    let dir = test_dir("unsupported-read");
    let path = dir.join("secret");
    std::fs::write(&path, "kept").expect("seed file");
    let opened = open_existing(SecretStore::Unsupported, &path);
    assert!(
        matches!(opened, Err(SecretFileError::Unsupported)),
        "an unsupported store must refuse the read, got {opened:?}"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn the_host_store_houses_secrets_on_unix_and_refuses_elsewhere() {
    let dir = test_dir("host-store");
    let housed = create_owner_dir(HOST_SECRET_STORE, &dir.join("sub"));
    if cfg!(unix) {
        let created = housed.map(|owner| owner.create_new(&name("secret")).is_ok());
        assert!(
            matches!(created, Ok(true)),
            "a Unix host houses the secret: {created:?}"
        );
    } else {
        assert!(
            matches!(housed, Err(SecretFileError::Unsupported)),
            "a host without owner-only files must refuse, got {housed:?}"
        );
        assert!(entries(&dir).is_empty(), "the refusal creates nothing");
    }
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_temp_name_is_hidden_beside_its_final_name_and_unpredictable() {
    let first = TempSuffix::fresh().expect("csprng");
    let second = TempSuffix::fresh().expect("csprng");
    assert_ne!(first, second, "two suffixes must differ");
    let temp = first.name_for(&name("token")).expect("one component");
    let text = temp.as_os_str().to_string_lossy().into_owned();
    assert!(
        text.starts_with(".token.") && Path::new(&text).extension() == Some(OsStr::new("tmp")),
        "unexpected temp name {text}"
    );
    assert!(
        text.len() > ".token..tmp".len() + TEMP_SUFFIX_BYTES * 2,
        "the suffix must carry the random bytes: {text}"
    );
}

#[test]
fn a_path_without_a_plain_final_component_is_not_an_entry() {
    for refused in ["/", "/a/..", ""] {
        assert!(
            matches!(split_entry(Path::new(refused)), Err(SecretFileError::Io(e)) if e.kind() == io::ErrorKind::InvalidInput),
            "{refused:?} must not split into a dir and an entry"
        );
    }
    let split = split_entry(Path::new("/a/b"));
    assert!(
        matches!(&split, Ok((dir, entry)) if *dir == Path::new("/a") && *entry == name("b")),
        "{split:?}"
    );
}

#[cfg(unix)]
mod unix {
    use std::os::unix::fs::PermissionsExt as _;

    use super::*;

    /// The directory `dir` held as a secret store, which the test knows to be private.
    fn held(dir: &Path) -> OwnerDir {
        open_owner_dir(SecretStore::OwnerOnlyFile, dir).expect("the test dir is private")
    }

    /// Set the permission bits of `path` to `mode`.
    fn chmod(path: &Path, mode: u32) {
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode)).expect("chmod");
    }

    /// The permission bits of `path`, file type excluded.
    fn mode_of(path: &Path) -> u32 {
        std::fs::metadata(path)
            .expect("metadata")
            .permissions()
            .mode()
            & 0o7777
    }

    #[test]
    fn a_secret_file_is_0600_inside_a_0700_dir_from_creation() {
        let base = test_dir("owner-only");
        let dir = base.join("a").join("b");
        let housed = create_owner_dir(SecretStore::OwnerOnlyFile, &dir);
        assert!(housed.is_ok(), "a fresh secret dir: {housed:?}");
        let Ok(housed) = housed else { return };
        assert_eq!(mode_of(&dir), 0o700, "a new secret dir must be 0700");
        let created = housed.create_new(&name("secret"));
        assert!(created.is_ok(), "owner-only create failed: {created:?}");
        assert_eq!(mode_of(&dir.join("secret")), 0o600);
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn a_staged_temp_file_is_owner_only_beside_its_final_name() {
        let dir = test_dir("temp-owner-only");
        let housed = held(&dir);
        let suffix = TempSuffix::fresh().expect("csprng");
        let staged = housed.create_temp_for(&name("token"), &suffix);
        assert!(staged.is_ok(), "temp create failed: {staged:?}");
        let Ok((_file, temp)) = staged else { return };
        assert_eq!(
            temp,
            suffix.name_for(&name("token")).expect("one component")
        );
        let reopened = housed.open_existing(&temp);
        assert!(
            reopened.is_ok(),
            "the staged file is owner-only: {reopened:?}"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn an_existing_name_is_refused_not_truncated() {
        let dir = test_dir("existing");
        std::fs::write(dir.join("secret"), "kept").expect("seed existing file");
        let created = held(&dir).create_new(&name("secret"));
        assert!(
            matches!(&created, Err(SecretFileError::Io(e)) if e.kind() == io::ErrorKind::AlreadyExists),
            "an existing name must be refused, got {created:?}"
        );
        assert_eq!(
            std::fs::read_to_string(dir.join("secret")).expect("read existing file"),
            "kept"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_planted_symlink_at_the_secret_name_is_refused_and_never_followed() {
        let dir = test_dir("symlink");
        let outside = test_dir("symlink-outside");
        let existing_target = outside.join("existing");
        std::fs::write(&existing_target, "victim").expect("seed target");
        let dangling_target = outside.join("absent");
        let housed = held(&dir);

        std::os::unix::fs::symlink(&existing_target, dir.join("secret")).expect("plant symlink");
        let created = housed.create_new(&name("secret"));
        assert!(
            matches!(&created, Err(SecretFileError::Io(e)) if e.kind() == io::ErrorKind::AlreadyExists),
            "a symlink at the secret name must be refused, got {created:?}"
        );
        assert_eq!(
            std::fs::read_to_string(&existing_target).expect("read target"),
            "victim",
            "the link's target must be untouched"
        );

        std::os::unix::fs::symlink(&dangling_target, dir.join("dangling")).expect("plant dangling");
        let created = housed.create_new(&name("dangling"));
        assert!(
            matches!(&created, Err(SecretFileError::Io(e)) if e.kind() == io::ErrorKind::AlreadyExists),
            "a dangling symlink at the secret name must be refused, got {created:?}"
        );
        assert!(
            std::fs::symlink_metadata(&dangling_target).is_err(),
            "the dangling link's target must not be created"
        );
        let _ = std::fs::remove_dir_all(&dir);
        let _ = std::fs::remove_dir_all(&outside);
    }

    #[test]
    fn an_owner_only_secret_is_read_back() {
        use std::io::{Read as _, Write as _};
        let dir = test_dir("read-back");
        let created = held(&dir).create_new(&name("secret"));
        assert!(created.is_ok(), "create failed: {created:?}");
        let Ok(mut file) = created else { return };
        file.write_all(b"kept").expect("write secret");
        drop(file);
        let opened = open_existing(SecretStore::OwnerOnlyFile, &dir.join("secret"));
        assert!(opened.is_ok(), "an owner-only secret must open: {opened:?}");
        let Ok(mut file) = opened else { return };
        let mut text = String::new();
        file.read_to_string(&mut text).expect("read secret");
        assert_eq!(text, "kept");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_group_or_world_accessible_secret_is_refused_on_read() {
        let dir = test_dir("exposed-read");
        for mode in [0o644, 0o640, 0o604, 0o660] {
            let path = dir.join(format!("secret-{mode:o}"));
            std::fs::write(&path, "exposed").expect("seed secret");
            chmod(&path, mode);
            let opened = open_existing(SecretStore::OwnerOnlyFile, &path);
            assert!(
                matches!(&opened, Err(SecretFileError::NotOwnerOnly(p)) if *p == path),
                "a mode-{mode:o} secret must be refused, got {opened:?}"
            );
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_symlink_or_non_file_secret_is_refused_on_read() {
        let dir = test_dir("link-read");
        let target = dir.join("target");
        std::fs::write(&target, "kept").expect("seed target");
        chmod(&target, 0o600);
        let link = dir.join("secret");
        std::os::unix::fs::symlink(&target, &link).expect("plant symlink");
        let opened = open_existing(SecretStore::OwnerOnlyFile, &link);
        assert!(
            matches!(opened, Err(SecretFileError::Io(_))),
            "a final symlink must not be followed, got {opened:?}"
        );
        let sub = dir.join("sub");
        std::fs::create_dir(&sub).expect("mkdir");
        chmod(&sub, 0o700);
        let opened = open_existing(SecretStore::OwnerOnlyFile, &sub);
        assert!(
            matches!(&opened, Err(SecretFileError::NotRegularFile(p)) if *p == sub),
            "a directory must be refused, got {opened:?}"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_created_file_failing_the_owner_proof_is_refused_and_removed() {
        let dir = test_dir("create-refused");
        let other_user = crate::owner_trust::Invoker::current().uid.wrapping_add(1);
        let created = held(&dir).create_proven(&name("secret"), |file, path| {
            host::prove_owner_only_as(file, path, other_user)
        });
        assert!(
            matches!(&created, Err(SecretFileError::NotOwnerOnly(p)) if *p == dir.join("secret")),
            "a created file owned by someone other than the euid must be refused, got {created:?}"
        );
        assert!(entries(&dir).is_empty(), "the refused file must be removed");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_fifo_secret_is_refused_on_read_without_blocking() {
        let dir = test_dir("fifo-read");
        let fifo = dir.join("secret");
        let made = std::process::Command::new("mkfifo")
            .arg(&fifo)
            .status()
            .expect("run mkfifo");
        assert!(made.success(), "mkfifo failed: {made:?}");
        let opened = open_existing(SecretStore::OwnerOnlyFile, &fifo);
        assert!(
            matches!(&opened, Err(SecretFileError::NotRegularFile(p)) if *p == fifo),
            "a FIFO must be refused as not a regular file, got {opened:?}"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_secret_dir_another_user_can_write_is_refused() {
        let dir = test_dir("owner-dir");
        let exposed = dir.join("exposed");
        std::fs::create_dir(&exposed).expect("mkdir");
        chmod(&exposed, 0o777);
        let housed = create_owner_dir(SecretStore::OwnerOnlyFile, &exposed);
        assert!(
            matches!(&housed, Err(SecretFileError::Dir(DirRefusal::Untrusted(p))) if *p == exposed),
            "a world-writable secret dir must be refused, got {housed:?}"
        );
        chmod(&exposed, 0o700);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_secret_under_a_world_writable_ancestor_is_refused_on_create_and_read() {
        let dir = test_dir("exposed-ancestor");
        let shared = dir.join("shared");
        let inner = shared.join("inner");
        std::fs::create_dir_all(&inner).expect("mkdir");
        std::fs::write(inner.join("secret"), "kept").expect("seed secret");
        chmod(&inner.join("secret"), 0o600);
        chmod(&shared, 0o777);
        let housed = create_owner_dir(SecretStore::OwnerOnlyFile, &inner);
        assert!(
            matches!(&housed, Err(SecretFileError::Dir(DirRefusal::Untrusted(p))) if *p == shared),
            "an ancestor another user can write must be refused, got {housed:?}"
        );
        let opened = open_existing(SecretStore::OwnerOnlyFile, &inner.join("secret"));
        assert!(
            matches!(&opened, Err(SecretFileError::Dir(DirRefusal::Untrusted(p))) if *p == shared),
            "a secret below such an ancestor is never read, got {opened:?}"
        );
        chmod(&shared, 0o700);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_symlinked_secret_dir_is_refused() {
        let dir = test_dir("dir-link");
        let real = dir.join("real");
        std::fs::create_dir(&real).expect("mkdir");
        chmod(&real, 0o700);
        let link = dir.join("link");
        std::os::unix::fs::symlink(&real, &link).expect("symlink");
        let housed = create_owner_dir(SecretStore::OwnerOnlyFile, &link);
        assert!(
            !matches!(&housed, Err(SecretFileError::NotOwnerOnly(_))),
            "a symlinked secret dir is never reported as an exposed secret, got {housed:?}"
        );
        assert!(
            matches!(&housed, Err(SecretFileError::Dir(DirRefusal::Symlinked(p))) if *p == link),
            "a link standing for the secret dir must be refused, got {housed:?}"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn entries_are_renamed_linked_and_removed_through_the_held_dir() {
        let dir = test_dir("entries");
        let housed = held(&dir);
        assert!(housed.create_public(&name("pub")).is_ok());
        assert_eq!(
            mode_of(&dir.join("pub")) & 0o022,
            0,
            "never group/world-writable"
        );
        assert!(housed.create_new(&name("a")).is_ok());
        assert!(housed.rename(&name("a"), &name("b")).is_ok());
        assert!(matches!(housed.is_vacant(&name("a")), Ok(true)));
        assert!(housed.hard_link(&name("b"), &name("c")).is_ok());
        assert!(housed.remove(&name("b")).is_ok());
        assert!(housed.open_existing(&name("c")).is_ok());
        assert_eq!(housed.path(), dir);
        assert_eq!(entries(&dir), vec!["c".to_owned(), "pub".to_owned()]);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
