# Program

The plain shape: a `main` you drive yourself, with no TEA loop. `main` is a
`Task` (or a `do` block of them) that runs top to bottom and exits. Choose it
for scripts, one-shot CLI tools, cron jobs, and HTTP servers — anything whose
control flow is a sequence of effects rather than an `init`/`update`/`view`
cycle. This is also the fallback shape: a program that binds `main` to anything
other than an app entry point is a Program.

An HTTP / JSON API is a Program too — its `main` builds routes with
`Ipe.Http.Server` and ends in `Server.listen`.

## Entry point

`main = <task>` — no `app` kernel. Sequence effects with a `do` block or
`Task.andThen`; fan out with `parallelDo` / `Task.parallel`.

## Minimal example

```ipe
module Main exposing (main)

import Ipe.Prelude exposing (..)
import Ipe.Task as Task
import Ipe.Error as Error exposing (Error)
import Ipe.String as String
import Ipe.Io as Io


main =
    do
        version = "1.4.0"
        Io.println ("Preflight for v" ++ version)
        results <- parallelDo
            checkBuild
            checkChangelog
            checkGitClean
        Io.println (report version results)


checkBuild : Task Error String
checkBuild =
    Task.succeed "build     ok  — artifact present"


checkChangelog : Task Error String
checkChangelog =
    Task.succeed "changelog ok  — entry for this version"


checkGitClean : Task Error String
checkGitClean =
    Task.succeed "git       ok  — working tree clean"


report : String -> List String -> String
report version results =
    "v" ++ version ++ " preflight passed:\n  " ++ String.join "\n  " results
```

Run it with `ipe run examples/release-preflight`; it announces the run, fires
three checks concurrently, then prints a report and exits 0.

## Example

[`examples/release-preflight/`](../../examples/release-preflight/) — the program above.
