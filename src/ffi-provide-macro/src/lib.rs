//! The `#[ipe::provide]` marker attribute for Tier 2 FFI wrapper crates.
//!
//! Some crate types need a real hand-written `impl Trait` whose derive is
//! outside Ipê's closed modellable set — a Bevy `#[derive(Component)]` /
//! `Resource`, an Iced widget trait, any framework contract that only a
//! hand-authored impl can satisfy. Those types cannot be *declared* through the
//! closed Tier 1 `[rust.provide.*]` forms, so the author writes them as normal
//! Rust in a `[rust.wrapper]` crate and tags the type with `#[ipe::provide]`.
//!
//! The macro is INERT: it re-emits the annotated item token-for-token and adds
//! one pure-data marker so the Ipê FFI inspector can find, out of the crate's
//! rustdoc JSON, exactly which hand-written items to surface. It generates no
//! trait impl, no glue, no logic — the author's Rust is bound as *inspected
//! wrapper symbols* (opaque nominal + carrier-compatible forwarders), never
//! injected into emitted `.ipe`. The Tier 2 guarantees are unchanged: the
//! wrapper is sandbox-built, source panic-scanned, and bound under the
//! owned-only, over-drop discipline like any other wrapper symbol.
//!
//! Author usage:
//!
//! ```ignore
//! // wrappers/src/lib.rs — a normal Rust wrapper crate depending on `ipe_provide`.
//! use ipe_provide::provide as ipe_provide_attr; // or `use ipe_provide::provide;`
//!
//! #[ipe_provide::provide]
//! pub struct Widget { /* … */ }
//!
//! impl some_framework::Component for Widget { /* hand-written impl */ }
//! ```

use proc_macro::TokenStream;

/// The pure-data breadcrumb the inert attribute attaches to a marked item. It is
/// a plain `#[doc = "…"]` string, so it survives macro expansion into the item's
/// rustdoc `attrs` array where the inspector reads it as a boolean "is this item
/// marked". The string is DATA the inspector matches on and never renders, so it
/// cannot become a code/TOML injection vector. Kept in sync, by convention, with
/// the inspector's `IPE_PROVIDE_MARKER` constant.
const MARKER_DOC: &str = concat!(" ", "ipe-provide-marker: this item is surfaced to Ipê FFI");

/// Tag a hand-written wrapper item as an Ipê FFI Tier 2 escape-hatch symbol.
///
/// Attach to a `struct`/`enum`/`fn` in a `[rust.wrapper]` crate whose behaviour
/// only a real hand-written `impl Trait` can express (outside the closed Tier 1
/// forms). The Ipê FFI inspector then surfaces the item as a wrapper-exposed
/// symbol — an Ipê-held opaque nominal plus its carrier-compatible forwarders —
/// without the author having to list it in `[rust.wrapper] expose`.
///
/// The attribute is inert: it returns the annotated item UNCHANGED, preceded
/// only by the pure-data [`MARKER_DOC`] breadcrumb. It never inspects, rewrites,
/// or generates code, so it can neither fail to parse the item nor alter its
/// meaning — the item compiles exactly as if the attribute were absent.
#[proc_macro_attribute]
pub fn provide(_attr: TokenStream, item: TokenStream) -> TokenStream {
    // Prepend `#[doc = MARKER_DOC]` to the item, unchanged. Building the tokens
    // by parsing a fixed, self-authored source string keeps this free of any
    // `syn`/`quote` dependency; the string is a compile-time constant we wrote,
    // so its parse is total in practice. `parse()` is fallible only on malformed
    // Rust tokens — impossible for this fixed literal — so on the unreachable
    // error we fall back to returning the item untouched (still a sound,
    // compilable pass-through; only the marker is absent).
    let doc_attr = format!("#[doc = {MARKER_DOC:?}]")
        .parse::<TokenStream>()
        .unwrap_or_else(|_| TokenStream::new());
    let mut out = TokenStream::new();
    out.extend(doc_attr);
    out.extend(item);
    out
}
