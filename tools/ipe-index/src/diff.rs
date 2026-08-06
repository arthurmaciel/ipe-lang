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
    fn events_are_deterministically_ordered() {
        let old = vec![pair("u-b", "h1")];
        let new = vec![pair("u-a", "h2"), pair("u-b", "h2")];
        let events = diff_units(&old, &new);
        let uids: Vec<&str> = events.iter().map(|e| e.uid.as_str()).collect();
        assert_eq!(uids, vec!["u-a", "u-b"]);
    }
}
