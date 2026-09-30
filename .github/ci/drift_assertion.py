#!/usr/bin/env python3
"""The one parser of a drift assertion: a shell command that fails when
regenerated output no longer matches what is committed.

Two spellings exist, and `parse` reads both into a `DriftAssertion`:
  * `SCRIPT PATH...` — the sanctioned assertion; it sees tracked changes and
    files git does not track yet (`sees_untracked`);
  * `git [global options] diff|diff-index|diff-files ... --exit-code|--quiet`
    — blind to an untracked file the generator wrote.
Its `paths` are the operands the command reads: every word after `--`, or
without one every non-option word after the subcommand (a revision among them
is harmless to the callers, which only ask what a path overlaps).

Callers: `change_class.guard` (no drift assertion may read a PROSE path) and
`verify-manifest` check 17 (no CI drift assertion may be blind to untracked
output). A command with no path operand reads the whole tree; check 17
refuses every blind one wherever CI runs it, so the PROSE guard reads only the
named operands.
"""

from __future__ import annotations

import posixpath
from collections.abc import Sequence
from dataclasses import dataclass

import shell_lex

SCRIPT = "tools/scripts/generated-unchanged.sh"
_SCRIPT_NAME = posixpath.basename(SCRIPT)
# The git subcommands that compare the working tree or index and can exit
# non-zero on a difference.
GIT_DIFFS = frozenset({"diff", "diff-index", "diff-files"})
# The flags that make one of them an assertion rather than a report.
GIT_ASSERT_FLAGS = frozenset({"--exit-code", "--quiet"})
# git's global options that take the next word as their value.
_GIT_VALUED_GLOBALS = frozenset({"-C", "-c", "--git-dir", "--work-tree", "--namespace", "--config-env"})


@dataclass(frozen=True)
class DriftAssertion:
    """One drift assertion: the operands it reads, and whether it fails on a
    file git does not track yet."""

    paths: tuple[str, ...]
    sees_untracked: bool
    command: str


def _operands(words: Sequence[str]) -> tuple[str, ...]:
    if "--" in words:
        return tuple(words[list(words).index("--") + 1 :])
    return tuple(w for w in words if not w.startswith("-"))


def _git_diff(words: Sequence[str]) -> tuple[str, ...] | None:
    """The operands of `words` (starting after `git`) when they are a
    `git diff` assertion, else None."""
    j = 0
    while j < len(words) and words[j].startswith("-"):
        j += 2 if words[j] in _GIT_VALUED_GLOBALS else 1
    if j >= len(words) or words[j] not in GIT_DIFFS:
        return None
    rest = words[j + 1 :]
    if not GIT_ASSERT_FLAGS.intersection(rest):
        return None
    return _operands(rest)


def parse(words: Sequence[str]) -> list[DriftAssertion]:
    """Every drift assertion in one command's words. Each anchor (`SCRIPT` or
    `git`, by basename) reads the words up to the next anchor, so a wrapper
    such as `sh -c '...'` whose words were split again is read too; the
    quotes such an inner layer leaves on a word are removed."""
    words = [w.strip("'\"") for w in words]
    anchors = [
        k for k, w in enumerate(words) if posixpath.basename(w) in (_SCRIPT_NAME, "git")
    ] + [len(words)]
    out: list[DriftAssertion] = []
    for k, end in zip(anchors, anchors[1:]):
        body = words[k + 1 : end]
        command = " ".join(words[k:end])
        if posixpath.basename(words[k]) == _SCRIPT_NAME:
            out.append(DriftAssertion(tuple(body), True, command))
        elif (paths := _git_diff(body)) is not None:
            out.append(DriftAssertion(paths, False, command))
    return out


def in_shell(text: str) -> list[DriftAssertion]:
    """Every drift assertion in shell `text`: each command's quote-removed
    words, joined and split again on blanks so a quoted `sh -c` body is read
    as words."""
    return [a for cmd in shell_lex.split_commands(text) for a in parse(" ".join(cmd.words).split())]
