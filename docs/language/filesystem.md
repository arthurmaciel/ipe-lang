# Filesystem: `Ipe.Path` and `Ipe.File`

A filesystem path is not a `String`. A raw string can hide a `..` that climbs
out of the directory you meant, or a NUL byte that truncates the path at the
syscall boundary — both are how "read this file" quietly becomes "read
`/etc/passwd`". So Ipê makes a path a **type** you have to earn.

## `Path` — parse, don't validate

`Ipe.Path.Path` is opaque. The only way to get one is `Path.fromString`, which
normalises the path and *rejects* the dangerous shapes up front:

```elm
fromString : String -> Result Error Path
```

- A `..` that escapes its root (the cleaned path is `..` or starts with `../`)
  is an `Err`. A rooted path such as `/a/../../b` cannot escape — it cleans to
  `/b` — so it is allowed.
- A NUL byte (`\0`) anywhere in the string is an `Err`.
- Everything else succeeds with the lexically-cleaned path (repeated `/`
  collapsed, `.`/`..` resolved, trailing `/` dropped).

Because a `Path` can only exist after that check, nothing downstream re-validates
— the type is the proof. The pure helpers all take a `Path`:

```elm
toString   : Path -> String   -- recover the cleaned string
base       : Path -> String   -- final component
dir        : Path -> String   -- everything but the final component
ext        : Path -> String   -- extension, with the dot (or "")
isAbsolute : Path -> Bool
```

## `Ipe.File` — every path is a `Path`

Every path-consuming entry point in `Ipe.File` takes a `Path`, so an
unvalidated string can never reach a filesystem syscall:

```elm
readFile  : Path -> Task Error String
writeFile : Path -> String -> Task Error ()
readDir   : Path -> Task Error (List String)
copy      : Path -> Path -> Task Error ()
-- …and append / exists / remove / mkdirAll / isDir / readFileLimit /
--    readFileBytes / rename, all Path-first.
```

`tempFile` / `tempDir` are the exception: their argument is a filename *prefix*,
not a path, so they stay `String -> Task Error String` (and return the created
absolute path as a `String`, which you re-seal with `Path.fromString` before
handing it back to `Ipe.File`).

## Bridging the seal into a `Task`

`Path.fromString` yields a `Result`; `Ipe.File` wants a `Task`. `Task.fromResult`
turns the one into the other, and `Task.andThen` runs the file operation only
when the path validated — a rejected path flows straight to the `Task`'s error
channel:

```elm
module Main exposing (main)

import Ipe.Prelude exposing (..)
import Ipe.Io as Io
import Ipe.String as String
import Ipe.Path as Path
import Ipe.File as File
import Ipe.Task as Task


-- Seal a raw string into a validated Path, then list that directory.
listDir : String -> Task Error (List String)
listDir raw =
    Task.andThen File.readDir (Task.fromResult (Path.fromString raw))


main : Task ()
main =
    Task.andThen
        (\names -> Io.println (String.join ", " names))
        (listDir ".")
```

`ipe run` prints the entries of the current directory. Change `"."` to
`"../.."` and the run ends with a typed `Error` instead — the traversal was
refused at construction, before any directory was touched.
