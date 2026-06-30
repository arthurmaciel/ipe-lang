// Path kernel stubs — pure string manipulation, no filesystem I/O.
// All functions are total (never panic, no Task wrapper):
//   base, dir, ext : String -> String
//   isAbsolute     : String -> Bool
//
// These are FAITHFUL ports of Go's `path/filepath` (Unix semantics, separator
// `/`) rather than thin wrappers over Rust's `std::path` — `std::path` is
// OS-tagged and diverges from Go on trailing slashes, repeated separators, and
// dotfiles (`std::path::Path::extension(".bashrc")` → None, Go's
// `filepath.Ext(".bashrc")` → ".bashrc"). The Rust backend's Go≡Rust equivalence
// target runs on Linux, where Go's filepath uses `/`, so we implement Unix
// filepath exactly. (Audit 2026-06-19, correctness/parity — was deferred.)
//
// Routing (Kernel.hs fallthrough — all map cleanly via toSnakeCase):
//   ("Path", "base")       / ("Sky.Core.Path", "base")       -> "path_base"
//   ("Path", "dir")        / ("Sky.Core.Path", "dir")        -> "path_dir"
//   ("Path", "ext")        / ("Sky.Core.Path", "ext")        -> "path_ext"
//   ("Path", "isAbsolute") / ("Sky.Core.Path", "isAbsolute") -> "path_is_absolute"

const SEP: u8 = b'/';

/// Faithful port of Go `path/filepath.Clean` (Unix). Lexically simplifies a
/// path: collapses repeated `/`, resolves `.`/`..` elements, drops a trailing
/// `/` (except root). Pure byte work — multi-byte UTF-8 path elements are copied
/// intact (their bytes are never `/` or ASCII `.`), so the result is valid
/// UTF-8.
fn clean(path: &str) -> String {
    if path.is_empty() {
        return ".".to_string();
    }
    let b = path.as_bytes();
    let n = b.len();
    // Total byte access (no `[]` indexing — clippy::indexing_slicing / no-panic
    // gate). Out-of-range reads as `None`, never panics.
    let at = |i: usize| -> Option<u8> { b.get(i).copied() };
    let rooted = at(0) == Some(SEP);
    let mut out: Vec<u8> = Vec::with_capacity(n + 1);
    let mut r = 0usize;
    // `dotdot` is the index in `out` past which leading `..`s have been written
    // (for a relative path) or past the root `/` — popping never crosses it.
    let mut dotdot = 0usize;
    if rooted {
        out.push(SEP);
        r = 1;
        dotdot = 1;
    }
    while r < n {
        if at(r) == Some(SEP) {
            // empty path element → skip
            r += 1;
        } else if at(r) == Some(b'.') && (r + 1 == n || at(r + 1) == Some(SEP)) {
            // `.` element → skip
            r += 1;
        } else if at(r) == Some(b'.')
            && at(r + 1) == Some(b'.')
            && (r + 2 == n || at(r + 2) == Some(SEP))
        {
            // `..` element → back up
            r += 2;
            if out.len() > dotdot {
                // pop the last element
                let mut w = out.len() - 1;
                while w > dotdot && out.get(w).copied() != Some(SEP) {
                    w -= 1;
                }
                out.truncate(w);
            } else if !rooted {
                // cannot back up → keep the `..`
                if !out.is_empty() {
                    out.push(SEP);
                }
                out.push(b'.');
                out.push(b'.');
                dotdot = out.len();
            }
        } else {
            // real path element → append a separator (if needed) then the element
            if (rooted && out.len() != 1) || (!rooted && !out.is_empty()) {
                out.push(SEP);
            }
            while r < n && at(r) != Some(SEP) {
                if let Some(c) = at(r) {
                    out.push(c);
                }
                r += 1;
            }
        }
    }
    if out.is_empty() {
        return ".".to_string();
    }
    String::from_utf8(out).unwrap_or_else(|_| ".".to_string())
}

/// `Sky.Core.Path.base : String -> String` — Go `filepath.Base` (Unix).
/// "" → "."; all-slashes → "/"; else the final element with trailing slashes
/// stripped.
pub fn path_base(path: String) -> String {
    if path.is_empty() {
        return ".".to_string();
    }
    // strip trailing separators
    let b = path.as_bytes();
    let mut end = b.len();
    while end > 0 && b.get(end - 1).copied() == Some(SEP) {
        end -= 1;
    }
    if end == 0 {
        // path was all separators
        return "/".to_string();
    }
    let stripped = path.get(..end).unwrap_or(&path);
    let sb = stripped.as_bytes();
    // find the last separator
    let mut i = sb.len();
    while i > 0 && sb.get(i - 1).copied() != Some(SEP) {
        i -= 1;
    }
    stripped.get(i..).unwrap_or("").to_string()
}

/// `Sky.Core.Path.dir : String -> String` — Go `filepath.Dir` (Unix).
/// All but the last element, then `Clean`ed: "" / "foo" → "."; "/" → "/";
/// "/foo/bar" → "/foo"; "/foo/" → "/foo"; "a//b" → "a".
pub fn path_dir(path: String) -> String {
    let b = path.as_bytes();
    let mut i = b.len();
    while i > 0 && b.get(i - 1).copied() != Some(SEP) {
        i -= 1;
    }
    // path[..i] is everything up to and including the last separator (or "" when
    // there is none). Clean("") = ".".
    clean(path.get(..i).unwrap_or(""))
}

/// `Sky.Core.Path.ext : String -> String` — Go `filepath.Ext` (Unix).
/// The suffix from the LAST `.` in the final path element (including the dot),
/// or "" when the final element has no dot. `filepath.Ext(".bashrc")` → ".bashrc".
pub fn path_ext(path: String) -> String {
    let b = path.as_bytes();
    let mut i = b.len();
    while i > 0 {
        match b.get(i - 1).copied() {
            Some(c) if c == SEP => break,
            Some(b'.') => return path.get(i - 1..).unwrap_or("").to_string(),
            _ => {}
        }
        i -= 1;
    }
    String::new()
}

/// `Sky.Core.Path.isAbsolute : String -> Bool` — Go `filepath.IsAbs` (Unix):
/// an absolute path begins with `/`.
pub fn path_is_absolute(path: String) -> bool {
    path.as_bytes().first() == Some(&SEP)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base_empty() {
        assert_eq!(path_base(String::new()), ".");
    }

    #[test]
    fn base_filename() {
        assert_eq!(path_base("/foo/bar.txt".to_string()), "bar.txt");
    }

    #[test]
    fn base_no_dir() {
        assert_eq!(path_base("hello.sky".to_string()), "hello.sky");
    }

    #[test]
    fn base_trailing_slash() {
        // Go: filepath.Base("/foo/") = "foo"
        assert_eq!(path_base("/foo/".to_string()), "foo");
    }

    #[test]
    fn base_root() {
        // Go: filepath.Base("/") = "/"
        assert_eq!(path_base("/".to_string()), "/");
    }

    #[test]
    fn dir_empty() {
        assert_eq!(path_dir(String::new()), ".");
    }

    #[test]
    fn dir_with_parent() {
        assert_eq!(path_dir("/foo/bar.txt".to_string()), "/foo");
    }

    #[test]
    fn dir_bare_name() {
        assert_eq!(path_dir("hello.sky".to_string()), ".");
    }

    #[test]
    fn dir_trailing_slash() {
        // Go: filepath.Dir("/foo/") = "/foo"
        assert_eq!(path_dir("/foo/".to_string()), "/foo");
    }

    #[test]
    fn dir_double_separator() {
        // Go: filepath.Dir("a//b") = "a"  (Clean collapses the repeat)
        assert_eq!(path_dir("a//b".to_string()), "a");
    }

    #[test]
    fn dir_root() {
        // Go: filepath.Dir("/") = "/"
        assert_eq!(path_dir("/".to_string()), "/");
    }

    #[test]
    fn ext_present() {
        assert_eq!(path_ext("/foo/bar.txt".to_string()), ".txt");
    }

    #[test]
    fn ext_absent() {
        assert_eq!(path_ext("/foo/bar".to_string()), "");
    }

    #[test]
    fn ext_dotfile() {
        // Go: filepath.Ext(".bashrc") = ".bashrc"
        assert_eq!(path_ext(".bashrc".to_string()), ".bashrc");
    }

    #[test]
    fn ext_multiple_dots() {
        // Go: filepath.Ext("a.b.c") = ".c"
        assert_eq!(path_ext("a.b.c".to_string()), ".c");
    }

    #[test]
    fn is_absolute_true() {
        assert!(path_is_absolute("/usr/bin".to_string()));
    }

    #[test]
    fn is_absolute_false() {
        assert!(!path_is_absolute("relative/path".to_string()));
    }
}
