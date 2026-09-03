//! Minimal blocking HTTP GET for the CLI's own network needs.
//!
//! One dependency-light client with the same rustls backend the runtime uses,
//! serving the release version check; the compiler/runtime HTTP surface is a
//! separate, feature-gated concern.

use std::time::Duration;

/// GET `url`, returning the response body as a string.
///
/// `None` on any failure (DNS, connect, TLS, non-2xx, timeout, oversized body):
/// a network error is never fatal here — the caller decides how to degrade.
#[must_use]
pub fn get(url: &str) -> Option<String> {
    get_with_accept(url, "application/vnd.github+json")
}

/// GET `url`, returning the response body as a string, requesting `accept` as
/// the `Accept` header.
///
/// `None` on any failure (DNS, connect, TLS, non-2xx, timeout, oversized body):
/// a network error is never fatal here — the caller decides how to degrade. The
/// registry's static Pages read API serves `application/json`, so the registry
/// fetch passes that rather than the GitHub API media type.
#[must_use]
pub fn get_with_accept(url: &str, accept: &str) -> Option<String> {
    let agent = ureq::AgentBuilder::new()
        .timeout(Duration::from_secs(5))
        .build();
    let response = agent
        .get(url)
        .set("User-Agent", "ipe-cli")
        .set("Accept", accept)
        .call()
        .ok()?;
    response.into_string().ok()
}
