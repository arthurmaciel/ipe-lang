#![forbid(unsafe_code)]
//! The ownership marker and the claim file are named only by the claim protocol and an inventory.
//!
//! `output_dir/held.rs` runs every create, write, rename, and unlink of
//! `OWNERSHIP_MARKER` and `CLAIM_FILE` under the claim lock. Outside it, every
//! non-test item of `src/` that names either, the marker's contents
//! (`MARKER_TEXT`, `MARKER_HEADER`), or a `.ipe-output` literal must be an
//! entry of [`ALLOWED`]: the constants' own definitions, the refusal messages,
//! the listing classifier, and the hand-over to the user. A new item naming
//! one is a violation until it is reviewed into the inventory, and an
//! inventory entry no item matches is stale. No `fn adopt` or
//! `fn write_marker` exists, and `fn publish_marker` is never `pub`.
//!
//! Each file is parsed into a syntax tree, so a comment is not a use, a string
//! is classified by its value, and an item under `#[test]` or a `cfg` that
//! requires `test` is exempt by its attribute, not by a text pattern. A file
//! that does not parse fails the scan.

use std::path::{Path, PathBuf};

use proc_macro2::{TokenStream, TokenTree};
use syn::visit::{self, Visit};

/// The file whose items run the claim protocol.
const PROTOCOL_FILE: &str = "output_dir/held.rs";

/// The non-test items outside [`PROTOCOL_FILE`] allowed to name a marker word, by file and item name.
const ALLOWED: &[(&str, &str)] = &[
    ("output_dir.rs", "OWNERSHIP_MARKER"),
    ("output_dir.rs", "CLAIM_FILE"),
    ("output_dir.rs", "MARKER_HEADER"),
    ("output_dir.rs", "MARKER_TEXT"),
    ("output_dir.rs", "fmt"),
    ("output_dir.rs", "tolerated_entry"),
    ("output_dir.rs", "release_to_user"),
];

/// The identifiers that name the marker, its contents, or the claim file.
const MARKER_WORDS: &[&str] = &[
    "OWNERSHIP_MARKER",
    "CLAIM_FILE",
    "MARKER_TEXT",
    "MARKER_HEADER",
];

/// The text a literal naming the marker or the claim file holds.
const MARKER_LITERAL: &str = ".ipe-output";

/// Whether `word` is one of [`MARKER_WORDS`].
fn is_marker_word(ident: &proc_macro2::Ident) -> bool {
    MARKER_WORDS.iter().any(|word| ident == *word)
}

/// Whether the `cfg` predicate `meta` holds only in a test build.
fn requires_test(meta: &syn::Meta) -> bool {
    match meta {
        syn::Meta::Path(path) => path.is_ident("test"),
        syn::Meta::List(list) => {
            let Ok(args) = list.parse_args_with(
                syn::punctuated::Punctuated::<syn::Meta, syn::Token![,]>::parse_terminated,
            ) else {
                return false;
            };
            if list.path.is_ident("all") {
                args.iter().any(requires_test)
            } else if list.path.is_ident("any") {
                !args.is_empty() && args.iter().all(requires_test)
            } else {
                false
            }
        }
        syn::Meta::NameValue(_) => false,
    }
}

/// Whether `attrs` exempt their item as test-only: `#[test]`, or a `cfg` that requires `test`.
fn is_test_only(attrs: &[syn::Attribute]) -> bool {
    attrs.iter().any(|attr| {
        if attr.path().is_ident("test") {
            return true;
        }
        attr.path().is_ident("cfg")
            && attr
                .parse_args::<syn::Meta>()
                .is_ok_and(|meta| requires_test(&meta))
    })
}

/// Whether `tokens`, a macro body, names a marker word or holds a marker literal.
fn tokens_name_marker(tokens: TokenStream) -> bool {
    tokens.into_iter().any(|tree| match tree {
        TokenTree::Ident(ident) => is_marker_word(&ident),
        TokenTree::Literal(literal) => literal.to_string().contains(MARKER_LITERAL),
        TokenTree::Group(group) => tokens_name_marker(group.stream()),
        TokenTree::Punct(_) => false,
    })
}

/// A visitor recording whether a syntax node names a marker word or literal.
#[derive(Default)]
struct Mentions(bool);

impl<'ast> Visit<'ast> for Mentions {
    fn visit_ident(&mut self, ident: &'ast proc_macro2::Ident) {
        if is_marker_word(ident) {
            self.0 = true;
        }
    }

    fn visit_lit(&mut self, lit: &'ast syn::Lit) {
        let text = match lit {
            syn::Lit::Str(s) => s.value(),
            syn::Lit::ByteStr(s) => String::from_utf8_lossy(&s.value()).into_owned(),
            syn::Lit::CStr(s) => s.value().to_string_lossy().into_owned(),
            _ => String::new(),
        };
        if text.contains(MARKER_LITERAL) {
            self.0 = true;
        }
    }

    fn visit_macro(&mut self, mac: &'ast syn::Macro) {
        if tokens_name_marker(mac.tokens.clone()) {
            self.0 = true;
        }
        visit::visit_macro(self, mac);
    }
}

/// Whether the syntax node `walk` walks names a marker word or literal.
fn names_marker(walk: impl FnOnce(&mut Mentions)) -> bool {
    let mut mentions = Mentions::default();
    walk(&mut mentions);
    mentions.0
}

/// What a scan of one file found.
#[derive(Default)]
struct Found {
    /// The non-test items that name a marker word or literal.
    naming: Vec<String>,
    /// The banned marker writers declared: `fn adopt`, `fn write_marker`, or a `pub fn publish_marker`.
    banned: Vec<String>,
}

impl Found {
    /// Record the function `name` of visibility `vis` whose node `walk` walks.
    fn function(&mut self, name: String, vis: &syn::Visibility, walk: impl FnOnce(&mut Mentions)) {
        let public = !matches!(vis, syn::Visibility::Inherited);
        match name.as_str() {
            "adopt" | "write_marker" => self.banned.push(name.clone()),
            "publish_marker" if public => self.banned.push(name.clone()),
            _ => {}
        }
        self.named(name, walk);
    }

    /// Record the item `name` when the node `walk` walks names a marker word or literal.
    fn named(&mut self, name: String, walk: impl FnOnce(&mut Mentions)) {
        if names_marker(walk) {
            self.naming.push(name);
        }
    }

    /// Scan the items of a module.
    fn items(&mut self, items: &[syn::Item]) {
        for item in items {
            self.item(item);
        }
    }

    /// Scan one non-test item: functions and constants by name, impls, traits, and modules by member.
    fn item(&mut self, item: &syn::Item) {
        if is_test_only(item_attrs(item)) {
            return;
        }
        match item {
            syn::Item::Fn(f) => {
                self.function(f.sig.ident.to_string(), &f.vis, |m| m.visit_item_fn(f));
            }
            syn::Item::Const(c) => self.named(c.ident.to_string(), |m| m.visit_item_const(c)),
            syn::Item::Static(s) => self.named(s.ident.to_string(), |m| m.visit_item_static(s)),
            syn::Item::Mod(module) => {
                if let Some((_, items)) = &module.content {
                    self.items(items);
                }
            }
            syn::Item::Impl(imp) => self.impl_items(&imp.items),
            syn::Item::Trait(tr) => self.trait_items(&tr.items),
            syn::Item::Use(_) => {}
            other => self.named("<item>".to_owned(), |m| m.visit_item(other)),
        }
    }

    /// Scan the non-test members of an `impl` block.
    fn impl_items(&mut self, items: &[syn::ImplItem]) {
        for member in items {
            match member {
                syn::ImplItem::Fn(f) if !is_test_only(&f.attrs) => {
                    self.function(f.sig.ident.to_string(), &f.vis, |m| {
                        m.visit_impl_item_fn(f);
                    });
                }
                syn::ImplItem::Const(c) if !is_test_only(&c.attrs) => {
                    self.named(c.ident.to_string(), |m| m.visit_impl_item_const(c));
                }
                syn::ImplItem::Fn(_) | syn::ImplItem::Const(_) => {}
                other => self.named("<impl item>".to_owned(), |m| m.visit_impl_item(other)),
            }
        }
    }

    /// Scan the non-test members of a trait.
    fn trait_items(&mut self, items: &[syn::TraitItem]) {
        for member in items {
            match member {
                syn::TraitItem::Fn(f) if !is_test_only(&f.attrs) => {
                    let vis = syn::Visibility::Inherited;
                    self.function(f.sig.ident.to_string(), &vis, |m| {
                        m.visit_trait_item_fn(f);
                    });
                }
                syn::TraitItem::Fn(_) => {}
                other => self.named("<trait item>".to_owned(), |m| m.visit_trait_item(other)),
            }
        }
    }
}

/// The attributes of `item`; an item kind without attributes has none.
fn item_attrs(item: &syn::Item) -> &[syn::Attribute] {
    match item {
        syn::Item::Const(i) => &i.attrs,
        syn::Item::Enum(i) => &i.attrs,
        syn::Item::ExternCrate(i) => &i.attrs,
        syn::Item::Fn(i) => &i.attrs,
        syn::Item::ForeignMod(i) => &i.attrs,
        syn::Item::Impl(i) => &i.attrs,
        syn::Item::Macro(i) => &i.attrs,
        syn::Item::Mod(i) => &i.attrs,
        syn::Item::Static(i) => &i.attrs,
        syn::Item::Struct(i) => &i.attrs,
        syn::Item::Trait(i) => &i.attrs,
        syn::Item::TraitAlias(i) => &i.attrs,
        syn::Item::Type(i) => &i.attrs,
        syn::Item::Union(i) => &i.attrs,
        syn::Item::Use(i) => &i.attrs,
        _ => &[],
    }
}

/// Scan the source `src`, or say why it cannot be parsed.
fn scan(src: &str) -> Result<Found, String> {
    let file = syn::parse_file(src).map_err(|e| e.to_string())?;
    let mut found = Found::default();
    if !is_test_only(&file.attrs) {
        found.items(&file.items);
    }
    Ok(found)
}

/// Whether the module file `parent` (relative to `root`) declares `mod tests;` as test-only.
///
/// An absent or unparsable parent declares nothing, so the file it would exempt is scanned.
fn declares_test_module(root: &Path, parent: &str) -> bool {
    let Ok(src) = std::fs::read_to_string(root.join(parent)) else {
        return false;
    };
    let Ok(file) = syn::parse_file(&src) else {
        return false;
    };
    file.items.iter().any(|item| {
        matches!(item, syn::Item::Mod(module)
            if module.ident == "tests" && module.content.is_none() && is_test_only(&module.attrs))
    })
}

/// Whether the file at `parts` lies in a `tests` module its parent declares under a test-only `cfg`.
///
/// The exemption is proved by the declaration, never by the file name alone:
/// a `tests.rs` or `tests/` whose parent compiles it in a non-test build is scanned.
fn in_test_module(root: &Path, parts: &[String]) -> bool {
    let Some(at) = parts
        .iter()
        .position(|part| part == "tests" || part == "tests.rs")
    else {
        return false;
    };
    let Some(prefix) = parts.get(..at) else {
        return false;
    };
    if prefix.is_empty() {
        return ["lib.rs", "main.rs"]
            .iter()
            .any(|parent| declares_test_module(root, parent));
    }
    let dir = prefix.join("/");
    [format!("{dir}.rs"), format!("{dir}/mod.rs")]
        .iter()
        .any(|parent| declares_test_module(root, parent))
}

/// Every `.rs` file under `root` that a non-test build compiles, relative to `root` with `/` separators.
///
/// # Errors
/// When a source directory cannot be listed.
fn production_files(root: &Path) -> std::io::Result<Vec<(String, PathBuf)>> {
    let mut found = Vec::new();
    let mut pending = vec![root.to_path_buf()];
    while let Some(dir) = pending.pop() {
        for entry in std::fs::read_dir(&dir)? {
            let path = entry?.path();
            if path.is_dir() {
                pending.push(path);
            } else if path.extension().is_some_and(|e| e == "rs")
                && let Ok(rel) = path.strip_prefix(root)
            {
                let parts: Vec<String> = rel
                    .components()
                    .map(|c| c.as_os_str().to_string_lossy().into_owned())
                    .collect();
                if !in_test_module(root, &parts) {
                    found.push((parts.join("/"), path));
                }
            }
        }
    }
    found.sort();
    Ok(found)
}

#[test]
fn marker_and_claim_file_names_stay_in_the_claim_protocol() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let files = production_files(&root).expect("list the production sources");
    assert!(
        files.iter().any(|(rel, _)| rel == PROTOCOL_FILE),
        "the protocol file `{PROTOCOL_FILE}` is scanned from `{}`",
        root.display()
    );
    let mut violations = Vec::new();
    let mut allowed_seen = Vec::new();
    for (rel, path) in &files {
        let src = std::fs::read_to_string(path).expect("read source file");
        let found = match scan(&src) {
            Ok(found) => found,
            Err(e) => {
                violations.push(format!("{rel}: does not parse: {e}"));
                continue;
            }
        };
        for writer in found.banned {
            violations.push(format!("{rel}: `fn {writer}` is banned"));
        }
        if rel == PROTOCOL_FILE {
            continue;
        }
        for name in found.naming {
            if ALLOWED.contains(&(rel.as_str(), name.as_str())) {
                allowed_seen.push((rel.clone(), name));
            } else {
                violations.push(format!(
                    "{rel}: `{name}` names the marker or claim file outside `{PROTOCOL_FILE}`"
                ));
            }
        }
    }
    assert!(
        violations.is_empty(),
        "marker names outside the claim protocol:\n{}",
        violations.join("\n")
    );
    for &(file, name) in ALLOWED {
        assert!(
            allowed_seen
                .iter()
                .any(|(rel, seen)| rel == file && seen == name),
            "the allow-listed `{file}::{name}` no longer names the marker; drop it from `ALLOWED`"
        );
    }
}

#[test]
fn a_planted_marker_name_is_caught() {
    let planted = r##"
        use std::path::Path;
        fn braces() -> &'static str { "}{" }
        fn forge(dir: &Path) {
            let _ = std::fs::write(dir.join(OWNERSHIP_MARKER), "x");
        }
        fn forge_claim(dir: &Path) {
            let _ = std::fs::remove_file(dir.join(".ipe-output.claim"));
        }
        fn char_brace(c: char) -> bool { c == '{' }
        fn raw_forge(dir: &Path) {
            let _ = dir.unlink(r#"x/.ipe-output"#);
        }
        fn bytes_forge(dir: &Path) {
            let _ = std::fs::write(dir.join("m"), MARKER_TEXT);
            let _ = b".ipe-output";
        }
        fn in_macro(dir: &Path) {
            let _ = std::fs::write(format!("{}/{}", dir.display(), CLAIM_FILE), "x");
        }
        #[cfg(not(test))]
        fn not_test(dir: &Path) {
            let _ = dir.join(OWNERSHIP_MARKER);
        }
        #[cfg(any(test, unix))]
        fn maybe_test(dir: &Path) {
            let _ = dir.join(OWNERSHIP_MARKER);
        }
        impl Held {
            fn method(&self) {
                let _ = self.unlink(OWNERSHIP_MARKER);
            }
        }
        mod inner {
            fn nested(dir: &Path) {
                let _ = dir.join(CLAIM_FILE);
            }
        }
        static ALIAS: &str = OWNERSHIP_MARKER;
    "##;
    let found = scan(planted).expect("the fixture parses");
    assert_eq!(
        found.naming,
        [
            "forge",
            "forge_claim",
            "raw_forge",
            "bytes_forge",
            "in_macro",
            "not_test",
            "maybe_test",
            "method",
            "nested",
            "ALIAS"
        ],
        "each planted name is caught"
    );
}

#[test]
fn test_code_and_comments_are_not_marker_names() {
    let clean = r#"
        fn commented(dir: &Path) {
            // std::fs::write(dir.join(OWNERSHIP_MARKER), "x");
            /* dir.unlink(CLAIM_FILE) */
            let _ = std::fs::create_dir(dir);
        }
        #[cfg(test)]
        mod tests {
            fn plant(dir: &Path) {
                let _ = std::fs::write(dir.join(OWNERSHIP_MARKER), "x");
            }
        }
        #[cfg(all(test, unix))]
        fn unix_plant(dir: &Path) {
            let _ = dir.join(CLAIM_FILE);
        }
        #[test]
        fn planted() {
            let _ = std::fs::write(Path::new(".ipe-output"), "x");
        }
        use crate::output_dir::OWNERSHIP_MARKER;
    "#;
    let found = scan(clean).expect("the fixture parses");
    assert!(found.naming.is_empty(), "got {:?}", found.naming);
    let test_file = "#![cfg(test)]\nfn plant() { let _ = OWNERSHIP_MARKER; }";
    let found = scan(test_file).expect("the fixture parses");
    assert!(
        found.naming.is_empty(),
        "a test-only file, got {:?}",
        found.naming
    );
}

#[test]
fn a_banned_marker_writer_is_caught() {
    let banned = "
        impl HeldDir {
            pub fn adopt(&self) {}
            fn write_marker(&self) {}
            pub(crate) fn publish_marker(&self) {}
        }
        impl Other {
            pub fn publish_marker(&self) {}
        }
    ";
    let found = scan(banned).expect("the fixture parses");
    assert_eq!(
        found.banned,
        ["adopt", "write_marker", "publish_marker", "publish_marker"]
    );
    let private = "impl HeldDir { fn publish_marker(&self) {} }";
    let found = scan(private).expect("the fixture parses");
    assert!(
        found.banned.is_empty(),
        "a private publish is the protocol's"
    );
}

#[test]
fn an_unparsable_file_fails_the_scan() {
    assert!(
        scan("fn broken( {").is_err(),
        "a parse failure is never a pass"
    );
}
