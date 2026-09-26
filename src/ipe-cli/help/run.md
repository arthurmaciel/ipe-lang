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
- `[--target <triple>]` — cross-compile to <triple>
- @--allocator
- @--allow-slow-allocator
- @--cfree
- @--accept-risks
- `[--debugger]` — compile the in-app time-travelling debugger overlay into the run app
- @--quiet
- @--json
- `[-- <args>...]` — forward <args> to the compiled program
