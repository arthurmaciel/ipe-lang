//! The A6 change diff: what changed inside a re-extracted file, expressed as
//! queue events.

use std::collections::HashMap;
use std::time::{SystemTime, UNIX_EPOCH};

/// One unit-level change destined for the change queue. `change` is one of
/// "new", "modified", "deleted".
pub struct Change {
    pub uid: String,
    pub change: &'static str,
    pub old_hash: Option<String>,
    pub new_hash: Option<String>,
}

/// Diff two (uid, body_hash) snapshots of the same path. Units whose hash is
/// unchanged produce no event; everything else becomes one event per uid.
pub fn diff_units(old: &[(String, String)], new: &[(String, String)]) -> Vec<Change> {
    let old_map: HashMap<&str, &str> = old.iter().map(|(u, h)| (u.as_str(), h.as_str())).collect();
    let new_map: HashMap<&str, &str> = new.iter().map(|(u, h)| (u.as_str(), h.as_str())).collect();
    let mut uids: Vec<&str> = old_map.keys().chain(new_map.keys()).copied().collect();
    uids.sort_unstable();
    uids.dedup();
    let mut events = Vec::new();
    for uid in uids {
        match (old_map.get(uid), new_map.get(uid)) {
            (Some(old_h), None) => events.push(Change {
                uid: uid.to_string(),
                change: "deleted",
                old_hash: Some(old_h.to_string()),
                new_hash: None,
            }),
            (None, Some(new_h)) => events.push(Change {
                uid: uid.to_string(),
                change: "new",
                old_hash: None,
                new_hash: Some(new_h.to_string()),
            }),
            (Some(old_h), Some(new_h)) if old_h != new_h => events.push(Change {
                uid: uid.to_string(),
                change: "modified",
                old_hash: Some(old_h.to_string()),
                new_hash: Some(new_h.to_string()),
            }),
            _ => {}
        }
    }
    events
}

/// A changed file and the new-side line ranges its diff hunks touch.
pub type FileHunks = (String, Vec<(i64, i64)>);

/// The changed line ranges of each file in a git range, from
/// `git diff --unified=0`. Returns `(relpath, [(start, end)])` covering the
/// NEW-side lines a hunk touches — the review scope a `changed` query maps onto
/// units. Deletions (no new-side lines) attribute to the line the removal sits
/// at, so a deleted body still surfaces its enclosing unit.
///
/// `range` is validated the same way `walk::changed`'s since-ref is: no leading
/// `-` (option smuggling) and only ref-safe bytes, so a crafted range can't
/// inject git options.
pub fn changed_line_ranges(repo: &str, range: &str) -> anyhow::Result<Vec<FileHunks>> {
    use anyhow::bail;
    use std::collections::BTreeMap;
    if range.is_empty()
        || range.starts_with('-')
        || !range.bytes().all(|b| {
            b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'/' | b'-' | b'~' | b'^')
        })
    {
        bail!("refusing unsafe git range: {range:?}");
    }
    let out = std::process::Command::new("git")
        .arg("-c")
        .arg("core.quotePath=false")
        .arg("-C")
        .arg(repo)
        .env("GIT_TERMINAL_PROMPT", "0")
        .stdin(std::process::Stdio::null())
        .args(["diff", "--unified=0", "--no-color", "--no-renames", range])
        .output()?;
    if !out.status.success() {
        bail!(
            "git diff failed in {repo}: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }
    let text = String::from_utf8_lossy(&out.stdout);
    let mut per_file: BTreeMap<String, Vec<(i64, i64)>> = BTreeMap::new();
    let mut current: Option<String> = None;
    for line in text.lines() {
        if let Some(rest) = line.strip_prefix("+++ ") {
            // `+++ b/path` (or `+++ /dev/null` for a deletion).
            current = rest
                .strip_prefix("b/")
                .filter(|p| *p != "/dev/null")
                .map(|p| p.to_string());
        } else if let Some(range_text) = line.strip_prefix("@@ ") {
            let Some(file) = &current else { continue };
            if let Some((start, count)) = parse_new_hunk(range_text) {
                // `count == 0` is a pure deletion at `start`; treat it as a
                // single line so the enclosing unit is still reported.
                let end = start + count.max(1) - 1;
                per_file.entry(file.clone()).or_default().push((start, end));
            }
        }
    }
    Ok(per_file.into_iter().collect())
}

/// Parse the NEW-side `+start[,count]` of a `@@ -a,b +start,count @@` header.
fn parse_new_hunk(header: &str) -> Option<(i64, i64)> {
    let plus = header.split_whitespace().find(|t| t.starts_with('+'))?;
    let body = plus.strip_prefix('+')?;
    match body.split_once(',') {
        Some((s, c)) => Some((s.parse().ok()?, c.parse().ok()?)),
        None => Some((body.parse().ok()?, 1)),
    }
}

/// Unix-epoch milliseconds, as the change queue's enqueued_at column wants.
pub fn now_millis() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as i64
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pair(uid: &str, hash: &str) -> (String, String) {
        (uid.to_string(), hash.to_string())
    }

    #[test]
    fn identical_snapshots_produce_no_events() {
        let old = vec![pair("u-a", "h1"), pair("u-b", "h2")];
        let new = vec![pair("u-b", "h2"), pair("u-a", "h1")];
        assert!(diff_units(&old, &new).is_empty());
    }

    #[test]
    fn brand_new_units_are_new() {
        let events = diff_units(&[], &[pair("u-c", "h3")]);
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].uid, "u-c");
        assert_eq!(events[0].change, "new");
        assert_eq!(events[0].old_hash, None);
        assert_eq!(events[0].new_hash.as_deref(), Some("h3"));
    }

    #[test]
    fn vanished_units_are_deleted() {
        let events = diff_units(&[pair("u-a", "h1")], &[]);
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].change, "deleted");
        assert_eq!(events[0].old_hash.as_deref(), Some("h1"));
        assert_eq!(events[0].new_hash, None);
    }

    #[test]
    fn changed_hash_is_modified() {
        let events = diff_units(&[pair("u-a", "h1")], &[pair("u-a", "h2")]);
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].change, "modified");
        assert_eq!(events[0].old_hash.as_deref(), Some("h1"));
        assert_eq!(events[0].new_hash.as_deref(), Some("h2"));
    }

    #[test]
    fn parses_new_hunk_headers() {
        assert_eq!(parse_new_hunk("-1,3 +4,5 @@ fn x"), Some((4, 5)));
        assert_eq!(parse_new_hunk("-1 +2 @@"), Some((2, 1))); // single line, no count
        assert_eq!(parse_new_hunk("-1,2 +0,0 @@"), Some((0, 0))); // pure deletion
        assert_eq!(parse_new_hunk("no plus token"), None);
    }

    #[test]
    fn events_are_deterministically_ordered() {
        let old = vec![pair("u-b", "h1")];
        let new = vec![pair("u-a", "h2"), pair("u-b", "h2")];
        let events = diff_units(&old, &new);
        let uids: Vec<&str> = events.iter().map(|e| e.uid.as_str()).collect();
        assert_eq!(uids, vec!["u-a", "u-b"]);
    }
}
