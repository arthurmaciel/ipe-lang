//! The authenticated `Principal` — the verified subject of a request.
//!
//! A `Principal` names the caller a row-security policy filters on. Its subject
//! string is only ever a value that a cryptographically verified, unexpired
//! session token carried: the sole producer is [`principal_mint`], which the
//! HTTP-server auth middleware calls exclusively on the success branch of token
//! verification. No other runtime path and no Ipê term can build one — the field
//! is private and there is no public constructor — so holding a `Principal` is
//! proof the subject was authenticated.

/// The verified subject of an authenticated request. The inner subject is
/// private: a value of this type can only originate from [`principal_mint`].
///
/// Deliberately NOT serde: a `Principal` must never round-trip through a session
/// store or JSON boundary, or a client could forge an authenticated identity by
/// supplying the serialized datum. Minting is the only way in.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Principal {
    subject: String,
}

/// Mint a `Principal` from a verified subject claim. Crate-internal: the auth
/// middleware is the only caller, and it invokes this solely after a successful
/// token verification, so every `Principal` in existence carries a subject that
/// a valid session proved. Not a registered kernel and not reachable from Ipê.
/// Gated on `jwt` to match its sole caller (the token-verifying middleware).
#[cfg(any(feature = "jwt", test))]
#[must_use]
pub(crate) fn principal_mint(subject: String) -> Principal {
    Principal { subject }
}

/// Ipê `Ipe.Auth.subject : Principal -> String` — the verified subject claim.
#[must_use]
pub fn principal_subject(p: Principal) -> String {
    p.subject
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn subject_round_trips_the_minted_value() {
        let p = principal_mint("user-42".to_string());
        assert_eq!(principal_subject(p), "user-42");
    }

    #[test]
    fn distinct_subjects_are_unequal() {
        assert_ne!(
            principal_mint("a".to_string()),
            principal_mint("b".to_string())
        );
    }
}
