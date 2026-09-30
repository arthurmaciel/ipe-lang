//! Names every path the runtime `clippy.toml` denies.
//!
//! An entry marked `allow-invalid` is skipped by clippy without a word once its
//! path stops resolving, so each one is spelled here under the feature that
//! brings its crate in: a renamed or removed path breaks the `--all-targets`
//! build instead of quietly disabling its lint. The `lenient_decode_scan` test
//! asserts this file names exactly the `clippy.toml` set.

#[allow(deprecated, clippy::disallowed_methods)] // naming each denied path, never calling it, is the point
const _STD: () = {
    let _ = ::std::env::home_dir;
    let _ = ::std::string::String::from_utf8_lossy;
};

#[cfg(feature = "encoding")]
#[allow(clippy::disallowed_methods)] // naming each denied path, never calling it, is the point
const _PERCENT_ENCODING: () = {
    let _ = ::percent_encoding::percent_decode_str;
    let _ = ::percent_encoding::percent_decode;
    let _ = ::percent_encoding::PercentDecode::decode_utf8_lossy;
};

#[cfg(feature = "url")]
#[allow(clippy::disallowed_methods)] // naming each denied path, never calling it, is the point
const _URL: () = {
    let _ = ::url::form_urlencoded::parse;
    let _ = ::url::Url::query_pairs;
};

#[cfg(feature = "web-core")]
#[allow(clippy::disallowed_methods)] // naming each denied path, never calling it, is the point
const _SERDE_URLENCODED: () = {
    let _ = ::serde_urlencoded::from_str::<()>;
    let _ = ::serde_urlencoded::from_bytes::<()>;
    let _ = ::serde_urlencoded::from_reader::<(), &[u8]>;
};

#[cfg(feature = "server")]
#[allow(clippy::disallowed_types)] // naming each denied path, never using it, is the point
const _AXUM: () = {
    let _: Option<::axum::extract::Query<()>> = None;
    let _: Option<::axum::extract::Form<()>> = None;
};
