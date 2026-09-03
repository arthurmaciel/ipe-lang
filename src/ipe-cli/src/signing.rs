//! Publisher-identity provenance over the index-pinned `sha256`.
//!
//! The registry's trust root is verify-before-trust: at resolve time the client
//! git-fetches a version's pinned `rev` and refuses unless the fetched tree's
//! content hash equals the entry's `sha256` (`crate::resolve`). Signing layers
//! PUBLISHER IDENTITY over that anchor. A publisher signs an in-toto/DSSE
//! statement whose subject digest is exactly the entry's `sha256`, keyless:
//! the signature is made under a Fulcio-issued ephemeral certificate bound to a
//! GitHub Actions OIDC identity, with a Rekor transparency-log inclusion proof.
//! The client verifies that a *trusted* identity produced a signature over the
//! *pinned* digest before trusting the version.
//!
//! # The asymmetry: present-but-untrusted is worse than absent
//!
//! A missing signature is a known-unknown — the publisher never asserted
//! identity. A signature that does not verify against a trusted identity is a
//! present, active claim that failed: it may be an attacker's. So the policy is
//! deliberately asymmetric and fail-closed:
//!
//! | signature | `require_signature` | outcome                         |
//! |-----------|---------------------|---------------------------------|
//! | absent    | `false`             | resolve, with a warning         |
//! | absent    | `true`              | REJECT                          |
//! | present   | either              | verify or REJECT — never ignore |
//!
//! The load-bearing rule is the last row: **a version carrying a signature MUST
//! verify against a trusted identity, or the version is rejected**, regardless
//! of `require_signature`. A signature the client cannot validate never
//! resolves — you can neither silently drop it (it might be genuine and pin a
//! real trust decision) nor silently accept it (it might be forged).
//!
//! # Deny by default
//!
//! [`TrustPolicy::trusted_identities`] is EMPTY until a project or global config
//! adds `[registry.trust]` identities. With no trusted identity configured, any
//! present signature fails to match and the version is rejected — the safe
//! default. `require_signature` is `false` pre-1.0 so unsigned versions still
//! resolve (with a warning) until the registry's publishers have signed.
//!
//! # Verification seam
//!
//! [`SignatureVerifier`] is the trait the decision core calls. The concrete
//! Sigstore-backed implementation lives behind the `signing` feature (it pulls a
//! heavy async/TLS graph); the always-compiled decision core
//! ([`evaluate_signature`]) is exercised in tests against a mock verifier, so the
//! deny-by-default and present-but-untrusted rules are covered by the default
//! gate without linking the crypto crate.

use crate::CliError;

/// The maximum size, in bytes, of a Sigstore bundle carried in an index entry.
///
/// A DSSE bundle is a small JSON document (a cert chain, a signature, a Rekor
/// inclusion proof). A value far larger than any real bundle is a malformed or
/// hostile field, refused before it is parsed rather than allocated in full.
const MAX_BUNDLE_BYTES: usize = 512 * 1024;

/// A trusted signing identity: an OIDC issuer paired with the exact certificate
/// identity (subject-alternative-name) the issuer must have bound.
///
/// Both fields are matched EXACTLY against the verified certificate — there is
/// no wildcard or prefix match. A GitHub Actions publisher is, for example,
/// issuer `https://token.actions.githubusercontent.com` and identity
/// `https://github.com/owner/repo/.github/workflows/publish.yml@refs/heads/main`.
/// Exact match is the conservative choice: a looser pattern would admit any
/// workflow in the org, widening the trusted set beyond the one the user named.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Identity {
    issuer: String,
    san: String,
}

impl Identity {
    /// Parse a configured `(issuer, identity)` pair into a typed [`Identity`].
    ///
    /// Both values must be non-empty and free of control/whitespace characters —
    /// a certificate SAN or OIDC issuer is a URL-shaped token, and a value with
    /// embedded control characters is malformed config, never a real identity.
    ///
    /// # Errors
    /// [`CliError::Resolve`] when either field is empty or carries a control or
    /// whitespace character.
    pub fn parse(issuer: &str, san: &str) -> Result<Self, CliError> {
        let clean = |label: &str, raw: &str| -> Result<String, CliError> {
            let trimmed = raw.trim();
            if trimmed.is_empty() || trimmed.chars().any(|c| c.is_control() || c.is_whitespace()) {
                return Err(CliError::Resolve(format!(
                    "registry trust: `{label}` must be a non-empty token with no whitespace or \
                     control characters, got: {raw:?}"
                )));
            }
            Ok(trimmed.to_owned())
        };
        Ok(Self {
            issuer: clean("issuer", issuer)?,
            san: clean("identity", san)?,
        })
    }

    /// The OIDC issuer this identity requires (e.g. the GitHub Actions issuer).
    #[must_use]
    pub fn issuer(&self) -> &str {
        &self.issuer
    }

    /// The exact certificate SAN this identity requires.
    #[must_use]
    pub fn san(&self) -> &str {
        &self.san
    }
}

/// The client's signature trust policy, deny-by-default.
///
/// [`Self::trusted_identities`] starts EMPTY: absent an explicit
/// `[registry.trust]` identity, no present signature can match, so any signed
/// version is rejected — fail closed. [`Self::require_signature`] defaults to
/// `false` (pre-1.0), so an unsigned version still resolves with a warning; set
/// it `true` to refuse any version that carries no signature.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct TrustPolicy {
    trusted_identities: Vec<Identity>,
    require_signature: bool,
}

impl TrustPolicy {
    /// Build a policy from a set of trusted identities and the require flag.
    ///
    /// An empty `trusted_identities` is the deny-by-default policy.
    #[must_use]
    pub const fn new(trusted_identities: Vec<Identity>, require_signature: bool) -> Self {
        Self {
            trusted_identities,
            require_signature,
        }
    }

    /// The trusted identities. Empty means no signed version can be trusted.
    #[must_use]
    pub fn trusted_identities(&self) -> &[Identity] {
        &self.trusted_identities
    }

    /// Whether an unsigned version is refused.
    #[must_use]
    pub const fn require_signature(&self) -> bool {
        self.require_signature
    }
}

/// A parsed reference to a version's Sigstore bundle.
///
/// Parse, don't validate: a present bundle field is turned into this typed value
/// at read time. The bundle bytes must be non-empty, within
/// [`MAX_BUNDLE_BYTES`], and a JSON object (the DSSE-bundle envelope shape) — a
/// truncated, oversized, or non-object value is a HARD error, never a
/// silently-dropped field. The full cryptographic validation (cert chain, Rekor
/// proof, subject digest) happens in [`SignatureVerifier::verify`]; this
/// boundary only guarantees the field is a well-formed bundle envelope so a
/// malformed value cannot masquerade as "no signature".
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SignatureBundle {
    /// The raw bundle JSON, retained verbatim for the verifier. Kept as the
    /// original bytes (not a re-serialized value) so verification sees exactly
    /// what the registry served.
    raw: String,
}

impl SignatureBundle {
    /// Parse a raw bundle string from an index entry into a typed
    /// [`SignatureBundle`], refusing an empty, oversized, or non-object value.
    ///
    /// # Errors
    /// [`CliError::Resolve`] when the bundle is empty, exceeds
    /// [`MAX_BUNDLE_BYTES`], is not valid JSON, or is not a JSON object.
    pub fn parse(pkg: &str, raw: &str) -> Result<Self, CliError> {
        let refuse = |detail: &str| {
            CliError::Resolve(format!(
                "package `{pkg}`: signature bundle is malformed ({detail})"
            ))
        };
        let trimmed = raw.trim();
        if trimmed.is_empty() {
            return Err(refuse("empty bundle"));
        }
        if trimmed.len() > MAX_BUNDLE_BYTES {
            return Err(refuse("bundle exceeds the size ceiling"));
        }
        // A DSSE Sigstore bundle is a JSON object. Parsing here rejects a
        // truncated or injection-shaped value at the boundary so it can never
        // reach the verifier — or, worse, be silently treated as absent.
        let value: serde_json::Value =
            serde_json::from_str(trimmed).map_err(|e| refuse(&format!("not valid JSON: {e}")))?;
        if !value.is_object() {
            return Err(refuse("bundle top level is not a JSON object"));
        }
        Ok(Self {
            raw: trimmed.to_owned(),
        })
    }

    /// The raw bundle JSON, as served, for the verifier.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.raw
    }
}

/// The identity a verifier confirmed produced a valid signature.
///
/// The `(issuer, san)` extracted from the verified certificate, returned by a
/// successful [`SignatureVerifier::verify`] so the caller can report *who*
/// signed, not merely that verification passed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct VerifiedIdentity {
    /// The OIDC issuer bound in the verified certificate.
    pub issuer: String,
    /// The certificate SAN bound in the verified certificate.
    pub san: String,
}

/// A typed signature-verification failure, distinct from the ordinary
/// resolve/parse errors so a caller can tell "the crypto did not check out" from
/// "the field was malformed".
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SignatureError {
    /// The bundle's cryptographic material did not verify against the trust root
    /// (bad cert chain, bad Rekor proof, or a bad signature).
    BundleInvalid { detail: String },
    /// The bundle verified, but its signed subject digest did not equal the
    /// entry's pinned `sha256` — the signature is over other bytes.
    DigestMismatch { expected: String, signed: String },
    /// The bundle verified, but the certificate's `(issuer, san)` is not among
    /// the policy's trusted identities.
    UntrustedIdentity { issuer: String, san: String },
    /// The concrete verifier is not compiled in (the `signing` feature is off),
    /// so a present signature cannot be validated — fail closed.
    VerifierUnavailable,
}

impl std::fmt::Display for SignatureError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::BundleInvalid { detail } => {
                write!(f, "signature bundle did not verify: {detail}")
            }
            Self::DigestMismatch { expected, signed } => write!(
                f,
                "signature is over digest {signed} but the entry pins {expected} — the signature \
                 does not cover the published source"
            ),
            Self::UntrustedIdentity { issuer, san } => write!(
                f,
                "signature verified but its identity (issuer {issuer:?}, identity {san:?}) is not \
                 in the configured `[registry.trust]` allowlist"
            ),
            Self::VerifierUnavailable => f.write_str(
                "a signature is present but this build cannot verify Sigstore bundles \
                 (built without the `signing` feature) — refusing rather than trusting \
                 an unverifiable signature",
            ),
        }
    }
}

/// The seam the decision core calls to verify a bundle.
///
/// The concrete implementation is Sigstore-backed and feature-gated; tests
/// inject a mock so the deny-by-default policy is exercised without the crypto
/// crate or network.
pub trait SignatureVerifier {
    /// Verify `bundle` offline against the public-good trust root, requiring that
    /// (a) the cert chain and Rekor inclusion proof are valid, (b) the signed
    /// subject digest equals `subject_digest`, and (c) the certificate's
    /// `(issuer, san)` is one of `policy.trusted_identities()`.
    ///
    /// # Errors
    /// A [`SignatureError`] for any of the three failures above. On success,
    /// returns the [`VerifiedIdentity`] that signed.
    fn verify(
        &self,
        bundle: &SignatureBundle,
        subject_digest: &str,
        policy: &TrustPolicy,
    ) -> Result<VerifiedIdentity, SignatureError>;
}

/// What signature verification decided for one version.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SignatureOutcome {
    /// No signature was present and the policy does not require one — resolve,
    /// but carry the reason so the caller can warn.
    UnsignedAllowed,
    /// A signature was present and verified against a trusted identity — resolve.
    Verified(VerifiedIdentity),
}

/// The deny-by-default, fail-closed decision core over one version's signature.
///
/// This is the whole trust-model asymmetry in one place, and it is always
/// compiled (the concrete verifier is injected, so this logic is tested against
/// a mock):
///
/// - no signature + `require_signature == false` ⟹ [`SignatureOutcome::UnsignedAllowed`];
/// - no signature + `require_signature == true`  ⟹ [`CliError::Resolve`] (reject);
/// - a signature present ⟹ it MUST verify against a trusted identity, else
///   [`CliError::Resolve`] (reject) — regardless of `require_signature`.
///
/// # Errors
/// [`CliError::Resolve`] when a required signature is absent, or when a present
/// signature fails any verification check (invalid bundle, digest mismatch, or
/// untrusted identity).
pub fn evaluate_signature(
    pkg: &str,
    policy: &TrustPolicy,
    signature: Option<&SignatureBundle>,
    subject_digest: &str,
    verifier: &dyn SignatureVerifier,
) -> Result<SignatureOutcome, CliError> {
    signature.map_or_else(
        // Absent signature: fail closed only when the policy requires one.
        || {
            if policy.require_signature() {
                Err(CliError::Resolve(format!(
                    "package `{pkg}`: no publisher signature is present, but the configured \
                     registry trust policy requires one (`require_signature = true`) — refusing \
                     to resolve an unsigned version"
                )))
            } else {
                Ok(SignatureOutcome::UnsignedAllowed)
            }
        },
        // Present signature: it MUST verify against a trusted identity, else the
        // version is rejected — regardless of `require_signature`.
        |bundle| {
            verifier
                .verify(bundle, subject_digest, policy)
                .map(SignatureOutcome::Verified)
                .map_err(|e| {
                    CliError::Resolve(format!(
                        "package `{pkg}`: a publisher signature is present but was not trusted \
                         — {e}"
                    ))
                })
        },
    )
}

/// Parse a `[registry.trust]` table from config TOML into a [`TrustPolicy`].
///
/// Absent table, absent keys, or an empty identity list all yield the
/// deny-by-default policy (empty trusted set, `require_signature = false`) — a
/// config that says nothing trusts nothing.
///
/// The expected shape:
///
/// ```toml
/// [registry.trust]
/// require_signature = false
/// trusted_identities = [
///   { issuer = "https://token.actions.githubusercontent.com", identity = "https://github.com/owner/repo/.github/workflows/publish.yml@refs/heads/main" },
/// ]
/// ```
///
/// # Errors
/// [`CliError::Resolve`] when the TOML does not parse, when `[registry.trust]`
/// is present but malformed (a non-array `trusted_identities`, an entry missing
/// `issuer`/`identity`), or when an identity fails [`Identity::parse`]. A
/// malformed trust config is a hard error, never silently downgraded to "trust
/// nothing" — a typo in an allowlist must not quietly widen (or here, void) the
/// trusted set.
pub fn parse_trust_policy_toml(text: &str) -> Result<TrustPolicy, CliError> {
    let refuse =
        |detail: &str| CliError::Resolve(format!("registry trust config is malformed ({detail})"));
    let table: toml::Table = text
        .parse()
        .map_err(|e| refuse(&format!("not valid TOML: {e}")))?;

    let Some(trust) = table
        .get("registry")
        .and_then(toml::Value::as_table)
        .and_then(|reg| reg.get("trust"))
    else {
        // No `[registry.trust]` at all — the deny-by-default policy.
        return Ok(TrustPolicy::default());
    };
    let trust = trust
        .as_table()
        .ok_or_else(|| refuse("`registry.trust` is not a table"))?;

    let require_signature = match trust.get("require_signature") {
        None => false,
        Some(v) => v
            .as_bool()
            .ok_or_else(|| refuse("`require_signature` must be a boolean"))?,
    };

    let mut identities = Vec::new();
    if let Some(raw) = trust.get("trusted_identities") {
        let array = raw
            .as_array()
            .ok_or_else(|| refuse("`trusted_identities` must be an array"))?;
        for entry in array {
            let item = entry
                .as_table()
                .ok_or_else(|| refuse("each trusted identity must be a table"))?;
            let issuer = item
                .get("issuer")
                .and_then(toml::Value::as_str)
                .ok_or_else(|| refuse("a trusted identity is missing string `issuer`"))?;
            let san = item
                .get("identity")
                .and_then(toml::Value::as_str)
                .ok_or_else(|| refuse("a trusted identity is missing string `identity`"))?;
            identities.push(Identity::parse(issuer, san)?);
        }
    }

    Ok(TrustPolicy::new(identities, require_signature))
}

/// Load the client's [`TrustPolicy`] from config, merging the global
/// `$IPE_HOME/config.toml` with the project's `ipe.toml`.
///
/// Both files may carry a `[registry.trust]` table. The merge is UNION-safe: the
/// trusted-identity sets are unioned (a project may add trusted identities but
/// never has to re-declare the global ones), and `require_signature` is the OR
/// of both (either file may tighten the policy; neither can loosen the other).
/// A file that is absent contributes the empty deny-by-default policy; a file
/// that is present but malformed is a hard error (never silently ignored).
///
/// # Errors
/// [`CliError::Resolve`] when a present config file cannot be read or its
/// `[registry.trust]` is malformed.
pub fn load_trust_policy(project_root: &std::path::Path) -> Result<TrustPolicy, CliError> {
    let mut identities: Vec<Identity> = Vec::new();
    let mut require_signature = false;

    let mut absorb = |path: std::path::PathBuf| -> Result<(), CliError> {
        let text = match crate::io_bounded::read_to_string_capped(
            &path,
            crate::io_bounded::SMALL_FILE_READ_CAP,
        ) {
            Ok(t) => t,
            Err(CliError::Io { ref source, .. })
                if source.kind() == std::io::ErrorKind::NotFound =>
            {
                return Ok(());
            }
            Err(e) => return Err(e),
        };
        let policy = parse_trust_policy_toml(&text)?;
        require_signature = require_signature || policy.require_signature();
        for id in policy.trusted_identities() {
            if !identities.contains(id) {
                identities.push(id.clone());
            }
        }
        Ok(())
    };

    // Global config first, then the project — order is immaterial to the union.
    if let Ok(home) = crate::runtime_embed::ipe_home() {
        absorb(home.join("config.toml"))?;
    }
    absorb(project_root.join("ipe.toml"))?;

    Ok(TrustPolicy::new(identities, require_signature))
}

/// The GitHub Actions OIDC issuer — the only keyless issuer the registry uses.
///
/// Surfaced so config parsing and the publish-side statement generator name one
/// constant rather than repeating the URL.
pub const GITHUB_ACTIONS_ISSUER: &str = "https://token.actions.githubusercontent.com";

/// Build the in-toto/DSSE **statement** a publisher signs for one version.
///
/// The statement is an in-toto Statement whose single subject is named
/// `<name>@<version>` with a `sha256` digest equal to the entry's pinned
/// `sha256`.
///
/// Binding the subject digest to the already-pinned `sha256` is what ties the
/// signature to the exact source the resolver hash-verifies: a signature over
/// this statement is a signature over those bytes. The publisher feeds this
/// statement to keyless `cosign`/`gh attest` in CI, which mints the Fulcio cert,
/// signs, and records the Rekor entry, producing the bundle that goes into the
/// index entry's `signature` field.
///
/// The returned JSON is deterministic (sorted keys) so the same version always
/// yields the same statement.
#[must_use]
pub fn dsse_statement(name: &str, version: &str, sha256: &str) -> String {
    // A minimal in-toto Statement (predicateType left generic): the load-bearing
    // content is the subject name + sha256 digest that the signature covers.
    let statement = serde_json::json!({
        "_type": "https://in-toto.io/Statement/v1",
        "subject": [ {
            "name": format!("{name}@{version}"),
            "digest": { "sha256": sha256 },
        } ],
        "predicateType": "https://ipe-lang.org/registry/publish/v1",
        "predicate": {},
    });
    // `serde_json::to_string` over a `json!` value emits object keys in
    // insertion order; the fixed insertion order above makes this deterministic.
    serde_json::to_string(&statement).unwrap_or_default()
}

/// A verifier that refuses every present signature.
///
/// Used when no crypto backend is compiled in (the `signing` feature is off).
/// Fail-closed: a present signature that cannot be verified is rejected, never
/// trusted.
///
/// The decision core still runs — an unsigned version resolves under this
/// verifier exactly as it would under the real one (the verifier is only
/// consulted for a PRESENT signature), so a default build behaves identically
/// for the unsigned pre-1.0 registry while refusing to trust any signature it
/// cannot check.
pub struct UnavailableVerifier;

impl SignatureVerifier for UnavailableVerifier {
    fn verify(
        &self,
        _bundle: &SignatureBundle,
        _subject_digest: &str,
        _policy: &TrustPolicy,
    ) -> Result<VerifiedIdentity, SignatureError> {
        Err(SignatureError::VerifierUnavailable)
    }
}

#[cfg(feature = "signing")]
pub use sigstore_impl::{SigstoreVerifier, vendored_sigstore_verifier};

/// The Sigstore-backed [`SignatureVerifier`]. Compiled only under the `signing`
/// feature — it pulls the heavy async/TLS Sigstore graph.
///
/// # Crypto-integration fork (documented, deliberate)
///
/// `sigstore` 0.14's `verify_digest(input_digest: sha2::Sha256, …)` takes a
/// `Sha256` HASHER and finalizes it internally; for the DSSE case it compares
/// `hex(input_digest.finalize())` against the bundle's subject digest. There is
/// no public constructor that injects a PRECOMPUTED digest into a `sha2::Sha256`
/// hasher — a hasher's `finalize()` is determined by the bytes fed to it, which
/// requires the original artifact bytes at verify time. Our registry pins a
/// PRECOMPUTED content hash (a tree hash from `cache::hash_tree`), not a raw
/// byte stream sigstore can rehash, so the pinned digest cannot be soundly fed
/// through this API. Rather than double-hash (which would silently verify the
/// wrong digest), [`vendored_sigstore_verifier`] returns `None` and the
/// fail-closed `UnavailableVerifier` governs — a present signature is refused,
/// never trusted on an unsound bridge. The trust-root + identity-policy wiring
/// below is complete and compiles against the real API; only the
/// precomputed-digest → `verify_digest` bridge is the open fork.
#[cfg(feature = "signing")]
mod sigstore_impl {
    use super::{
        Identity, SignatureBundle, SignatureError, SignatureVerifier, TrustPolicy, VerifiedIdentity,
    };

    use sigstore::bundle::verify::blocking::Verifier;
    use sigstore::rekor::apis::configuration::Configuration as RekorConfiguration;
    use sigstore::trust::ManualTrustRoot;

    /// A [`SignatureVerifier`] verifying a bundle OFFLINE against a manual root.
    ///
    /// Offline: the bundle carries its own Rekor inclusion proof, so no network
    /// call is made at verify time.
    ///
    /// The verifier enforces exactly the policy asymmetry the decision core
    /// relies on: the bundle must cryptographically verify, its subject digest
    /// must equal the pinned `sha256`, and its certificate `(issuer, san)` must
    /// be one of the policy's trusted identities. Any failure is a typed
    /// [`SignatureError`].
    ///
    /// The trust root is a [`ManualTrustRoot`] rather than the TUF-fetched
    /// public-good root, so no `sigstore-trust-root`/`tough` TUF client is linked
    /// and verification never touches the network. The registry supplies the
    /// vendored Fulcio root certificates and the Rekor / CT-log public keys.
    pub struct SigstoreVerifier {
        trust_root: ManualTrustRoot<'static>,
    }

    impl SigstoreVerifier {
        /// Construct a verifier over an EMPTY [`ManualTrustRoot`].
        ///
        /// The registry supplies the real Fulcio root certificates and Rekor /
        /// CT-log public keys when it ships signed packages (parsed from the
        /// vendored `trusted_root.json` — the material the TUF client would
        /// otherwise fetch). Until then this trust root is empty, so
        /// [`vendored_sigstore_verifier`] never hands this verifier to the
        /// resolver; a present signature is refused by the fail-closed path.
        #[must_use]
        const fn with_empty_root() -> Self {
            Self {
                trust_root: ManualTrustRoot {
                    fulcio_certs: Vec::new(),
                    rekor_keys: std::collections::BTreeMap::new(),
                    ctfe_keys: std::collections::BTreeMap::new(),
                },
            }
        }

        /// The exact-match certificate-identity policy for one trusted identity.
        fn identity_policy(identity: &Identity) -> sigstore::bundle::verify::policy::Identity {
            sigstore::bundle::verify::policy::Identity::new(identity.san(), identity.issuer())
        }
    }

    impl SignatureVerifier for SigstoreVerifier {
        fn verify(
            &self,
            bundle: &SignatureBundle,
            _subject_digest: &str,
            policy: &TrustPolicy,
        ) -> Result<VerifiedIdentity, SignatureError> {
            // Parse into the crate's typed `Bundle` and build the identity
            // policies — proving the bundle + identity-policy wiring is correct
            // against the real API before the digest bridge is reached.
            let _parsed: sigstore::bundle::Bundle =
                serde_json::from_str(bundle.as_str()).map_err(|e| {
                    SignatureError::BundleInvalid {
                        detail: format!("bundle is not a Sigstore bundle: {e}"),
                    }
                })?;
            let _identity_policies: Vec<_> = policy
                .trusted_identities()
                .iter()
                .map(Self::identity_policy)
                .collect();

            // Build the blocking verifier from THIS verifier's trust root, proving
            // the `Verifier::new(RekorConfiguration, ManualTrustRoot)` wiring on
            // the hot path. `ManualTrustRoot` is not `Clone`, so rebuild it from
            // the (clonable) fields.
            let root = ManualTrustRoot {
                fulcio_certs: self.trust_root.fulcio_certs.clone(),
                rekor_keys: self.trust_root.rekor_keys.clone(),
                ctfe_keys: self.trust_root.ctfe_keys.clone(),
            };
            let _verifier = Verifier::new(RekorConfiguration::default(), root).map_err(|e| {
                SignatureError::BundleInvalid {
                    detail: format!("could not build the verifier: {e}"),
                }
            })?;

            // The digest bridge is the open fork (see the module doc): the pinned
            // value is a precomputed tree hash, but `verify_digest` takes a
            // `sha2::Sha256` HASHER it finalizes, with no way to inject a
            // precomputed digest. Refuse rather than verify against a wrong digest.
            Err(SignatureError::BundleInvalid {
                detail: "cannot bridge the registry's precomputed content hash to sigstore \
                         0.14's `verify_digest(Sha256 hasher)` API — refusing rather than \
                         verifying against a re-hashed (wrong) digest"
                    .to_owned(),
            })
        }
    }

    /// The vendored public-good Sigstore trusted-root JSON.
    ///
    /// Embedded at build time so offline verification never fetches it over TUF.
    /// Empty until the registry ships signed packages and the parse into Fulcio
    /// roots + Rekor/CT-log keys is supplied; while empty, no verifier is
    /// available and a present signature falls to the fail-closed
    /// `UnavailableVerifier`.
    const VENDORED_TRUSTED_ROOT: &[u8] = include_bytes!("signing/trusted_root.json");

    /// Build the offline [`SigstoreVerifier`], or `None` when unavailable.
    ///
    /// Returns `None` while the trust material is absent AND while the
    /// precomputed-digest bridge (the documented fork) is open, so a present
    /// signature is refused rather than trusted on an unsound path.
    #[must_use]
    pub fn vendored_sigstore_verifier() -> Option<SigstoreVerifier> {
        // While no trust material is vendored, the verifier would carry an empty
        // trust root; the digest bridge is also still open (see the module doc).
        // Build the empty-root verifier to keep the construction path exercised,
        // then decline it so the fail-closed `UnavailableVerifier` governs.
        if VENDORED_TRUSTED_ROOT.is_empty() {
            let _unusable = SigstoreVerifier::with_empty_root();
            return None;
        }
        None
    }

    #[cfg(test)]
    mod feature_tests {
        use super::{
            SignatureBundle, SignatureVerifier as _, SigstoreVerifier, TrustPolicy,
            vendored_sigstore_verifier,
        };

        #[test]
        fn verify_over_empty_root_reaches_the_documented_fork() {
            // The bundle-parse + verifier-build wiring runs; the precomputed-digest
            // bridge is the open fork, so a present signature is refused.
            let verifier = SigstoreVerifier::with_empty_root();
            let bundle = SignatureBundle::parse("p", r#"{"dsseEnvelope":{}}"#).expect("bundle");
            let err = verifier
                .verify(&bundle, "00", &TrustPolicy::default())
                .expect_err("must refuse at the digest fork");
            assert!(format!("{err}").contains("did not verify"), "{err}");
        }

        #[test]
        fn no_vendored_material_means_no_verifier() {
            // Deny by default while the trust material + digest bridge are open.
            assert!(vendored_sigstore_verifier().is_none());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A 64-char lowercase-hex digest fixture (a real sha256 shape).
    const DIGEST: &str = "abcabcabcabcabcabcabcabcabcabcabcabcabcabcabcabcabcabcabcabcabcab";

    /// A minimal well-formed JSON-object bundle. The typed [`SignatureBundle`]
    /// only requires a JSON object at the parse boundary; cryptographic content
    /// is the verifier's concern (mocked in these tests).
    fn valid_bundle_json() -> &'static str {
        r#"{ "mediaType": "application/vnd.dev.sigstore.bundle+json;version=0.3",
             "verificationMaterial": {}, "dsseEnvelope": {} }"#
    }

    /// A mock verifier scripted to accept or reject, so the decision core's
    /// asymmetry is exercised without the crypto crate.
    struct MockVerifier {
        result: Result<VerifiedIdentity, SignatureError>,
    }

    impl MockVerifier {
        fn accepting() -> Self {
            Self {
                result: Ok(VerifiedIdentity {
                    issuer: GITHUB_ACTIONS_ISSUER.to_owned(),
                    san: "https://github.com/o/r/.github/workflows/publish.yml@refs/heads/main"
                        .to_owned(),
                }),
            }
        }
        fn rejecting(err: SignatureError) -> Self {
            Self { result: Err(err) }
        }
    }

    impl SignatureVerifier for MockVerifier {
        fn verify(
            &self,
            _bundle: &SignatureBundle,
            _subject_digest: &str,
            _policy: &TrustPolicy,
        ) -> Result<VerifiedIdentity, SignatureError> {
            self.result.clone()
        }
    }

    fn trusted_identity() -> Identity {
        Identity::parse(
            GITHUB_ACTIONS_ISSUER,
            "https://github.com/o/r/.github/workflows/publish.yml@refs/heads/main",
        )
        .expect("valid identity")
    }

    // ── Identity parse boundary ─────────────────────────────────────────────

    #[test]
    fn identity_parse_accepts_url_shaped_pair() {
        let id = trusted_identity();
        assert_eq!(id.issuer(), GITHUB_ACTIONS_ISSUER);
        assert!(id.san().contains("publish.yml"));
    }

    #[test]
    fn identity_parse_rejects_empty_and_whitespace() {
        assert!(
            Identity::parse("", "san").is_err(),
            "empty issuer must fail"
        );
        assert!(Identity::parse("iss", "").is_err(), "empty san must fail");
        assert!(
            Identity::parse("iss with space", "san").is_err(),
            "whitespace in issuer must fail"
        );
        assert!(
            Identity::parse("iss", "san\twith\ttab").is_err(),
            "control char in san must fail"
        );
    }

    // ── SignatureBundle parse boundary ──────────────────────────────────────

    #[test]
    fn bundle_parse_accepts_a_json_object() {
        let b = SignatureBundle::parse("p", valid_bundle_json()).expect("object parses");
        assert!(b.as_str().contains("dsseEnvelope"));
    }

    #[test]
    fn bundle_parse_rejects_empty() {
        let err = SignatureBundle::parse("p", "   ").unwrap_err();
        assert!(format!("{err}").contains("empty"), "{err}");
    }

    #[test]
    fn bundle_parse_rejects_non_json() {
        let err = SignatureBundle::parse("p", "this is not json ===").unwrap_err();
        assert!(format!("{err}").contains("malformed"), "{err}");
    }

    #[test]
    fn bundle_parse_rejects_a_non_object_json() {
        // A JSON array/string/number is valid JSON but not a bundle envelope.
        let err = SignatureBundle::parse("p", "[1, 2, 3]").unwrap_err();
        assert!(format!("{err}").contains("not a JSON object"), "{err}");
    }

    #[test]
    fn bundle_parse_rejects_a_partial_truncated_object() {
        // A truncated object is a malformed field, never silently "no signature".
        let err = SignatureBundle::parse("p", r#"{ "verificationMaterial": {"#).unwrap_err();
        assert!(format!("{err}").contains("malformed"), "{err}");
    }

    #[test]
    fn bundle_parse_rejects_oversized() {
        let huge = format!("{{\"x\":\"{}\"}}", "a".repeat(MAX_BUNDLE_BYTES + 1));
        let err = SignatureBundle::parse("p", &huge).unwrap_err();
        assert!(format!("{err}").contains("size ceiling"), "{err}");
    }

    // ── evaluate_signature: the deny-by-default asymmetry ───────────────────

    #[test]
    fn unsigned_with_require_false_resolves_with_warning() {
        let policy = TrustPolicy::new(vec![trusted_identity()], false);
        let verifier = MockVerifier::accepting();
        let outcome = evaluate_signature("p", &policy, None, DIGEST, &verifier)
            .expect("unsigned + !require resolves");
        assert_eq!(outcome, SignatureOutcome::UnsignedAllowed);
    }

    #[test]
    fn unsigned_with_require_true_is_rejected() {
        let policy = TrustPolicy::new(vec![trusted_identity()], true);
        let verifier = MockVerifier::accepting();
        let err = evaluate_signature("p", &policy, None, DIGEST, &verifier)
            .expect_err("unsigned + require must reject");
        assert!(format!("{err}").contains("requires one"), "{err}");
    }

    #[test]
    fn present_and_verified_against_trusted_identity_resolves() {
        let policy = TrustPolicy::new(vec![trusted_identity()], false);
        let verifier = MockVerifier::accepting();
        let bundle = SignatureBundle::parse("p", valid_bundle_json()).expect("bundle");
        let outcome = evaluate_signature("p", &policy, Some(&bundle), DIGEST, &verifier)
            .expect("verified signature resolves");
        assert!(matches!(outcome, SignatureOutcome::Verified(_)));
    }

    #[test]
    fn present_but_untrusted_identity_is_rejected_even_when_require_is_false() {
        // The load-bearing asymmetry: a present signature that fails to match a
        // trusted identity is rejected REGARDLESS of require_signature.
        let policy = TrustPolicy::new(vec![trusted_identity()], false);
        let verifier = MockVerifier::rejecting(SignatureError::UntrustedIdentity {
            issuer: "https://evil.example".to_owned(),
            san: "https://github.com/attacker/repo/...".to_owned(),
        });
        let bundle = SignatureBundle::parse("p", valid_bundle_json()).expect("bundle");
        let err = evaluate_signature("p", &policy, Some(&bundle), DIGEST, &verifier)
            .expect_err("present-but-untrusted must reject");
        let msg = format!("{err}");
        assert!(msg.contains("not trusted"), "{msg}");
        assert!(msg.contains("allowlist"), "{msg}");
    }

    #[test]
    fn present_with_digest_mismatch_is_rejected() {
        // A signature over other bytes than the pinned sha256 is rejected: the
        // provenance must cover exactly the source the resolver hash-verifies.
        let policy = TrustPolicy::new(vec![trusted_identity()], false);
        let verifier = MockVerifier::rejecting(SignatureError::DigestMismatch {
            expected: DIGEST.to_owned(),
            signed: "0000000000000000000000000000000000000000000000000000000000000000".to_owned(),
        });
        let bundle = SignatureBundle::parse("p", valid_bundle_json()).expect("bundle");
        let err = evaluate_signature("p", &policy, Some(&bundle), DIGEST, &verifier)
            .expect_err("digest mismatch must reject");
        assert!(format!("{err}").contains("does not cover"), "{err}");
    }

    #[test]
    fn present_with_empty_trusted_set_is_rejected_deny_by_default() {
        // Deny by default: with no trusted identity configured, a real verifier
        // matches nothing. Model that here with a rejecting verifier and an empty
        // policy — the version is refused.
        let policy = TrustPolicy::default();
        assert!(policy.trusted_identities().is_empty());
        assert!(!policy.require_signature());
        let verifier = MockVerifier::rejecting(SignatureError::UntrustedIdentity {
            issuer: "<none configured>".to_owned(),
            san: "<none configured>".to_owned(),
        });
        let bundle = SignatureBundle::parse("p", valid_bundle_json()).expect("bundle");
        let err = evaluate_signature("p", &policy, Some(&bundle), DIGEST, &verifier)
            .expect_err("signed version with empty allowlist must reject");
        assert!(format!("{err}").contains("not trusted"), "{err}");
    }

    #[test]
    fn invalid_bundle_crypto_is_rejected() {
        let policy = TrustPolicy::new(vec![trusted_identity()], false);
        let verifier = MockVerifier::rejecting(SignatureError::BundleInvalid {
            detail: "bad Rekor inclusion proof".to_owned(),
        });
        let bundle = SignatureBundle::parse("p", valid_bundle_json()).expect("bundle");
        let err = evaluate_signature("p", &policy, Some(&bundle), DIGEST, &verifier)
            .expect_err("invalid bundle must reject");
        assert!(format!("{err}").contains("did not verify"), "{err}");
    }

    #[test]
    fn verifier_unavailable_is_rejected_fail_closed() {
        // A present signature in a build without the crypto verifier fails closed:
        // an unverifiable signature is worse than none.
        let policy = TrustPolicy::new(vec![trusted_identity()], false);
        let verifier = MockVerifier::rejecting(SignatureError::VerifierUnavailable);
        let bundle = SignatureBundle::parse("p", valid_bundle_json()).expect("bundle");
        let err = evaluate_signature("p", &policy, Some(&bundle), DIGEST, &verifier)
            .expect_err("no verifier + present signature must reject");
        assert!(format!("{err}").contains("cannot verify"), "{err}");
    }

    // ── DSSE statement generation ───────────────────────────────────────────

    #[test]
    fn dsse_statement_binds_name_version_and_sha256() {
        let stmt = dsse_statement("http-extras", "1.2.0", DIGEST);
        assert!(stmt.contains("http-extras@1.2.0"), "{stmt}");
        assert!(stmt.contains(DIGEST), "{stmt}");
        assert!(stmt.contains("in-toto.io/Statement"), "{stmt}");
        // Parse it back to prove it is well-formed JSON with the subject digest.
        let value: serde_json::Value = serde_json::from_str(&stmt).expect("valid JSON");
        let signed_digest = value
            .pointer("/subject/0/digest/sha256")
            .and_then(serde_json::Value::as_str);
        assert_eq!(signed_digest, Some(DIGEST));
    }

    // ── Trust-policy config parsing ─────────────────────────────────────────

    #[test]
    fn trust_config_absent_table_is_deny_by_default() {
        let policy = parse_trust_policy_toml("name = \"app\"\n").expect("parses");
        assert!(policy.trusted_identities().is_empty());
        assert!(!policy.require_signature());
    }

    #[test]
    fn trust_config_reads_identities_and_require_flag() {
        let text = r#"
[registry.trust]
require_signature = true
trusted_identities = [
  { issuer = "https://token.actions.githubusercontent.com", identity = "https://github.com/o/r/.github/workflows/publish.yml@refs/heads/main" },
]
"#;
        let policy = parse_trust_policy_toml(text).expect("parses");
        assert!(policy.require_signature());
        assert_eq!(policy.trusted_identities().len(), 1);
        let id = policy.trusted_identities().first().expect("one identity");
        assert_eq!(id.issuer(), GITHUB_ACTIONS_ISSUER);
        assert!(id.san().contains("publish.yml"));
    }

    #[test]
    fn trust_config_rejects_malformed_toml() {
        let err = parse_trust_policy_toml("this is not toml ===").unwrap_err();
        assert!(format!("{err}").contains("malformed"), "{err}");
    }

    #[test]
    fn trust_config_rejects_identity_missing_issuer() {
        let text = r#"
[registry.trust]
trusted_identities = [ { identity = "https://github.com/o/r/wf@refs/heads/main" } ]
"#;
        let err = parse_trust_policy_toml(text).unwrap_err();
        assert!(format!("{err}").contains("issuer"), "{err}");
    }

    #[test]
    fn trust_config_rejects_non_array_identities() {
        let text = "[registry.trust]\ntrusted_identities = \"nope\"\n";
        let err = parse_trust_policy_toml(text).unwrap_err();
        assert!(format!("{err}").contains("array"), "{err}");
    }

    #[test]
    fn trust_config_rejects_non_bool_require() {
        let text = "[registry.trust]\nrequire_signature = \"yes\"\n";
        let err = parse_trust_policy_toml(text).unwrap_err();
        assert!(format!("{err}").contains("boolean"), "{err}");
    }

    #[test]
    fn dsse_statement_is_deterministic() {
        let a = dsse_statement("p", "1.0.0", DIGEST);
        let b = dsse_statement("p", "1.0.0", DIGEST);
        assert_eq!(a, b, "the statement for a version must be stable");
    }
}
