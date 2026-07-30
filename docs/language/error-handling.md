# Errors: the `Ipe.Error` type

Every effect in Ipê flows through a `Task Error a` boundary, and `Error` is a
real, pattern-matchable type — not a bare string. An `Error` pairs an
`ErrorKind` (an eleven-variant classification: `Io`, `Network`, `Ffi`,
`Decode`, `Timeout`, `NotFound`, `PermissionDenied`, `InvalidInput`,
`Conflict`, `Unavailable`, `Unexpected`) with a human-readable message. You
construct errors with the classifying constructors (`Error.io`, `Error.network`,
…), inspect them with `Error.kind`, `Error.message`, and `Error.kindName`, and
render them with `Error.toString`. `Error.isRetryable` reports whether the kind
is one a caller can reasonably back off and retry (`Timeout` / `Network` /
`Unavailable`).

```ipe
module Main exposing (main)

import Ipe.Task as Task
import Ipe.Error as Error exposing (Error, ErrorKind)
import Ipe.Io as Io


diskError : Error
diskError =
    Error.io "disk full"


main : Task Error ()
main =
    let
        line =
            "kind="
                ++ Error.kindName (Error.kind diskError)
                ++ " msg="
                ++ Error.message diskError
                ++ " retry="
                ++ errorToString (Error.isRetryable diskError)
    in
    Io.println line
```

Running this prints `kind=Io msg=disk full retry=false`.

## Asserting an error in a test

`Ipe.Test.expectErr` passes when a `Result` is an `Err` classified as the
expected kind, and `Ipe.Test.kindName` renders a kind's stable label for
messages:

```ipe
module Main exposing (main, tests)

import Ipe.Error as Error exposing (Error)
import Ipe.Test as Test exposing (Test)


main =
    Test.runMain tests


tests : List Test
tests =
    [ Test.test "network failure is classified"
        (\_ -> Test.expectErr Network (fetch "down"))
    ]


-- A stand-in for a real effect: it always fails with a network error. The
-- `Result Error ()` annotation pins the success type so `expectErr`'s argument
-- is unambiguous.
fetch : String -> Result Error ()
fetch reason =
    Err (Error.network reason)
```
