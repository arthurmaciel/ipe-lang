#!/usr/bin/env python3
"""Quote-removing POSIX shell lexer for the CI verifiers' `run:` checks.

`split_commands` turns shell text into simple commands: each a list of words
with quotes and backslashes removed exactly as the shell removes them, plus
the targets of its output redirections. A check that asks "which file does
this command write" then matches on the word the shell will see
(`".github/"ci/x` is `.github/ci/x`), never on the spelling in the source.

Command separators are `;`, `&`, `|`, `(`, `)`, newline, `$(`, and a
backtick; a command substitution inside double quotes is lexed again from its
own start (`suffixes`), so a command nested there is seen as a command. A
`#` at word start is a comment. A here-document body is data to its command
and is skipped; `heredoc_commands` names the commands that were fed one.
Unterminated quotes run to the end of the text — the lexer never raises, it
only ever sees more text as one word.
"""

from __future__ import annotations

from dataclasses import dataclass, field


@dataclass
class Command:
    """One simple command: quote-removed words and output-redirect targets."""

    words: list[str] = field(default_factory=list)
    writes: list[str] = field(default_factory=list)
    heredoc: bool = False


_SEPARATORS = frozenset(";&|()\n`")


def _lex(text: str, bodies: list[tuple[int, int]] | None = None) -> list[Command]:
    cmds: list[Command] = []
    cur = Command()
    buf: list[str] | None = None
    pending: str | None = None  # "write" | "read" | "heredoc" — the next word's role
    heredocs: list[tuple[str, bool]] = []
    i, n = 0, len(text)

    def end_word() -> None:
        nonlocal buf, pending
        if buf is None:
            return
        word = "".join(buf)
        buf = None
        if pending == "write":
            cur.writes.append(word)
        elif pending == "heredoc":
            heredocs.append((word, pending_strip))
            cur.heredoc = True
        elif pending is None:
            cur.words.append(word)
        pending = None

    def end_command() -> None:
        nonlocal cur
        end_word()
        if cur.words or cur.writes:
            cmds.append(cur)
        cur = Command()

    pending_strip = False
    while i < n:
        c = text[i]
        if c == "\n":
            end_command()
            i += 1
            while heredocs:
                delim, strip = heredocs.pop(0)
                body_start = i
                while i < n:
                    j = text.find("\n", i)
                    line = text[i : n if j < 0 else j]
                    i = n if j < 0 else j + 1
                    if (line.lstrip("\t") if strip else line) == delim:
                        break
                if bodies is not None:
                    bodies.append((body_start, i))
            continue
        if c in " \t\r":
            end_word()
            i += 1
            continue
        if c == "$" and text.startswith("$(", i):
            end_command()
            i += 2
            continue
        if c in _SEPARATORS:
            end_command()
            i += 1
            continue
        if c in "<>":
            fd = buf is not None and "".join(buf).isdigit()
            if fd:
                buf = None
            else:
                end_word()
            if text.startswith("<<<", i):
                pending, i = "read", i + 3
            elif text.startswith("<<", i):
                i += 2
                pending_strip = text.startswith("-", i)
                if pending_strip:
                    i += 1
                pending = "heredoc"
            elif c == "<" and text.startswith("<>", i):
                pending, i = "write", i + 2
            elif c == "<":
                pending, i = "read", i + 1
            else:
                i += 2 if text.startswith((">>", ">|", ">&"), i) else 1
                pending = "write"
            continue
        if c == "#" and buf is None:
            j = text.find("\n", i)
            i = n if j < 0 else j
            continue
        if buf is None:
            buf = []
        if c == "\\":
            if i + 1 < n and text[i + 1] != "\n":
                buf.append(text[i + 1])
            i += 2
            continue
        if c == "'":
            j = text.find("'", i + 1)
            j = n if j < 0 else j
            buf.append(text[i + 1 : j])
            i = j + 1
            continue
        if c == '"':
            i += 1
            while i < n and text[i] != '"':
                if text[i] == "\\" and i + 1 < n and text[i + 1] in '"\\$`\n':
                    if text[i + 1] != "\n":
                        buf.append(text[i + 1])
                    i += 2
                    continue
                buf.append(text[i])
                i += 1
            i += 1
            continue
        buf.append(c)
        i += 1
    end_command()
    return cmds


def suffixes(text: str) -> list[str]:
    """`text` and its tail after every `$(` and backtick: each command
    substitution, even one inside double quotes, starts a lexed text."""
    out = [text]
    for i, c in enumerate(text):
        if c == "`":
            out.append(text[i + 1 :])
        elif c == "$" and text.startswith("$(", i):
            out.append(text[i + 2 :])
    return out


def split_commands(text: str) -> list[Command]:
    """Every simple command in `text`, command substitutions included."""
    return [cmd for part in suffixes(text) for cmd in _lex(part)]


def without_heredoc_bodies(text: str) -> str:
    """`text` with every here-document body (data, not shell) removed."""
    bodies: list[tuple[int, int]] = []
    _lex(text, bodies)
    out, at = [], 0
    for lo, hi in bodies:
        out.append(text[at:lo])
        at = hi
    out.append(text[at:])
    return "".join(out)
