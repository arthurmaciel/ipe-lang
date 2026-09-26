Compile a program and run the resulting binary.

```
ipe run [<path>]
```

## Arguments

A source file, a project directory, or a package.ipe. Defaults to the current project.

## Options

- @--out
- @--runtime
- @--static
- @--target
- @--allocator
- @--cfree
- @--accept-risks
- `[--debugger]` — compile the in-app time-travelling debugger overlay into the run app
- `[--record]` — cli/worker apps: record the TEA session to out/session.ipelog, one plain `<msg> => <model>` line per step
- @--quiet
- @--json
- `[-- <args>...]` — forward <args> to the compiled program
