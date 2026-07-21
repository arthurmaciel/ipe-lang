//! The injectable Ipê interface module — the consumer-side seed artifact.
//!
//! For each bound crate the driver injects one ordinary, fully-annotated Ipê
//! module (`module Rust.Semver exposing (…)`) whose value bodies are
//! `Ffi.binding "<wrapper_fn_ident>" a0 …` forwarders. FFI signatures thus
//! flow through the SAME annotation → `Ty` path every user annotation takes:
//! there is no second, hand-maintained scheme table to drift against
//! (the kernel-registry design's OPEN DECISION 1, resolved by construction).
//!
//! Inclusion is gated fail-closed: a function reaches the interface only when
//! its wrapper region actually exists in `_bindings.rs`, its signature's
//! opaque foreign types all resolve to unambiguous Rust paths, and no foreign
//! type shadows an Ipê reserved builtin type. Anything else is skipped with a
//! reason (over-drop, never an under-bind that `cargo` rejects).

use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;

use crate::emit::{opaque_names_in, wrapper_ipe_signature};
use crate::pkginfo::{FnInfo, PkgInfo};

/// Ipê keywords that can never be a binding name in the generated module.
const IPE_KEYWORDS: &[&str] = &[
    "module", "import", "exposing", "type", "alias", "let", "in", "case", "of", "if", "then",
    "else", "as", "port",
];

/// One binding included in the interface module.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InterfaceBinding {
    /// The Ipê-visible name (= `wrapper_ref_name`, the `kernel.json` key).
    pub ref_name: String,
    /// The `_bindings.rs` wrapper `pub fn` identifier the body forwards to.
    pub wrapper_ident: String,
    /// Ipê-side arity (unit param for a zero-arg foreign fn).
    pub arity: usize,
    /// The full Ipê HM signature string.
    pub sig: String,
}

/// A binding excluded from the interface, with the reason — surfaced in the
/// coverage report so an over-drop is visible, never silent.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SkippedBinding {
    /// The `wrapper_ref_name` of the skipped function.
    pub ref_name: String,
    /// Why it was excluded.
    pub reason: String,
}

/// The complete consumer-side view of one bound crate.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CrateInterface {
    /// Ipê module qualifier, e.g. `Rust.Semver`.
    pub module_name: String,
    /// Kernel-name prefix, e.g. `Rust_Semver`.
    pub kernel_name: String,
    /// The injectable Ipê module source.
    pub source: String,
    /// Opaque foreign type name → absolute Rust path (`Version` →
    /// `::semver::Version`), for backend type rendering.
    pub opaque_types: BTreeMap<String, String>,
    /// Opaque foreign type name → the type's canonical DEFINING path (the
    /// rustdoc `paths` identity). Drives cross-crate nominal unification:
    /// two member modules whose same-named opaques carry the SAME defining
    /// path are the SAME Rust type and collapse to one Ipê nominal. A name
    /// absent here (older cache / no recoverable identity) never unifies.
    pub opaque_type_ids: BTreeMap<String, String>,
    /// The nominal names this crate's `[rust.provide.struct/enum]` decls DEFINE
    /// (`Counter`, `Message`). Unlike [`Self::opaque_types`] — external crate
    /// types the inspector found at an absolute `::crate::Path` — a provide type
    /// is DEFINED in the emitted `_bindings.rs` and lives at
    /// `crate::ffi::<slug>::<Name>`. The slug is not known here (the interface
    /// generator has only the `PkgInfo`), so the crate-local path is assembled
    /// downstream (`assemble_emit`) where the slug is; this set is the ground
    /// truth for WHICH names are provide-defined so the two paths never blur.
    pub provide_types: BTreeSet<String>,
    /// The included bindings.
    pub bindings: Vec<InterfaceBinding>,
    /// The excluded bindings, with reasons.
    pub skipped: Vec<SkippedBinding>,
}

/// Collect `Name → ::absolute::path` for every nominal foreign type mentioned
/// in the package's Rust type strings.
///
/// A base name claimed by two DIFFERENT paths is poisoned (removed): two
/// distinct foreign types would otherwise unify nominally on the Ipê side.
/// Poisoned names travel back so the per-fn gate can drop their users.
fn foreign_path_map(pkg: &PkgInfo) -> (BTreeMap<String, String>, BTreeSet<String>) {
    let mut map: BTreeMap<String, String> = BTreeMap::new();
    let mut poisoned: BTreeSet<String> = BTreeSet::new();
    let mut visit = |raw: &str| {
        for (base, path) in path_tokens(raw) {
            match map.get(&base) {
                Some(prev) if *prev != path => {
                    poisoned.insert(base);
                }
                Some(_) => {}
                None => {
                    map.insert(base, path);
                }
            }
        }
    };
    for f in pkg.fns() {
        visit(f.recv_rust_type());
        for p in f.params().iter().chain(f.results().iter()) {
            visit(p.rust_type_str());
            visit(&p.foreign_ty);
        }
    }
    for name in &poisoned {
        map.remove(name);
    }
    (map, poisoned)
}

/// Extract every `seg::…::Base` path token from a Rust type string, returning
/// `(Base, ::seg::…::Base)` pairs. Generic arguments split into their own
/// tokens; a bare identifier (no `::`) carries no path and is skipped.
fn path_tokens(raw: &str) -> Vec<(String, String)> {
    let mut out = Vec::new();
    let mut token = String::new();
    let flush = |token: &mut String, out: &mut Vec<(String, String)>| {
        if token.contains("::") {
            let normalized = if token.starts_with("::") {
                token.clone()
            } else {
                format!("::{token}")
            };
            if let Some(base) = normalized.rsplit("::").next()
                && !base.is_empty()
                && base.chars().next().is_some_and(char::is_uppercase)
            {
                out.push((base.to_owned(), normalized.clone()));
                // A SUBMODULE type surfaces under the inspector's
                // path-derived Ipê head (`checkout_session::ProductData` →
                // `Checkout_sessionProductData`); the map must answer for
                // that key too or every submodule type is "unresolvable".
                let composite = ipe_head_from_rust_path(&normalized);
                if !composite.is_empty() && composite != *base {
                    out.push((composite, normalized));
                }
            }
        }
        token.clear();
    };
    for c in raw.chars() {
        if c.is_alphanumeric() || c == '_' || c == ':' {
            token.push(c);
        } else {
            flush(&mut token, &mut out);
        }
    }
    flush(&mut token, &mut out);
    out
}

/// The inspector's path-derived Ipê head for a qualified Rust type path —
/// submodule segments CamelCase-join ahead of the type name so same-named
/// types in different submodules stay distinct (`::regex::bytes::Regex` →
/// `BytesRegex`; `::stripe_checkout::checkout_session::ProductData` →
/// `Checkout_sessionProductData`); a crate-root type keeps its bare name
/// unless that name collides with an Ipê builtin carrier, in which case the
/// crate segment joins too (`::bytes::Bytes` → `BytesBytes`). MUST mirror the
/// inspector's `ipe_name_from_path` — a drift makes submodule types
/// "unresolvable" and silently over-drops their whole surface.
fn ipe_head_from_rust_path(path: &str) -> String {
    fn camel(s: &str) -> String {
        let mut c = s.chars();
        c.next().map_or_else(String::new, |f| {
            f.to_uppercase().collect::<String>() + c.as_str()
        })
    }
    let segs: Vec<&str> = path.split("::").filter(|s| !s.is_empty()).collect();
    match segs.as_slice() {
        [] => String::new(),
        [one] => (*one).to_owned(),
        [crate_seg, mods @ .., ty] => {
            let builtin_collision =
                mods.is_empty() && matches!(*ty, "Bytes" | "String" | "Int" | "Float" | "Bool");
            let mut out = String::new();
            if builtin_collision {
                out.push_str(&camel(crate_seg));
            }
            for m in mods {
                out.push_str(&camel(m));
            }
            out.push_str(ty);
            out
        }
    }
}

/// Collect every foreign NOMINAL base name reachable from one Rust type
/// string — the generic HEAD and each generic ARGUMENT, recursively.
///
/// `stripe::Response<stripe::CheckoutSession>` yields both `Response` and
/// `CheckoutSession`; `Vec<semver::Error>` yields `Error`. A base is a
/// capitalised identifier (a type), never a scalar/lifetime/module segment.
fn foreign_nominal_bases(raw: &str, out: &mut BTreeSet<String>) {
    /// Bare (path-less) std heads whose Ipê mapping IS the builtin — `String`
    /// is the `String` carrier, `Vec` is `List`, … The inspector renders
    /// crate-local types with a qualified path, so a bare occurrence of one of
    /// these is std by construction, never a foreign nominal shadowing a
    /// builtin.
    const BARE_STD_CARRIERS: &[&str] = &[
        "String", "Vec", "Option", "Result", "HashMap", "BTreeMap", "HashSet", "BTreeSet",
    ];
    let mut token = String::new();
    let flush = |token: &mut String, out: &mut BTreeSet<String>| {
        // The last `::`-segment of the token is the type's own name.
        let base = token.rsplit("::").next().unwrap_or(token);
        let bare_std = !token.contains("::") && BARE_STD_CARRIERS.contains(&base);
        if base.chars().next().is_some_and(char::is_uppercase) && !bare_std {
            out.insert(base.to_owned());
        }
        token.clear();
    };
    for c in raw.chars() {
        if c.is_alphanumeric() || c == '_' || c == ':' {
            token.push(c);
        } else {
            // `<`, `,`, `>`, `&`, ` `, `(`, `)` all break a nominal token —
            // so a generic head and its args each flush separately.
            flush(&mut token, out);
        }
    }
    flush(&mut token, out);
}

/// The Ok arm of a `Result<Ok, Err>` type string (any path prefix), else the
/// input unchanged. A foreign error type in the ERR position folds into the
/// typed Ipê `Error` at the wrapper boundary — it never reaches the Ipê
/// signature, so it must be excluded from the reserved-collision scan (else a
/// legitimate `Result<Version, semver::Error>` would be over-dropped on its
/// harmless `Error` arm).
fn result_ok_arm(raw: &str) -> &str {
    let Some(open) = raw.find("Result<") else {
        return raw;
    };
    let inner = raw.get(open + "Result<".len()..).unwrap_or("");
    let mut depth = 0_i32;
    for (i, c) in inner.char_indices() {
        match c {
            '<' | '(' => depth += 1,
            '>' | ')' => depth -= 1,
            ',' if depth == 0 => return inner.get(..i).unwrap_or(inner).trim(),
            _ => {}
        }
    }
    inner
}

/// The first foreign nominal in `f`'s parameter / result / receiver types
/// that collides with an Ipê reserved builtin type name, if any.
///
/// Two scans per param: the RAW Rust type through the rust-syntax nominal
/// tokenizer (catches a foreign type folding onto a builtin HEAD), and the
/// Ipê-typed rendering through the ipe-syntax opaque scan — there the builtin
/// heads (`Result`/`Maybe`/`Task`/`Error`, …) are the language's own
/// containers, never foreign nominals, so tokenizing them as foreign would
/// over-drop every fallible binding on its own carrier.
fn foreign_reserved_collision(f: &FnInfo) -> Option<String> {
    let mut bases = BTreeSet::new();
    for p in f.params().iter().chain(f.results().iter()) {
        foreign_nominal_bases(result_ok_arm(p.rust_type_str()), &mut bases);
        opaque_names_in(&p.foreign_ty, &mut bases);
    }
    foreign_nominal_bases(f.recv_rust_type(), &mut bases);
    bases
        .into_iter()
        .find(|b| ipe_canon::is_reserved_builtin_type_name(b))
}

/// `true` when `name` is a well-formed Ipê value identifier the generated
/// module may bind: lowercase-led, alphanumeric/underscore, not a keyword.
fn valid_ipe_value_name(name: &str) -> bool {
    let mut chars = name.chars();
    chars.next().is_some_and(|c| c.is_ascii_lowercase())
        && chars.all(|c| c.is_alphanumeric() || c == '_')
        && !IPE_KEYWORDS.contains(&name)
}

/// `true` when an Ipê signature string contains a TUPLE — a parenthesised
/// region with a top-level comma (`(Int, Int)`); `Maybe (List Int)` has no
/// comma and stays clean.
fn contains_tuple(sig: &str) -> bool {
    let mut depth = 0_u32;
    for c in sig.chars() {
        match c {
            '(' => depth = depth.saturating_add(1),
            ')' => depth = depth.saturating_sub(1),
            ',' if depth > 0 => return true,
            _ => {}
        }
    }
    false
}

/// Build the consumer-side interface for one validated package.
#[must_use]
#[allow(clippy::too_many_lines)] // one linear per-binding gate cascade
pub fn crate_interface(pkg: &PkgInfo) -> CrateInterface {
    let module_name = crate::naming::rust_module_name(pkg.pkg_path());
    let kernel_name = crate::naming::rust_kernel_name(pkg.pkg_path());
    let survivors = crate::bindings::surviving_ref_names(pkg);
    let (path_map, poisoned) = foreign_path_map(pkg);

    let mut bindings: Vec<InterfaceBinding> = Vec::new();
    let mut skipped: Vec<SkippedBinding> = Vec::new();
    let mut seen: BTreeSet<String> = BTreeSet::new();
    let mut used_opaques: BTreeSet<String> = BTreeSet::new();
    let mut provide_types: BTreeSet<String> = BTreeSet::new();

    for f in pkg.fns() {
        let ref_name = f.wrapper_ref_name();
        let skip = |reason: &str, skipped: &mut Vec<SkippedBinding>| {
            skipped.push(SkippedBinding {
                ref_name: ref_name.clone(),
                reason: reason.to_owned(),
            });
        };
        if ref_name.starts_with('_') {
            continue; // internal probe artifact, not a binding
        }
        if !valid_ipe_value_name(&ref_name) {
            skip("name is not a legal Ipê identifier", &mut skipped);
            continue;
        }
        if f.generic().is_some_and(|g| !g.params.is_empty()) {
            skip(
                "parametric generic — monomorphised instances are not consumer-wired yet",
                &mut skipped,
            );
            continue;
        }
        // A closure adapter's wrapper takes an Ipê function value and returns a
        // boxed Rust closure surfaced as an opaque handle nominal. It is admitted
        // as an arity-1 Ipê forwarder — `(A -> B -> R) -> <Handle>` — so a program
        // can HOLD the closure and hand it to a foreign `run`-style entrypoint,
        // never seeing the `Box<dyn Fn …>` inside.
        //
        // The admission is gated on the emitter exactly like struct/enum: an
        // unresolvable/parameterised opaque param or return (`Element<'a, Msg>`)
        // over-drops the whole region in `_bindings.rs`, dropping the ref-name
        // from `survivors`. The emitter is the single resolvability oracle — the
        // `type <Handle>` alias and the wrapper share ONE region, so the interface
        // can never surface a forwarder onto an alias/wrapper that was not emitted
        // (a SEAL breach).
        if let crate::pkginfo::FnShape::ClosureAdapter { sig } = f.shape() {
            if survivors.contains(&ref_name) {
                admit_closure_forwarder(
                    &ref_name,
                    sig,
                    &kernel_name,
                    &mut bindings,
                    &mut skipped,
                    &mut seen,
                    &mut provide_types,
                );
            } else {
                skip(
                    "provide.closure with an unresolvable or parameterised opaque \
                     param/return — adapter over-dropped in _bindings.rs",
                    &mut skipped,
                );
            }
            continue;
        }
        // A provide.struct / provide.enum DEFINES an Ipê-held nominal Rust type
        // and admits its constructor(s) as Ipê forwarders. The whole surface is
        // synthesised from the parsed def (its `params()`/`results()` are empty —
        // it is a manifest entry, not an inspected fn), so its signature, arity,
        // and nominal name all come from the def, never from the empty fn shape.
        //
        // The forwarder is admitted only when the emitter kept the definition's
        // wrapper region: an opaque field/payload the crate cannot name (a bare or
        // lifetime/generic-parameterised handle) over-drops in `_bindings.rs`,
        // dropping the ref-name from `survivors`. Gating here keeps the interface
        // from surfacing a forwarder onto a wrapper fn that was never emitted (a
        // SEAL breach), with the emitter as the single resolvability oracle.
        if matches!(
            f.shape(),
            crate::pkginfo::FnShape::StructCtor { .. }
                | crate::pkginfo::FnShape::EnumDefCtor { .. }
        ) && !survivors.contains(&ref_name)
        {
            skip(
                "provide.struct/enum with an unresolvable or parameterised opaque \
                 field/payload — definition over-dropped in _bindings.rs",
                &mut skipped,
            );
            continue;
        }
        if let crate::pkginfo::FnShape::StructCtor { def } = f.shape() {
            admit_struct_forwarder(
                &ref_name,
                def,
                &kernel_name,
                &mut bindings,
                &mut skipped,
                &mut seen,
                &mut provide_types,
            );
            continue;
        }
        if let crate::pkginfo::FnShape::EnumDefCtor { def } = f.shape() {
            admit_enum_forwarders(
                &ref_name,
                def,
                &kernel_name,
                &mut bindings,
                &mut skipped,
                &mut seen,
                &mut provide_types,
            );
            continue;
        }
        if !survivors.contains(&ref_name) {
            skip("no wrapper region in _bindings.rs", &mut skipped);
            continue;
        }
        // A foreign nominal that collides with an Ipê reserved builtin type is
        // unsound TWO ways: one that folds onto a builtin HEAD (`semver::Error`
        // → the Ipê `Error`, while the wrapper keeps `semver::Error` — an
        // E0308), and one that would be DECLARED as an opaque `type X`
        // (`stripe::Response` → `IPE-N0026`). The raw-type scan below catches
        // the head-fold case (which never reaches the signature's opaque set);
        // the sig-opaque scan further down catches the declared-opaque case.
        if let Some(bad) = foreign_reserved_collision(f) {
            skip(
                &format!("foreign type `{bad}` shadows an Ipê reserved builtin type"),
                &mut skipped,
            );
            continue;
        }
        let sig = wrapper_ipe_signature(f);
        // A tuple anywhere in the signature renders as a Rust tuple whose
        // integer components keep their RAW widths (`(u64, u16)`), while the
        // Ipê signature maps every integer to `Int` (i64) — the forwarder
        // would be an E0308. Over-drop until tuple-component scalar coercion
        // is wired into the wrapper emitter.
        //
        // Two shapes coerce their tuple components and so type-check: a
        // by-borrow reader's receiver-threaded tuple (`(R, T)`), whose wrapper
        // coerces the result component before pairing it with the receiver
        // handle; and a plain multi-result tuple all of whose components are
        // owned scalars — a numeric width (widened to its `Int`/`Float` carrier),
        // an owned `String`, or a `bool` (each an identity coercion). Any other
        // tuple (a `&`-borrow, opaque handle, or nested-container component)
        // still over-drops until that wiring exists.
        if contains_tuple(&sig)
            && !f.is_borrow_reader()
            && !crate::bindings::multi_result_tuple_is_coercible(f)
        {
            skip(
                "tuple in signature needs component scalar coercion — not yet wired",
                &mut skipped,
            );
            continue;
        }
        // The opaque foreign types the SIGNATURE would declare (`type X`) —
        // the ground truth for both the reserved-builtin collision gate and
        // the path-resolvability gate. Reading the final signature (not the
        // raw `rust_type`) catches an inspector `ipeType` override that maps a
        // generic head like `stripe::Response<…>` to the bare `Response`.
        let mut opaques = BTreeSet::new();
        opaque_names_in(&sig, &mut opaques);
        if let Some(bad) = opaques
            .iter()
            .find(|n| ipe_canon::is_reserved_builtin_type_name(n))
        {
            skip(
                &format!("foreign type `{bad}` shadows an Ipê reserved builtin type"),
                &mut skipped,
            );
            continue;
        }
        if let Some(bad) = opaques.iter().find(|n| poisoned.contains(*n)) {
            skip(
                &format!("foreign type `{bad}` is claimed by two distinct Rust paths"),
                &mut skipped,
            );
            continue;
        }
        if let Some(bad) = opaques.iter().find(|n| !path_map.contains_key(*n)) {
            skip(
                &format!("foreign type `{bad}` has no resolvable Rust path"),
                &mut skipped,
            );
            continue;
        }
        if !seen.insert(ref_name.clone()) {
            skip(
                "duplicate binding name — first occurrence kept",
                &mut skipped,
            );
            continue;
        }
        used_opaques.extend(opaques);
        bindings.push(InterfaceBinding {
            wrapper_ident: crate::naming::wrapper_fn_ident(&kernel_name, &ref_name),
            arity: f.params().len().max(1),
            sig,
            ref_name,
        });
    }

    let opaque_types: BTreeMap<String, String> = used_opaques
        .iter()
        .filter_map(|n| path_map.get(n).map(|p| (n.clone(), p.clone())))
        .collect();
    let opaque_type_ids: BTreeMap<String, String> = opaque_types
        .iter()
        .filter_map(|(n, p)| {
            pkg.foreign_type_ids()
                .get(p)
                .map(|defid| (n.clone(), defid.clone()))
        })
        .collect();

    let source = render_module(
        &module_name,
        &BTreeMap::new(),
        &opaque_types,
        &provide_types,
        &bindings,
    );
    CrateInterface {
        module_name,
        kernel_name,
        source,
        opaque_types,
        opaque_type_ids,
        provide_types,
        bindings,
        skipped,
    }
}

/// Admit a `provide.struct` constructor as an Ipê forwarder, registering the
/// struct's nominal as a provide-defined type.
///
/// The whole surface is synthesised from the parsed [`StructDef`]: the forwarder
/// signature and arity come from the field carriers (the fn's own
/// `params()`/`results()` are empty — it is a manifest entry), and the nominal is
/// the struct's own name. Over-drops fail-closed:
///
/// * a constructor name that is not a legal Ipê value identifier is dropped;
/// * a duplicate constructor name keeps the first;
/// * the struct nominal routes through [`claim_nominal`], which refuses a
///   reserved-builtin shadow or a collision with another provide surface
///   (an admitted shadowing/colliding nominal is a silent-wrong-type or
///   duplicate-definition SEAL breach — refuse, never rename).
fn admit_struct_forwarder(
    ctor: &str,
    def: &crate::carrier::StructDef,
    kernel_name: &str,
    bindings: &mut Vec<InterfaceBinding>,
    skipped: &mut Vec<SkippedBinding>,
    seen: &mut BTreeSet<String>,
    provide_types: &mut BTreeSet<String>,
) {
    let type_name = def.name.as_str();
    if !valid_ipe_value_name(ctor) {
        skipped.push(SkippedBinding {
            ref_name: ctor.to_owned(),
            reason: "provide.struct constructor name is not a legal Ipê identifier".to_owned(),
        });
        return;
    }
    if !seen.insert(ctor.to_owned()) {
        skipped.push(SkippedBinding {
            ref_name: ctor.to_owned(),
            reason: "duplicate binding name — first occurrence kept".to_owned(),
        });
        return;
    }
    if let Err(reason) = claim_nominal("provide.struct", type_name, provide_types) {
        skipped.push(SkippedBinding {
            ref_name: ctor.to_owned(),
            reason,
        });
        return;
    }
    bindings.push(InterfaceBinding {
        wrapper_ident: crate::naming::wrapper_fn_ident(kernel_name, ctor),
        arity: def.fields.len(),
        sig: def.forwarder_ipe_sig(),
        ref_name: ctor.to_owned(),
    });
}

/// Admit each `provide.enum` variant constructor as an Ipê forwarder, registering
/// the enum's nominal ONCE.
///
/// The enum is one Rust type; its N per-variant constructor fns all return that
/// one nominal, so the nominal registers once and each variant forwarder differs
/// only in its value-level `ref_name` (`<ctor>_<snake(variant)>`) and arity.
/// Over-drops fail-closed:
///
/// * the enum nominal routes through [`claim_nominal`] up-front, which refuses a
///   reserved-builtin shadow or a collision with another provide surface,
///   dropping the WHOLE entry (all variant forwarders) — a shadowing/colliding
///   nominal is a silent-wrong-type or duplicate-definition SEAL breach;
/// * a per-variant constructor name that is not a legal Ipê value identifier is
///   dropped INDIVIDUALLY (each variant name is independent), the rest kept;
/// * a duplicate constructor name keeps the first.
fn admit_enum_forwarders(
    ref_name: &str,
    def: &crate::carrier::EnumDef,
    kernel_name: &str,
    bindings: &mut Vec<InterfaceBinding>,
    skipped: &mut Vec<SkippedBinding>,
    seen: &mut BTreeSet<String>,
    provide_types: &mut BTreeSet<String>,
) {
    let enum_name = def.name.as_str();
    // Claim the ONE shared nominal up-front so a reserved-builtin shadow or a
    // collision with another provide surface refuses the WHOLE entry fail-closed
    // — a variant forwarder must never point at an enum whose `type` was dropped.
    // An enum with no surviving variant un-claims the nominal at the tail.
    if let Err(reason) = claim_nominal("provide.enum", enum_name, provide_types) {
        skipped.push(SkippedBinding {
            ref_name: ref_name.to_owned(),
            reason,
        });
        return;
    }
    let mut any_admitted = false;
    for v in &def.variants {
        // The per-variant constructor `ref_name` — the SAME construction the
        // wrapper emitter uses for the emitted `pub fn`, so the forwarder's
        // `wrapper_ident` cannot drift from the fn it forwards to.
        let variant_ref = format!(
            "{ref_name}_{}",
            crate::naming::variant_snake(v.name.as_str())
        );
        if !valid_ipe_value_name(&variant_ref) {
            skipped.push(SkippedBinding {
                ref_name: variant_ref,
                reason: "provide.enum variant constructor name is not a legal Ipê identifier"
                    .to_owned(),
            });
            continue;
        }
        if !seen.insert(variant_ref.clone()) {
            skipped.push(SkippedBinding {
                ref_name: variant_ref,
                reason: "duplicate binding name — first occurrence kept".to_owned(),
            });
            continue;
        }
        bindings.push(InterfaceBinding {
            wrapper_ident: crate::naming::wrapper_fn_ident(kernel_name, &variant_ref),
            arity: v.payload.len(),
            sig: v.forwarder_ipe_sig(enum_name),
            ref_name: variant_ref,
        });
        any_admitted = true;
    }
    // Keep the nominal only when at least one variant forwarder survives — an
    // enum whose every variant name is illegal declares no reachable constructor,
    // so a bare `type` would be a dead opaque. Un-claim it otherwise (it was
    // claimed up-front for the fail-closed collision gate).
    if !any_admitted {
        provide_types.remove(enum_name);
    }
}

/// Admit a `provide.closure` adapter as an arity-1 Ipê forwarder, registering
/// its returned boxed closure's opaque handle nominal as a provide-defined type.
///
/// The wrapper takes ONE argument — the Ipê function value — and returns the
/// boxed closure surfaced as the handle nominal (see
/// [`crate::naming::closure_handle_nominal`]); the forwarder's Ipê signature is
/// `(A -> B -> R) -> <Handle>`, synthesised from the parsed [`ClosureSig`]'s
/// carriers, never the empty fn params. The handle registers in `provide_types`
/// so the backend resolves it at the crate-local `crate::ffi::<slug>::<Handle>`,
/// exactly as a provide-struct/enum nominal.
///
/// Over-drops fail-closed — the handle nominal is never renamed to dodge a clash:
///
/// * a `ref_name` that is not a legal Ipê value identifier, or a duplicate
///   `ref_name`, is dropped (keeping the first);
/// * the handle nominal routes through [`claim_nominal`], which refuses a
///   reserved-builtin shadow or a collision with any other provide surface
///   (struct / enum / another closure adapter) fail-closed.
fn admit_closure_forwarder(
    ref_name: &str,
    sig: &crate::carrier::ClosureSig,
    kernel_name: &str,
    bindings: &mut Vec<InterfaceBinding>,
    skipped: &mut Vec<SkippedBinding>,
    seen: &mut BTreeSet<String>,
    provide_types: &mut BTreeSet<String>,
) {
    if !valid_ipe_value_name(ref_name) {
        skipped.push(SkippedBinding {
            ref_name: ref_name.to_owned(),
            reason: "provide.closure adapter name is not a legal Ipê identifier".to_owned(),
        });
        return;
    }
    let handle = crate::naming::closure_handle_nominal(ref_name);
    if !seen.insert(ref_name.to_owned()) {
        skipped.push(SkippedBinding {
            ref_name: ref_name.to_owned(),
            reason: "duplicate binding name — first occurrence kept".to_owned(),
        });
        return;
    }
    if let Err(reason) = claim_nominal("provide.closure handle", &handle, provide_types) {
        skipped.push(SkippedBinding {
            ref_name: ref_name.to_owned(),
            reason,
        });
        return;
    }
    bindings.push(InterfaceBinding {
        wrapper_ident: crate::naming::wrapper_fn_ident(kernel_name, ref_name),
        arity: 1,
        sig: sig.forwarder_ipe_sig(&handle),
        ref_name: ref_name.to_owned(),
    });
}

/// Claim a provide-defined nominal for the interface, fail-closed.
///
/// EVERY provide surface — struct, enum, closure handle — DEFINES a bare
/// in-module `pub struct`/`pub enum`/`pub type` in the one emitted
/// `pub mod <slug>` region, so two distinct definitions that claim the SAME
/// nominal are an `E0428` the app crate cannot compile — an `ipe`-exit-0 ⇒
/// cargo-fail SEAL breach. Routing every claim through here makes "each provide
/// nominal registered at most once" the ONLY representable outcome, independent
/// of admission order (a name-collision is refused whichever surface declares it
/// second, never renamed to dodge the clash):
///
/// * a nominal shadowing an Ipê reserved builtin is refused (a shadowing nominal
///   is a silent-wrong-type breach);
/// * a nominal already claimed by another provide surface is refused.
///
/// Returns `Ok(())` with the nominal inserted, or `Err(reason)` naming the broken
/// rule for the caller to record as a skip.
fn claim_nominal(
    kind: &str,
    nominal: &str,
    provide_types: &mut BTreeSet<String>,
) -> Result<(), String> {
    if ipe_canon::is_reserved_builtin_type_name(nominal) {
        return Err(format!(
            "{kind} type `{nominal}` shadows an Ipê reserved builtin type"
        ));
    }
    if !provide_types.insert(nominal.to_owned()) {
        return Err(format!(
            "{kind} type `{nominal}` collides with another provide-defined nominal"
        ));
    }
    Ok(())
}

/// Render the injectable module text.
///
/// Opaque types are exported WITHOUT `(..)` so their placeholder constructor
/// never escapes the module; the lowerer additionally fails closed on any
/// constructor use of a foreign union.
///
/// `imports` (home module → type names) renders one
/// `import <Home> exposing (T, …)` line per entry: the catalog unification
/// demotes a re-declared foreign type to an import of its ONE home module, so
/// the importer's bare `T` canonicalises to the home's nominal.
pub fn render_module(
    module_name: &str,
    imports: &BTreeMap<String, BTreeSet<String>>,
    opaque_types: &BTreeMap<String, String>,
    provide_types: &BTreeSet<String>,
    bindings: &[InterfaceBinding],
) -> String {
    let mut exports: Vec<String> = opaque_types.keys().cloned().collect();
    exports.extend(provide_types.iter().cloned());
    exports.extend(bindings.iter().map(|b| b.ref_name.clone()));
    let mut out = format!("module {module_name} exposing ({})\n", exports.join(", "));
    for (home, names) in imports {
        let joined = names.iter().cloned().collect::<Vec<_>>().join(", ");
        let _ = write!(out, "\nimport {home} exposing ({joined})\n");
    }
    // Both an inspected opaque foreign type and a `provide`-defined nominal are
    // Ipê-held opaque handles — one nullary `type <Name> = <Name>` declaration,
    // exported WITHOUT `(..)` so the placeholder constructor never escapes. The
    // two differ only in their Rust PATH (external `::crate::T` vs crate-local
    // `crate::ffi::<slug>::T`), resolved downstream, never in their Ipê surface.
    for name in opaque_types.keys().chain(provide_types.iter()) {
        // Writing into a String is infallible.
        let _ = write!(out, "\ntype {name} = {name}\n");
    }
    for b in bindings {
        let args: Vec<String> = (0..b.arity).map(crate::naming::arg_name).collect();
        let args_joined = args.join(" ");
        // A nullary forwarder (a fieldless struct / unit enum variant) binds a
        // zero-arg `Ffi.binding "<wrapper>"` — the emitted wrapper `pub fn` is
        // itself zero-param, so forcing a spurious `arg0` here would be an arity
        // mismatch the app crate cannot compile.
        if b.arity == 0 {
            let _ = write!(
                out,
                "\n{} : {}\n{} =\n    Ffi.binding \"{}\"\n",
                b.ref_name, b.sig, b.ref_name, b.wrapper_ident
            );
        } else {
            let _ = write!(
                out,
                "\n{} : {}\n{} {} =\n    Ffi.binding \"{}\" {}\n",
                b.ref_name, b.sig, b.ref_name, args_joined, b.wrapper_ident, args_joined
            );
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pkginfo::PkgInfo;

    fn pkg() -> PkgInfo {
        let doc = serde_json::json!({
            "pkg": "semver",
            "name": "semver",
            "version": "1.0.26",
            "functions": [
                {
                    "name": "parse",
                    "params": [{"name": "text", "type": "&str", "ipeType": "String"}],
                    "results": [{"name": "", "type": "Result<Version, Error>",
                                 "rustType": "Result<semver::Version, semver::Error>"}],
                    "effect": "fallible"
                },
                {
                    "name": "major_field",
                    "params": [{"name": "self", "type": "&Version", "ipeType": "Version",
                                "rustType": "semver::Version"}],
                    "results": [{"name": "", "type": "u64", "rustType": "u64"}],
                    "effect": "pure",
                    "recvType": "Version",
                    "recvRustType": "semver::Version",
                    "methodName": "major",
                    "isField": true
                },
                {
                    "name": "explain",
                    "params": [{"name": "self", "type": "&Error", "ipeType": "Error",
                                "rustType": "semver::Error"}],
                    "results": [{"name": "", "type": "String"}],
                    "effect": "pure",
                    "recvType": "Error",
                    "recvRustType": "semver::Error"
                }
            ],
            "errors": [],
            "transitiveDeps": [
                {"ident": "semver", "name": "semver", "version": "1.0.26"}
            ],
            "foreignTypeIds": {
                "::semver::Version": "semver::version::Version"
            }
        });
        PkgInfo::decode_json(&doc.to_string()).expect("decodes")
    }

    #[test]
    fn interface_includes_survivors_and_maps_opaque_paths() {
        let iface = crate_interface(&pkg());
        assert_eq!(iface.module_name, "Rust.Semver");
        assert_eq!(iface.kernel_name, "Rust_Semver");
        let names: Vec<&str> = iface.bindings.iter().map(|b| b.ref_name.as_str()).collect();
        assert_eq!(
            names,
            vec!["parse", "major_field_from_version"],
            "{:?}",
            iface.skipped
        );
        assert_eq!(
            iface.opaque_types.get("Version").map(String::as_str),
            Some("::semver::Version")
        );
        // The defining-path identity rides along for catalog unification.
        assert_eq!(
            iface.opaque_type_ids.get("Version").map(String::as_str),
            Some("semver::version::Version")
        );
        // `Error` is a builtin head — never an opaque decl, and `explain`
        // (whose receiver is the foreign `semver::Error`) is dropped.
        assert!(!iface.opaque_types.contains_key("Error"));
        assert!(
            iface
                .skipped
                .iter()
                .any(|s| s.ref_name == "explain_from_error"
                    && s.reason.contains("shadows an Ipê reserved builtin type")),
            "{:?}",
            iface.skipped
        );
    }

    #[test]
    fn plain_multi_result_numeric_string_bool_tuple_is_admitted_not_dropped() {
        // A non-borrow-reader free fn returning a tuple of numeric / owned
        // `String` / `bool` components used to over-drop on the tuple gate; each
        // component is now coercible (numeric widens to its carrier, String/bool
        // ride identity), so it binds. A tuple carrying an OPAQUE component still
        // drops — its ownership/path wiring is not in the tuple emitter.
        let doc = serde_json::json!({
            "pkg": "geom",
            "name": "geom",
            "version": "0.1.0",
            "functions": [
                {
                    "name": "extent",
                    "params": [],
                    "results": [{"name": "", "type": "(Int, Int)", "rustType": "(u64, u32)"}],
                    "effect": "pure"
                },
                {
                    "name": "labelled_extent",
                    "params": [],
                    "results": [{"name": "", "type": "(Int, String, Bool)",
                                 "rustType": "(u64, String, bool)"}],
                    "effect": "pure"
                },
                {
                    "name": "handle_extent",
                    "params": [],
                    "results": [{"name": "", "type": "(Int, Version)",
                                 "rustType": "(u64, Version)"}],
                    "effect": "pure"
                }
            ],
            "errors": []
        });
        let iface = crate_interface(&PkgInfo::decode_json(&doc.to_string()).expect("decodes"));
        assert!(
            iface
                .bindings
                .iter()
                .any(|b| b.ref_name == "extent" && b.sig == "() -> Result Error (Int, Int)"),
            "{:?}",
            iface.skipped
        );
        // The String/bool-carrying tuple now binds too.
        assert!(
            iface.bindings.iter().any(|b| b.ref_name == "labelled_extent"
                && b.sig == "() -> Result Error (Int, String, Bool)"),
            "{:?}",
            iface.skipped
        );
        // The opaque-carrying tuple still over-drops on the tuple gate.
        assert!(
            iface.skipped.iter().any(|s| s.ref_name == "handle_extent"
                && s.reason.contains("component scalar coercion")),
            "{:?}",
            iface.skipped
        );
    }

    #[test]
    fn rendered_module_is_annotated_forwarders() {
        let iface = crate_interface(&pkg());
        let src = &iface.source;
        assert!(
            src.starts_with(
                "module Rust.Semver exposing (Version, parse, major_field_from_version)"
            ),
            "{src}"
        );
        assert!(src.contains("\ntype Version = Version\n"), "{src}");
        assert!(
            src.contains("\nparse : String -> Result Error Version\nparse arg0 =\n    Ffi.binding \"semver_parse\" arg0\n"),
            "{src}"
        );
        assert!(
            src.contains(
                "\nmajor_field_from_version : Version -> Int\nmajor_field_from_version arg0 =\n    Ffi.binding \"semver_major_field_from_version\" arg0\n"
            ),
            "{src}"
        );
    }

    #[test]
    fn conflicting_paths_poison_the_base_name() {
        let (map, poisoned) = foreign_path_map(&pkg());
        assert_eq!(
            map.get("Version").map(String::as_str),
            Some("::semver::Version")
        );
        assert!(poisoned.is_empty());
        assert_eq!(
            path_tokens("HashMap<foo::Bar, baz::Bar>"),
            vec![
                ("Bar".to_owned(), "::foo::Bar".to_owned()),
                ("Bar".to_owned(), "::baz::Bar".to_owned()),
            ]
        );
    }

    /// One-crate package carrying a single `provide.struct` entry.
    fn struct_pkg(ctor: &str, name: &str, fields: &serde_json::Value) -> PkgInfo {
        let doc = serde_json::json!({
            "pkg": "demo", "name": "demo", "version": "0.1.0",
            "functions": [{
                "name": ctor, "effect": "pure", "isStructCtor": true,
                "structName": name, "structFields": fields, "structDerives": ["Clone"]
            }],
            "errors": []
        });
        PkgInfo::decode_json(&doc.to_string()).expect("decodes")
    }

    /// One-crate package carrying a single `provide.enum` entry.
    fn enum_pkg(ctor: &str, name: &str, variants: &serde_json::Value) -> PkgInfo {
        let doc = serde_json::json!({
            "pkg": "demo", "name": "demo", "version": "0.1.0",
            "functions": [{
                "name": ctor, "effect": "pure", "isEnumDef": true,
                "enumName": name, "enumVariants": variants, "enumDerives": ["Clone"]
            }],
            "errors": []
        });
        PkgInfo::decode_json(&doc.to_string()).expect("decodes")
    }

    #[test]
    fn provide_struct_admits_a_forwarder_and_opaque_nominal() {
        let iface = crate_interface(&struct_pkg(
            "counter_new",
            "Counter",
            &serde_json::json!([{ "name": "value", "type": "i64" }]),
        ));
        assert!(
            iface.provide_types.contains("Counter"),
            "{:?}",
            iface.skipped
        );
        let b = iface
            .bindings
            .iter()
            .find(|b| b.ref_name == "counter_new")
            .expect("counter_new admitted");
        // Arity + signature come from the def's fields, not the empty fn params.
        assert_eq!(b.arity, 1);
        assert_eq!(b.sig, "Int -> Counter");
        assert_eq!(b.wrapper_ident, "demo_counter_new");
        // The forwarder + the opaque nominal both render into the module.
        assert!(
            iface.source.contains("\ntype Counter = Counter\n"),
            "{}",
            iface.source
        );
        assert!(
            iface.source.contains(
                "\ncounter_new : Int -> Counter\ncounter_new arg0 =\n    Ffi.binding \"demo_counter_new\" arg0\n"
            ),
            "{}",
            iface.source
        );
    }

    #[test]
    fn provide_struct_fieldless_is_a_nullary_forwarder() {
        // A zero-field struct's constructor is nullary — the emitted `pub fn` is
        // zero-param, so the forwarder must bind zero args (no spurious arg0).
        let iface = crate_interface(&struct_pkg("unit_new", "Unit", &serde_json::json!([])));
        let b = iface
            .bindings
            .iter()
            .find(|b| b.ref_name == "unit_new")
            .expect("unit_new admitted");
        assert_eq!(b.arity, 0);
        assert_eq!(b.sig, "() -> Unit");
        assert!(
            iface.source.contains(
                "\nunit_new : () -> Unit\nunit_new =\n    Ffi.binding \"demo_unit_new\"\n"
            ),
            "{}",
            iface.source
        );
    }

    #[test]
    fn provide_enum_admits_one_forwarder_per_variant_and_one_nominal() {
        let iface = crate_interface(&enum_pkg(
            "message_new",
            "Message",
            &serde_json::json!([
                { "name": "Increment", "payload": [] },
                { "name": "SetValue", "payload": ["i64"] }
            ]),
        ));
        // The enum nominal registers exactly once, not per-variant.
        assert_eq!(
            iface
                .provide_types
                .iter()
                .filter(|n| *n == "Message")
                .count(),
            1
        );
        let inc = iface
            .bindings
            .iter()
            .find(|b| b.ref_name == "message_new_increment")
            .expect("unit-variant forwarder");
        assert_eq!(inc.arity, 0);
        assert_eq!(inc.sig, "() -> Message");
        assert_eq!(inc.wrapper_ident, "demo_message_new_increment");
        let setv = iface
            .bindings
            .iter()
            .find(|b| b.ref_name == "message_new_set_value")
            .expect("payload-variant forwarder");
        assert_eq!(setv.arity, 1);
        assert_eq!(setv.sig, "Int -> Message");
        // The unit variant binds zero args; the payload variant binds one.
        assert!(
            iface.source.contains(
                "\nmessage_new_increment : () -> Message\nmessage_new_increment =\n    Ffi.binding \"demo_message_new_increment\"\n"
            ),
            "{}",
            iface.source
        );
        assert!(
            iface.source.contains(
                "\nmessage_new_set_value : Int -> Message\nmessage_new_set_value arg0 =\n    Ffi.binding \"demo_message_new_set_value\" arg0\n"
            ),
            "{}",
            iface.source
        );
    }

    #[test]
    fn provide_type_shadowing_a_builtin_is_refused_whole() {
        // A provide type named `Result` (a reserved builtin) refuses the WHOLE
        // entry — no forwarder, no nominal — an admitted shadow is a silent
        // wrong-type SEAL breach.
        let iface = crate_interface(&enum_pkg(
            "result_new",
            "Result",
            &serde_json::json!([{ "name": "Ok", "payload": [] }]),
        ));
        assert!(iface.provide_types.is_empty());
        assert!(iface.bindings.iter().all(|b| b.ref_name != "result_new_ok"));
        assert!(
            iface
                .skipped
                .iter()
                .any(|s| s.reason.contains("shadows an Ipê reserved builtin type")),
            "{:?}",
            iface.skipped
        );
    }
}
