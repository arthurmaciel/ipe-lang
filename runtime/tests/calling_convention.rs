//! Integration test: assert that no unintended curried helpers are added
//! to the runtime. Under the tupled calling convention a multi-arg Sky function
//! lowers to ONE tupled Rust fn `f(a, b)`, never a curried `f(a)(b)`.
//!
//! As of the JSON-pipeline uncurry refactor (commit 710f35f5), the runtime has
//! NO curried helpers — even the `Json.Decode.Pipeline` builders
//! (`decode_pipeline_required` / `_optional`) are now tupled: they take their
//! `next_decoder` as a `Box<dyn FnOnce(T) -> F>` argument and return a plain
//! `Decoder<E, F>`, not an `impl Fn…`. So the expected set is empty.
//!
//! The one shape that still returns `impl Fn…` is a Handler producer (the
//! `middleware_with_*` family): it takes all its Sky args tupled and returns a
//! **Handler**, the runtime type `Fn(ServerRequest) -> SkyTask<E, Response>`.
//! The returned closure takes a runtime `ServerRequest`, NOT a deferred Sky arg,
//! so this is not currying — it's a function that returns a function-typed
//! value. These are recognised structurally (return-Fn arg == `ServerRequest`)
//! so new middleware don't need allowlisting.

use std::path::PathBuf;

#[test]
fn no_unintended_curried_helpers() {
    let runtime_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("src")
        .join("sky_runtime");

    let mut curried: Vec<String> = Vec::new();
    // Capture the helper name AND the first parameter type of the returned
    // `Fn`, so Handler producers (returned `Fn(ServerRequest) -> …`) can be
    // distinguished from genuine curried builders.
    let re =
        regex::Regex::new(r"fn ([a-z_]+).*-> impl Fn(?:Once|Mut)?\(\s*([A-Za-z_][A-Za-z0-9_]*)")
            .unwrap();

    for entry in std::fs::read_dir(&runtime_dir).unwrap() {
        let path = entry.unwrap().path();
        if path.extension().and_then(|e| e.to_str()) != Some("rs") {
            continue;
        }
        let content = std::fs::read_to_string(&path).unwrap();
        for cap in re.captures_iter(&content) {
            let returned_arg = cap.get(2).unwrap().as_str();
            // A function returning a Handler (`Fn(ServerRequest) -> SkyTask`)
            // is a handler producer, not a curried helper — skip it.
            if returned_arg == "ServerRequest" {
                continue;
            }
            curried.push(cap.get(1).unwrap().as_str().to_string());
        }
    }

    curried.sort();
    assert_eq!(
        curried,
        Vec::<String>::new(),
        "Unexpected curried helper(s) found — the runtime uses the tupled calling \
         convention (a multi-arg Sky fn lowers to f(a, b), never f(a)(b)). Pipeline \
         decoders (decode_pipeline_*) are tupled (next_decoder passed as a Box<dyn FnOnce> \
         arg), and middleware that return a Handler (Fn(ServerRequest) -> …) are \
         recognised structurally and exempt. A new name here means a multi-arg helper \
         leaked a curried `-> impl Fn(<Sky type>)` shape — make it tupled instead."
    );
}
