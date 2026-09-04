# http-shell — an HTTP query shell over `Cli.app`

A line-driven REPL. `Cli.app` reads one line of standard input at a
time, turns it into a command, and re-renders `view : Model -> String`.

- `get <url>` performs a real `Http.get`, then prints the response status and
  body. The request runs as a `Task`, so the input loop never blocks.
- `quit` exits (end-of-input also exits).

Anything else prints a friendly hint and leaves the state unchanged.

## Run

```
ipe run examples/shapes/terminal/http-shell
```

Then type, for example:

```
get https://example.com
```

You can also drive it non-interactively by piping commands in:

```
printf 'get https://example.com\nquit\n' | ipe run examples/shapes/terminal/http-shell
```
