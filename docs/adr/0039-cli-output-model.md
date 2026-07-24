Status: Accepted
Date: 2026-07-21

# 0039. Human-first CLI output, machine forms behind a flag

## Context

A command-line tool serves two readers at once: a person at a terminal, and a
script in a pipeline. Their needs conflict. A person wants a gutter, colour, and
labelled prose; a script wants flush-left, unstyled, one record per line — and
breaks the moment a heading or a colour escape lands in the stream it parses. A
single output shape cannot serve both without shortchanging one.

Two further tensions shape the decision:

- **A program run under `ipe run` emits its own output.** If `ipe` were to
  gutter, colour, or wrap what it prints, it would corrupt the very stdout a
  user is trying to pipe. The tool's own chatter and the program's output must
  not share a channel.
- **Misuse needs to teach, not scold.** A terse one-line `usage:` names the
  mistake but not the fix; the reader then has to run `--help` anyway. The help
  page already holds exactly what a confused user needs.

## Decision

**Human-first by default; machine forms on request.**

1. Every command's default output is human-friendly: a two-space left gutter on
   prose lines, and colour emitted only to a terminal (stripped when piped,
   redirected, or under `NO_COLOR`). The gutter width and the colour rule live in
   one place (the `style` module), so every command and the shell installer apply
   them identically.

2. The commands that emit machine-consumable **data** — `capabilities`, `diff`,
   `version`, and `explain`'s code list — accept two mutually-exclusive flags:
   `--plain` (unstyled, flush-left, one record per line) and `--json` (a stable,
   documented schema). The default is the human form. `--plain --json` together
   is a usage error. The commands that do work rather than emit data (`run`,
   `build`, `init`, `watch`, `fix`, `fmt`) and `--help` take neither.

3. `ipe run` passes the compiled program's stdout through **untouched** — no
   gutter, no colour, no wrapping — and sends its own messages to stderr.

4. Any command or subcommand misuse prints that command's **full `--help` page
   to stderr** and exits non-zero, uniformly. Help requested with `--help` goes
   to stdout and exits zero; help shown because of a mistake goes to stderr and
   exits non-zero.

Alternatives considered and rejected:

- **One adaptive format that auto-detects a tty.** Auto-detection chooses colour,
  but it cannot choose *schema*: a script still needs a documented, stable shape,
  and `isatty` is the wrong axis to pick JSON on (a script may run attached to a
  pseudo-terminal). An explicit flag is the honest contract.
- **A terse `usage:` line on misuse.** Rejected: it withholds the fix the reader
  needs and forces a second `--help` invocation. The help page is the better
  error.
- **Colour/gutter constants duplicated per command.** Rejected under the
  single-source-of-truth rule: a second definition drifts. They live once in
  `style`, and the installer's mirror is asserted equal by a test.

## Consequences

Scripts get a stable, parseable contract they opt into (`--plain` / `--json`),
and pipelines that consumed the old bare `capabilities` list adopt `--plain`
byte-for-byte. Humans get a readable default without a flag. `ipe run` stays a
transparent shell around the program, so program output pipes cleanly. Every
misuse teaches with the same page the reader would have asked for.

The invariant that must continue to hold: the gutter, palette, glyphs, and any
shared phrasing have exactly one definition in `style`; a `--plain` stream stays
flush-left and unstyled; and a documented `--json` schema is stable — a field is
added, never silently renamed or removed, or downstream `jq` breaks.
