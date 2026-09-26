Record a cli/worker app's TEA session and replay it as plain text.

```
ipe debugger <record <Main.ipe> | replay <log>>
```

## Arguments

record: build the app with the time-travel debugger compiled in and run it, capturing each (msg, model) step to a bounded log (default: `<Main>.ipelog` beside the entry). replay: re-emit each recorded step as one plain line — off a TTY every control byte is stripped, so a pipe/file receives clean text only. A record/replay surface, not a live scrubber (which cannot be the cli default: it must fail-closed to plain streaming off-TTY).

## Options

- `[--out <log>]` — record: write the session's replay log to <log> (default: `<Main>.ipelog`)
