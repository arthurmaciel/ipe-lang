//! Runtime revocation store — the session-layer fail-closed gate.
//!
//! The store holds revoked subjects (every session of that user) and revoked
//! session ids (`jti`, one specific session). Its sole question is boolean.
//!
//! # Fail-closed
//!
//! `is_revoked` returns [`Verdict::Revoked`] on a positive hit, [`Verdict::Unknown`]
//! on any store error, and [`Verdict::Active`] only when the store is healthy and
//! the subject/session is absent from both sets. The `authed_route` middleware
//! denies on `Revoked` **and** on `Unknown` — a degraded store denies, never admits.
//!
//! # Store characteristics
//!
//! Both sets are stored behind a `Mutex<HashSet<String>>`. Mutation requires an
//! authenticated `Principal` (enforced by the kernel signatures), so an unauthenticated
//! caller cannot grow the store. The sets are currently **unbounded**: they grow when the
//! app calls `revokeUser`/`revokeSession` and shrink when the app calls `restoreUser`.
//! A hard entry ceiling with TTL eviction is not yet implemented.

use std::collections::HashSet;
use std::sync::{Mutex, MutexGuard, OnceLock};

use super::*;

/// The verdict `is_revoked` returns for a given subject + session pair.
///
/// The caller (`authed_route`) denies on both [`Verdict::Revoked`] and
/// [`Verdict::Unknown`] — only [`Verdict::Active`] allows the request through.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Verdict {
    /// The subject and session id are absent from both revocation sets.
    Active,
    /// The subject or session id is in a revocation set.
    Revoked,
    /// The store is unavailable (lock poisoned or internal error).
    Unknown,
}

/// The revocation store: two `HashSet<String>` protected by a single `Mutex`.
struct RevocationStore {
    /// Revoked subjects — every token for this user is denied.
    subjects: HashSet<String>,
    /// Revoked session ids (`jti`) — only this specific session is denied.
    sessions: HashSet<String>,
}

impl RevocationStore {
    fn new() -> Self {
        Self {
            subjects: HashSet::new(),
            sessions: HashSet::new(),
        }
    }
}

fn store() -> &'static Mutex<RevocationStore> {
    static STORE: OnceLock<Mutex<RevocationStore>> = OnceLock::new();
    STORE.get_or_init(|| Mutex::new(RevocationStore::new()))
}

/// Acquire the store lock. Returns `None` on lock poison (fail-closed: the
/// caller treats `None` as [`Verdict::Unknown`] and denies the request).
fn lock() -> Option<MutexGuard<'static, RevocationStore>> {
    store().lock().ok()
}

/// Query whether `subject` or `jti` is revoked.
///
/// Returns [`Verdict::Active`] only when the store is healthy and neither the
/// subject nor the session id appears in either revocation set. Any lock error
/// yields [`Verdict::Unknown`], which the middleware treats as a denial.
///
/// Fail-closed by construction: the store can grow (new revocations) but never
/// silently drop existing ones — an Unknown result is the conservative branch.
#[must_use]
pub fn is_revoked(subject: &str, jti: &str) -> Verdict {
    let Some(guard) = lock() else {
        return Verdict::Unknown;
    };
    if guard.subjects.contains(subject) || guard.sessions.contains(jti) {
        Verdict::Revoked
    } else {
        Verdict::Active
    }
}

/// Mark every session of `subject` as revoked. Idempotent.
pub fn revoke_subject(subject: String) -> Result<(), String> {
    let Some(mut guard) = lock() else {
        return Err("revocation store unavailable".to_string());
    };
    guard.subjects.insert(subject);
    Ok(())
}

/// Mark the specific session `jti` as revoked. Idempotent.
pub fn revoke_session(jti: String) -> Result<(), String> {
    let Some(mut guard) = lock() else {
        return Err("revocation store unavailable".to_string());
    };
    guard.sessions.insert(jti);
    Ok(())
}

/// Clear the subject revocation for `subject`. After this call a new token for
/// that subject passes the revocation gate (existing `jti`-scoped entries are
/// unaffected — restoring the subject does not un-revoke specific sessions that
/// were independently revoked via `revoke_session`).
pub fn restore_subject(subject: &str) -> Result<(), String> {
    let Some(mut guard) = lock() else {
        return Err("revocation store unavailable".to_string());
    };
    guard.subjects.remove(subject);
    Ok(())
}

/// Query whether `subject` is in the subject-revocation set. Does not check
/// session-scoped entries (a subject not in the set may still have a revoked
/// `jti`). Intended for the `isRevoked` app-facing kernel (an admin UI query),
/// not for the per-request auth gate (which calls [`is_revoked`]).
#[must_use]
pub fn subject_is_revoked(subject: &str) -> Result<bool, String> {
    let Some(guard) = lock() else {
        return Err("revocation store unavailable".to_string());
    };
    Ok(guard.subjects.contains(subject))
}

// ─── Kernel implementations ───────────────────────────────────────────────────
//
// These are the Ipê-facing Task-returning functions that correspond to the
// `Ipe.Auth.Revocation` stdlib surface:
//   revokeUser    : Principal -> Subject -> Task Error ()
//   revokeSession : Principal -> SessionId -> Task Error ()
//   restoreUser   : Principal -> Subject -> Task Error ()
//   isRevoked     : Subject -> Task Error Bool
//
// The `Principal` parameter enforces that only an authenticated caller can write
// to the store — an unauthenticated Ipê term cannot produce a `Principal`.

/// `Ipe.Auth.Revocation.revokeUser : Principal -> String -> Task Error ()`.
/// Marks every session of `subject` revoked. Requires an authenticated `Principal`
/// (only an authenticated caller can revoke).
pub fn auth_revocation_revoke_user<E: From<String> + Send + 'static>(
    _caller: crate::principal::Principal,
    subject: String,
) -> IpeTask<E, ()> {
    Box::pin(async move {
        match revoke_subject(subject) {
            Ok(()) => IpeResult::Ok(()),
            Err(e) => IpeResult::Err(format!("Auth.Revocation.revokeUser: {e}").into()),
        }
    })
}

/// `Ipe.Auth.Revocation.revokeSession : Principal -> String -> Task Error ()`.
/// Marks the specific session `jti` revoked. Requires an authenticated `Principal`.
pub fn auth_revocation_revoke_session<E: From<String> + Send + 'static>(
    _caller: crate::principal::Principal,
    jti: String,
) -> IpeTask<E, ()> {
    Box::pin(async move {
        match revoke_session(jti) {
            Ok(()) => IpeResult::Ok(()),
            Err(e) => IpeResult::Err(format!("Auth.Revocation.revokeSession: {e}").into()),
        }
    })
}

/// `Ipe.Auth.Revocation.restoreUser : Principal -> String -> Task Error ()`.
/// Clears the subject-level revocation. Requires an authenticated `Principal`.
pub fn auth_revocation_restore_user<E: From<String> + Send + 'static>(
    _caller: crate::principal::Principal,
    subject: String,
) -> IpeTask<E, ()> {
    Box::pin(async move {
        match restore_subject(&subject) {
            Ok(()) => IpeResult::Ok(()),
            Err(e) => IpeResult::Err(format!("Auth.Revocation.restoreUser: {e}").into()),
        }
    })
}

/// `Ipe.Auth.Revocation.isRevoked : String -> Task Error Bool`.
/// Queries whether `subject` is in the subject-revocation set. No `Principal`
/// required — this is a read-only query intended for admin/UI flows.
pub fn auth_revocation_is_revoked<E: From<String> + Send + 'static>(
    subject: String,
) -> IpeTask<E, bool> {
    Box::pin(async move {
        match subject_is_revoked(&subject) {
            Ok(b) => IpeResult::Ok(b),
            Err(e) => IpeResult::Err(format!("Auth.Revocation.isRevoked: {e}").into()),
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::principal::principal_mint;

    // Each test uses unique subject/jti strings to avoid cross-test state
    // contamination (the store is process-global).

    #[test]
    fn active_when_not_revoked() {
        let v = is_revoked("fresh-user", "fresh-jti-001");
        assert!(
            matches!(v, Verdict::Active),
            "an unknown subject/jti must be Active"
        );
    }

    #[test]
    fn revoked_subject_denied() {
        revoke_subject("revoked-subject-001".to_string()).unwrap();
        assert!(
            matches!(
                is_revoked("revoked-subject-001", "any-jti"),
                Verdict::Revoked
            ),
            "a revoked subject must yield Revoked"
        );
    }

    #[test]
    fn revoked_session_denied_other_sessions_unaffected() {
        // Revoke one specific jti.
        revoke_session("revoked-jti-001".to_string()).unwrap();
        // The revoked session is denied.
        assert!(
            matches!(is_revoked("subj-001", "revoked-jti-001"), Verdict::Revoked),
            "a revoked session must yield Revoked"
        );
        // A different session of the same subject is unaffected.
        assert!(
            matches!(is_revoked("subj-001", "other-jti-001"), Verdict::Active),
            "other sessions of the same subject must remain Active"
        );
    }

    #[test]
    fn restore_clears_subject_revocation() {
        revoke_subject("restore-subj-001".to_string()).unwrap();
        assert!(
            matches!(is_revoked("restore-subj-001", "any-jti"), Verdict::Revoked),
            "must be Revoked before restore"
        );
        restore_subject("restore-subj-001").unwrap();
        assert!(
            matches!(is_revoked("restore-subj-001", "any-jti-b"), Verdict::Active),
            "must be Active after restore"
        );
    }

    #[tokio::test]
    async fn revoke_user_kernel_requires_principal() {
        let p = principal_mint("admin".to_string());
        let result: IpeResult<String, ()> =
            auth_revocation_revoke_user(p, "kernel-subj-001".to_string()).await;
        assert!(
            matches!(result, IpeResult::Ok(())),
            "revokeUser with a Principal must succeed"
        );
        assert!(
            matches!(is_revoked("kernel-subj-001", "jti"), Verdict::Revoked),
            "subject must be Revoked after kernel call"
        );
    }

    #[tokio::test]
    async fn restore_user_kernel_re_allows() {
        let p = principal_mint("admin".to_string());
        let p2 = principal_mint("admin".to_string());
        let _: IpeResult<String, ()> =
            auth_revocation_revoke_user(p, "restore-kernel-subj-001".to_string()).await;
        let r: IpeResult<String, ()> =
            auth_revocation_restore_user(p2, "restore-kernel-subj-001".to_string()).await;
        assert!(matches!(r, IpeResult::Ok(())), "restoreUser must succeed");
        assert!(
            matches!(
                is_revoked("restore-kernel-subj-001", "jti"),
                Verdict::Active
            ),
            "subject must be Active after restoreUser"
        );
    }

    #[tokio::test]
    async fn is_revoked_kernel_reflects_store() {
        let p = principal_mint("admin".to_string());
        let _: IpeResult<String, ()> =
            auth_revocation_revoke_user(p, "is-revoked-subj-001".to_string()).await;
        let r: IpeResult<String, bool> =
            auth_revocation_is_revoked("is-revoked-subj-001".to_string()).await;
        assert!(
            matches!(r, IpeResult::Ok(true)),
            "isRevoked must return true for a revoked subject"
        );
        let r2: IpeResult<String, bool> =
            auth_revocation_is_revoked("not-revoked-subj-999".to_string()).await;
        assert!(
            matches!(r2, IpeResult::Ok(false)),
            "isRevoked must return false for a non-revoked subject"
        );
    }
}
