# URLs

`Ipe.Url` is a *typed, validated* URL. It exists so that a URL in your program is
always a real one — parsed, scheme-checked, and safe to hand to an outbound
request — and so that building a query string cannot accidentally become an
injection.

## The mental model

Three knots.

- **`Url` is opaque — `fromString` is the only door, and it rejects.** You cannot
  construct a `Url` from a raw string except through `Url.fromString`, which parses
  the string (the *same* parser the runtime's SSRF guard uses) and returns
  `Err` for anything scheme-less, relative, or malformed. So a value of type `Url`
  is *proof* it was validated — a bad URL is not a `Url` you have to remember to
  check, it is a value that could never be built.
- **Accessors take a `Url`, not a `String`.** `scheme`, `host`, `port`, `path`,
  `query`, `fragment` all require a `Url`. There is no way to read the host of an
  unvalidated string, which is exactly why a scheme-confused or unparseable address
  can never reach an outbound request — the type stops it at construction, upstream
  of any network call.
- **`buildQuery` encodes every key and value.** `Url.buildQuery` percent-encodes
  both sides of every pair, so a metacharacter in a value (`&`, `=`, space, `#`)
  stays *inside* that value rather than forging a new parameter. Injection safety
  is the default, not a discipline you must remember.

## A worked example: validating requests and building a safe query

The example under
[`examples/shapes/script/url-safe-request`](../../examples/shapes/script/url-safe-request/src/Main.ipe)
runs a mix of good and bad candidate URLs through the single gate, then builds a
query string from attacker-controlled values.

`fromString` is the one gate: each candidate becomes a `Result Error Url`, and
downstream code only ever sees a validated `Url`, whose parts it reads through the
typed accessors:

```ipe
describe raw =
    case Url.fromString raw of

        Ok url ->
            raw
                ++ "  ->  scheme="
                ++ Url.scheme url
                ++ " host="
                ++ Maybe.withDefault "?" (Url.host url)
                ++ " port="
                ++ Maybe.withDefault "-" (Maybe.map String.fromInt (Url.port url))

        Err _ ->
            raw ++ "  ->  REJECTED"
```

The query is built from raw values, one of which is a deliberate injection
attempt. `buildQuery` encodes it so the `&admin=true` stays trapped inside the
`note` value:

```ipe
searchQuery =
    Url.buildQuery
        [ ( "q", "red shoes" )
        , ( "note", "a&admin=true" )
        ]
```

Running it (`ipe run`) parses the valid URLs (filling default ports), rejects the
relative and garbage ones, and encodes the injection safely:

```
URL validation:
  https://api.example.com:8443/v1/users  ->  scheme=https host=api.example.com port=8443
  http://example.com/search  ->  scheme=http host=example.com port=80
  /relative/path  ->  REJECTED
  not a url at all  ->  REJECTED
  ftp://files.example.com/data  ->  scheme=ftp host=files.example.com port=21
safe query: q=red+shoes&note=a%26admin%3Dtrue
```

## The why

The opaque `Url` is [parse, don't validate][principles] and [make invalid states
unrepresentable][principles] at once: parsing happens exactly once, at
construction, and the result is a type that *cannot* hold an invalid URL — so no
function downstream re-checks or forgets to check. A `Bool`-returning validator
that let the raw string flow onward would reintroduce precisely the
check-or-forget gap this design removes.

Routing every URL through the same parser the SSRF guard uses is
[deny-by-default][principles]: an address that the security boundary would reject
also fails to become a `Url`, so the two can never disagree. And `buildQuery`
encoding by construction is [correctness][principles] — the safe path is the only
path, so a query-string injection is not a mistake a caller can make.

[principles]: ../../PRINCIPLES.md

## References

- **Per-symbol reference:** `ipe doc Ipe.Url` — `fromString`, the accessors, and
  `buildQuery` with verified examples.
- **Sibling guides:** [Url routing](url-parser.md) — matching a parsed `Url` into
  typed routes with `Ipe.Url.Parser`. [Net](net.md) — host/IP classification, the
  other half of deny-by-default addressing. [Results](result.md) — what
  `fromString` returns. [Strings](string.md) — raw URL text before it is parsed.
- **Concepts:** [The parse-don't-validate idiom](../idioms/parse-dont-validate.md)
  — the discipline the opaque `Url` embodies. The
  [live/HTTP security invariants ADR](../adr/0004-live-http-web-security-invariants.md)
  — where URL validation sits in the request path.
