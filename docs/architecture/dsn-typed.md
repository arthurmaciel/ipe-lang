# The typed-`Dsn` subsystem

`Ipe.Db.Dsn` is a typed, opaque database-connection descriptor built on one
invariant: **there is exactly one parse of any DSN string.** A raw connection
string becomes a `Dsn` once, at a validating constructor; everything downstream
reads the typed fields and never re-parses. A malformed or insecure DSN is a
typed `Err` at the boundary, never a silently-accepted, re-injectable string.

This module is the **parse surface** only. Constructing a `Dsn` performs no I/O
and discloses no capability. Connecting a `Dsn` to a live database is a separate,
separately-reviewed capability (it discloses `network`) and is not part of this
surface.

## What is shipped

`Dsn` is a reserved, opaque type (un-shadowable, like `Secret` / `Url`). Its two
constructors are the seal (signatures quoted from `src/stdlib/Ipe/Db/Dsn.ipe`):

```elm
parse : String -> Result Error Dsn
build :
    { driver : Driver
    , host : String
    , port : Int
    , database : String
    , user : String
    , password : Secret
    , tls : TlsMode
    }
    -> Result Error Dsn
```

Both run the **same** fail-closed validators, so a `Dsn` value is a proof that
the descriptor passed every check. `parse` covers the paste-a-URL case; `build`
is preferred when the parts are already structured (there is no string to
mis-escape).

The supporting closed ADTs:

```elm
type Driver = Postgres | Sqlite          -- exactly the drivers the runtime links
type TlsMode = Require | Prefer | Disable  -- Require is the secure default
```

`Driver` is closed to the two drivers the runtime actually links; an unsupported
driver is unrepresentable rather than a free string. `Disable` exists for
exhaustiveness and a future explicitly-disclosed opt-in, but the parse path
**rejects** it — a `Dsn` is never a proof of a cleartext transport.

## What fails closed

`parse` / `build` return `Err` on all of:

- an unparseable string, or an unknown driver scheme;
- a missing host for a network driver (Postgres);
- an out-of-range or non-numeric port (no narrowing cast);
- an explicit `sslmode=disable` (a hard error, not an accepted downgrade);
- an unknown `sslmode` value (fail-closed, never coerced to a permissive
  default);
- a credential or duplicated security key smuggled into the query string;
- a control-character, whitespace, or oversized component (checked on the
  percent-decoded form).

When `sslmode` is omitted, the transport defaults to `Require` — the strongest
posture, not a downgrade-tolerant one.

## The password is a `Secret`

The descriptor stores its password as the reserved `Secret` type, never a plain
`String`. There is **no** password accessor; the only display path is
`redacted`, which substitutes a placeholder. A `Dsn` cannot leak its credential
into a log, an error render, a `Debug` print, or a Model — the type carries the
proof of non-leak.

## Accessors

```elm
driver   : Dsn -> Driver
host     : Dsn -> String
port     : Dsn -> Int
database : Dsn -> String
user     : Dsn -> String
tls      : Dsn -> TlsMode
redacted : Dsn -> String     -- the ONLY display path; never includes the password
```

## Minimal runnable example

```elm
module Main exposing (main)

import Ipe.Io as Io
import Ipe.Db.Dsn as Dsn exposing (Driver(..), TlsMode(..))
import Ipe.Error as Error exposing (Error)
import Ipe.Result exposing (Result(..))


main =
    case Dsn.parse "postgres://reader:s3cr3t@db.example.com:5432/appdb" of
        Ok dsn ->
            -- prints:
            --   db.example.com
            --   postgres://reader@db.example.com:5432/appdb (tls=require, password=[redacted])
            -- the password never appears.
            Io.println (Dsn.host dsn ++ "\n" ++ Dsn.redacted dsn)

        Err e ->
            Io.eprintln (Error.toString e)
```

## Trust model

A `Dsn` guarantees the descriptor is **structurally valid and TLS-secure**: a
known driver, a present host for a network driver, an in-range port, no control
characters, and a transport that is not explicitly downgraded to cleartext. It
deliberately does **not** decide whether the host is safe to *reach* — that is
the separate authority of the connect step, which owns the `network` capability
and any host-egress policy. `Dsn` is the syntactic parse boundary; connecting is
a distinct act.
