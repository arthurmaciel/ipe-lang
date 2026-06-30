//! Form-data decoding for `Event::OnForm`.
//!
//! The browser posts a form submit as a `{name: value, …}` object; the event
//! handler turns it into a `FormData` (`HashMap<String, String>`). `decode_form`
//! narrows that into the typed Sky record `T` the `onSubmit` handler expects.

use crate::sky_runtime::live::html::FormData;

/// Decode browser form data into a typed Sky record `T`.
///
/// Form values are always strings; `T`'s fields are matched by name (Sky record
/// fields are camelCase, matching the HTML `name=` attributes the view emits).
/// A missing required field makes serde return an error → the caller dispatches
/// no Msg (the `OnForm` closure maps `Err` to `None`).
///
// Type-directed coercion via serde_urlencoded: a numeric/bool record field
// decodes `"42"`/`"true"` (the deserializer parses by the TARGET type), while a
// String field keeps the raw string. The old all-String serde_json path rejected
// any non-String field. We re-encode the `{name: value}` map to an x-www-form-
// urlencoded string (round-tripped through the same crate so values are escaped),
// then deserialize into `T`. A missing required field → `Err` → no Msg dispatched
// (the `OnForm` closure maps `Err` to `None`), unchanged.
pub fn decode_form<T: serde::de::DeserializeOwned>(fd: FormData) -> Result<T, String> {
    let pairs: Vec<(String, String)> = fd.into_iter().collect();
    let encoded = serde_urlencoded::to_string(&pairs).map_err(|e| e.to_string())?;
    serde_urlencoded::from_str::<T>(&encoded).map_err(|e| e.to_string())
}

/// `decode_form` + a warn on failure. The `OnForm` closure is synchronous and
/// returns `Option<M>`, so a decode failure must surface here (not via the
/// async logger) — we `eprintln!` a warn line (same plain style as the runtime
/// logger's error path) and return `None` so the live loop dispatches no Msg.
/// The codegen-emitted `onSubmit` closure calls this.
pub fn decode_form_or_warn<T: serde::de::DeserializeOwned>(fd: FormData) -> Option<T> {
    match decode_form::<T>(fd) {
        Ok(t) => Some(t),
        Err(e) => {
            // `e` is a serde error that embeds the attacker-supplied form value
            // (e.g. "unknown variant `<value>`" for an enum field). Escape it
            // before logging so embedded CR/LF/control bytes can't forge log
            // lines or inject terminal output.
            eprintln!(
                "[sky.live] form decode failed, dispatching no Msg: {}",
                crate::sky_runtime::telemetry::json_escape(&e)
            );
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // NB: form-target structs gain `#[derive(Default)] #[serde(default)]` in
    // codegen (Emitter.hs) so a missing form field decodes to the field's zero
    // value — Go json.Unmarshal parity (#37). These test structs mirror that.
    #[derive(serde::Deserialize, Default, PartialEq, Debug)]
    #[serde(default)]
    struct Creds {
        email: String,
        password: String,
    }

    #[test]
    fn decode_form_or_warn_some_and_none() {
        let mut fd = FormData::new();
        fd.insert("email".to_string(), "a@b.c".to_string());
        fd.insert("password".to_string(), "pw".to_string());
        let r: Option<Creds> = decode_form_or_warn(fd);
        assert_eq!(
            r,
            Some(Creds {
                email: "a@b.c".into(),
                password: "pw".into()
            })
        );

        // Go json.Unmarshal parity: a form with NEITHER field still decodes —
        // each field falls to its zero value — so the Msg dispatches (Some).
        let empty = FormData::new();
        let r2: Option<Creds> = decode_form_or_warn(empty);
        assert_eq!(
            r2,
            Some(Creds {
                email: String::new(),
                password: String::new()
            })
        );
    }

    // The #37 contract record: a String + an i64. Exercises the three cases the
    // Go json.Unmarshal parity demands.
    #[derive(serde::Deserialize, Default, PartialEq, Debug)]
    #[serde(default)]
    struct RunPayload {
        code: String,
        count: i64,
    }

    #[test]
    fn decode_form_missing_field_defaults_not_error() {
        // Neither field present (only an UNKNOWN key, as the playground posts
        // `ace-mirror`): decode SUCCEEDS with zero values, Msg dispatches.
        let mut unknown_only = FormData::new();
        unknown_only.insert("ace-mirror".to_string(), "1+1".to_string());
        let r: Option<RunPayload> = decode_form_or_warn(unknown_only);
        assert_eq!(
            r,
            Some(RunPayload {
                code: String::new(),
                count: 0
            })
        );

        // `code` present, `count` absent → count defaults to 0.
        let mut partial = FormData::new();
        partial.insert("code".to_string(), "hello".to_string());
        let r2: Option<RunPayload> = decode_form_or_warn(partial);
        assert_eq!(
            r2,
            Some(RunPayload {
                code: "hello".into(),
                count: 0
            })
        );
    }

    #[test]
    fn decode_form_genuine_coercion_failure_still_none() {
        // A non-numeric string into an i64 field is a GENUINE type error — it
        // must still fail (None + warn), unchanged from Go (json.Unmarshal also
        // errors on a malformed number). Only MISSING-field stopped being a
        // failure; present-but-malformed stays a failure.
        let mut bad = FormData::new();
        bad.insert("code".to_string(), "ok".to_string());
        bad.insert("count".to_string(), "abc".to_string());
        let r: Option<RunPayload> = decode_form_or_warn(bad);
        assert_eq!(r, None);
    }

    #[derive(serde::Deserialize, PartialEq, Debug)]
    struct Order {
        item: String,
        qty: i64,
        express: bool,
        price: f64,
    }

    #[test]
    fn decode_form_coerces_numeric_and_bool_fields() {
        // The old all-String serde_json path rejected these non-String fields.
        let mut fd = FormData::new();
        fd.insert("item".to_string(), "widget".to_string());
        fd.insert("qty".to_string(), "42".to_string());
        fd.insert("express".to_string(), "true".to_string());
        fd.insert("price".to_string(), "9.99".to_string());
        let r: Result<Order, String> = decode_form(fd);
        assert_eq!(
            r,
            Ok(Order {
                item: "widget".into(),
                qty: 42,
                express: true,
                price: 9.99
            })
        );
    }

    #[test]
    fn decode_form_ok_and_missing() {
        let mut fd = FormData::new();
        fd.insert("email".to_string(), "a@b.c".to_string());
        fd.insert("password".to_string(), "pw".to_string());
        let r: Result<Creds, String> = decode_form(fd);
        assert_eq!(
            r,
            Ok(Creds {
                email: "a@b.c".into(),
                password: "pw".into()
            })
        );

        // Go json.Unmarshal parity: missing `password` → "" (zero value), Ok —
        // the form-target struct's `#[serde(default)]` supplies it.
        let mut partial = FormData::new();
        partial.insert("email".to_string(), "a@b.c".to_string()); // missing password
        let r2: Result<Creds, String> = decode_form(partial);
        assert_eq!(
            r2,
            Ok(Creds {
                email: "a@b.c".into(),
                password: String::new()
            })
        );
    }

    // #37 regression: a form-target record with a `Maybe`-typed optional field
    // (`note : Maybe String` → `SkyMaybe<String>`) is idiomatic. Pre-fix the
    // codegen `#[derive(Default)]` on this struct cargo-failed E0277 because
    // `SkyMaybe<T>` had no `Default`. Part A's `impl Default for SkyMaybe`
    // (= `Nothing`) makes it qualify for the lenient stamp, so a MISSING `note`
    // decodes to `Nothing` (the exact case #37 fixes — the missing-field
    // leniency, Go's `json.Unmarshal` nil parity).
    //
    // NB on a PRESENT value: `SkyMaybe` derives serde's default (externally-
    // tagged) representation, so a bare urlencoded `note=hello` does NOT
    // round-trip into `SkyMaybe::Just("hello")` — serde expects a `Just`/
    // `Nothing` tag. Surfacing a present optional form value into `Just` is a
    // separate serde-representation concern (it would need `#[serde(untagged)]`
    // / a custom deserialize on `SkyMaybe`, which also touches the session-store
    // serialization contract) and is OUT OF SCOPE for #37, whose breach is the
    // missing-field E0277 cargo-fail. We assert the in-scope behaviour: missing
    // → Nothing (no cargo-fail, Msg dispatches), present-bare → decode declines.
    #[derive(serde::Deserialize, Default, PartialEq, Debug)]
    #[serde(default)]
    struct CredsWithNote {
        email: String,
        password: String,
        note: crate::sky_runtime::core::SkyMaybe<String>,
    }

    #[test]
    fn decode_form_maybe_field_missing_is_nothing() {
        use crate::sky_runtime::core::SkyMaybe;

        // The #37 fix: `note` absent → Nothing (Go decodes a missing nullable to
        // nil), decode SUCCEEDS, Msg dispatches. Pre-fix this struct could not
        // even be CONSTRUCTED (its `#[derive(Default)]` was an E0277 cargo-fail).
        let mut without_note = FormData::new();
        without_note.insert("email".to_string(), "a@b.c".to_string());
        without_note.insert("password".to_string(), "pw".to_string());
        let r: Option<CredsWithNote> = decode_form_or_warn(without_note);
        assert_eq!(
            r,
            Some(CredsWithNote {
                email: "a@b.c".into(),
                password: "pw".into(),
                note: SkyMaybe::Nothing,
            })
        );
    }

    #[test]
    fn sky_maybe_default_is_nothing() {
        // Part A contract: the manual `Default` impl yields `Nothing`, and it is
        // UNBOUNDED in `T` — a `SkyMaybe<NonDefault>` field still has a default.
        struct NonDefault;
        assert!(crate::sky_runtime::core::SkyMaybe::<String>::default().is_nothing());
        let _unbounded: crate::sky_runtime::core::SkyMaybe<NonDefault> = Default::default();
    }

    // #37 Part B: a form-target carrying a NON-Default field (here a `bool`-keyed
    // ADT modelled as a Rust enum) does NOT get the lenient `#[derive(Default)]`
    // stamp in codegen — it keeps the STRICT pre-#37 emission (plain
    // `serde::Deserialize`, no `Default`/`serde(default)`). That still compiles
    // and still decodes; it just lacks missing-field leniency. This struct
    // MIRRORS that strict emission (note: no `Default`, no `#[serde(default)]`).
    #[derive(serde::Deserialize, PartialEq, Debug)]
    enum Tier {
        Free,
        Pro,
    }

    #[derive(serde::Deserialize, PartialEq, Debug)]
    struct Subscription {
        email: String,
        tier: Tier,
    }

    #[test]
    fn strict_form_target_with_non_default_field_still_decodes() {
        // All fields present → Ok (strict path works).
        let mut fd = FormData::new();
        fd.insert("email".to_string(), "a@b.c".to_string());
        fd.insert("tier".to_string(), "Pro".to_string());
        let r: Result<Subscription, String> = decode_form(fd);
        assert_eq!(
            r,
            Ok(Subscription {
                email: "a@b.c".into(),
                tier: Tier::Pro
            })
        );

        // Missing the non-defaultable `tier` → strict decode ERRORS (no leniency),
        // exactly the pre-#37 behaviour the strict emission preserves. It does
        // NOT cargo-fail; it just returns Err — acceptable for the rare
        // enum/Result-field form.
        let mut partial = FormData::new();
        partial.insert("email".to_string(), "a@b.c".to_string());
        let r2: Result<Subscription, String> = decode_form(partial);
        assert!(r2.is_err());
    }

    // #42: a PRESENT optional form field (`note=hello`) must decode to
    // `Just("hello")`, not decline the whole form. The #37 fix covered the
    // MISSING case (absent field → Nothing); this covers the PRESENT case.
    //
    // Root cause: `SkyMaybe` derives serde with the default externally-tagged repr
    // (`{"Just":"hello"}` / `"Nothing"`); form data posts a bare string `"hello"`,
    // which serde_urlencoded passes to `SkyMaybe::deserialize` as a bare string
    // value — the tagged deserialiser rejects it, and `decode_form_or_warn` returns
    // None, declining the whole form. Fix: a custom `Deserialize` for `SkyMaybe<T>`
    // that first tries `Option<T>` (bare value → Some(v) → Just(v); null/absent →
    // None → Nothing) and falls back to the tagged enum repr for session-store
    // round-trips (stored `{"Just":"x"}` / `"Nothing"` still deserialise correctly).
    #[test]
    fn decode_form_maybe_field_present_is_just() {
        use crate::sky_runtime::core::SkyMaybe;

        // Present `note=hello` → Just("hello"). Pre-fix: decode declines (None).
        let mut with_note = FormData::new();
        with_note.insert("email".to_string(), "a@b.c".to_string());
        with_note.insert("password".to_string(), "pw".to_string());
        with_note.insert("note".to_string(), "hello".to_string());
        let r: Option<CredsWithNote> = decode_form_or_warn(with_note);
        assert_eq!(
            r,
            Some(CredsWithNote {
                email: "a@b.c".into(),
                password: "pw".into(),
                note: SkyMaybe::Just("hello".to_string()),
            })
        );

        // Absent `note` still → Nothing (regression from #37).
        let mut without_note = FormData::new();
        without_note.insert("email".to_string(), "a@b.c".to_string());
        without_note.insert("password".to_string(), "pw".to_string());
        let r2: Option<CredsWithNote> = decode_form_or_warn(without_note);
        assert_eq!(
            r2,
            Some(CredsWithNote {
                email: "a@b.c".into(),
                password: "pw".into(),
                note: SkyMaybe::Nothing,
            })
        );
    }

    // Session-store round-trip: serialize SkyMaybe → JSON → deserialize must
    // preserve the value. This catches any regression from the #42 Deserialize change.
    #[test]
    fn sky_maybe_session_store_round_trip() {
        use crate::sky_runtime::core::SkyMaybe;

        let just_val: SkyMaybe<String> = SkyMaybe::Just("stored".to_string());
        let nothing_val: SkyMaybe<String> = SkyMaybe::Nothing;

        // Serialize (what the session store writes).
        let just_json = serde_json::to_string(&just_val).expect("serialize Just");
        let nothing_json = serde_json::to_string(&nothing_val).expect("serialize Nothing");

        // Deserialize (what the session store reads back).
        let just_rt: SkyMaybe<String> =
            serde_json::from_str(&just_json).expect("deserialize Just round-trip");
        let nothing_rt: SkyMaybe<String> =
            serde_json::from_str(&nothing_json).expect("deserialize Nothing round-trip");

        assert_eq!(just_rt, SkyMaybe::Just("stored".to_string()));
        assert_eq!(nothing_rt, SkyMaybe::Nothing);
    }
}
