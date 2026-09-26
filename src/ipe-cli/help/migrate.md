Convert an interim manifest to the package.ipe record form.

```
ipe migrate config
```

## Arguments

The migration to run. `config` rewrites the interim manifest (a `Package.named |>` package.ipe, or a legacy ipe.toml) as the record form.

## Options

- `[--json]` — emit the migration result as JSON ({"schema":"ipe.cli.migrate/1","action":…,"path":…})
- `[--plain]` — print a single status line flush-left, no decoration
