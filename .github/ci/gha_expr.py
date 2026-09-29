#!/usr/bin/env python3
"""Typed parser for GitHub Actions expressions — the one reader every CI
verifier uses to see inside `${{ ... }}`.

A text scan over expression bodies cannot know where a body ends: `}}` may sit
inside a single-quoted literal (`${{ '}}' != '' && github.env }}`), so a
regex that stops at the first `}}` hands the rest of the expression to the
shell-text scan as if it were plain text. This module tokenizes the grammar
instead — quote-aware, `''` as the escaped quote — and parses each body into a
typed tree (`Literal`, `ContextRef`, `Call`, `Unary`, `Binary`, `Member`).
Every check matches on nodes, never on body text, and anything outside the
grammar (a stray `{`, an unterminated literal or expression, a trailing token)
is a `Refusal`: no body is ever partly understood.

Grammar (the Actions evaluator's, case-insensitive for names and keywords):

    or      := and ('||' and)*
    and     := eq ('&&' eq)*
    eq      := cmp (('==' | '!=') cmp)*
    cmp     := unary (('<' | '<=' | '>' | '>=') unary)*
    unary   := '!' unary | postfix
    postfix := primary ('.' (ident | '*') | '[' (or | '*') ']')*
    primary := literal | ident '(' [or (',' or)*] ')' | ident | '(' or ')'
"""

from __future__ import annotations

import re
from dataclasses import dataclass
from typing import Union

# Bound on expression nesting (parentheses, calls, indexes, `!`); deeper is a
# refusal, never a Python recursion overflow.
EXPRESSION_DEPTH_LIMIT = 64

_IDENT_RE = re.compile(r"[A-Za-z_][A-Za-z0-9_-]*")
_NUMBER_RE = re.compile(r"-?(?:0[xX][0-9A-Fa-f]+|(?:[0-9]+(?:\.[0-9]*)?|\.[0-9]+)(?:[eE][+-]?[0-9]+)?)")
_PUNCT = ("==", "!=", "<=", ">=", "&&", "||", "<", ">", "!", "(", ")", "[", "]", ".", ",", "*")
_KEYWORDS = {"true": True, "false": False, "null": None}


@dataclass(frozen=True)
class Refusal:
    """Text outside the expression grammar: `why` names the first defect."""

    why: str


@dataclass(frozen=True)
class Literal:
    """A literal value; `is_string` marks a single-quoted string."""

    value: object
    is_string: bool


@dataclass(frozen=True)
class Prop:
    """`.name` — a literal property name."""

    name: str


@dataclass(frozen=True)
class Index:
    """`[expr]` — a computed property name."""

    expr: "Expr"


@dataclass(frozen=True)
class Star:
    """`.*` or `[*]` — every property at once."""


Segment = Union[Prop, Index, Star]


@dataclass(frozen=True)
class ContextRef:
    """A named context (`github`, `env`, `matrix`, ...) and its access path."""

    ctx: str
    path: tuple[Segment, ...]


@dataclass(frozen=True)
class Call:
    """A function call; `name` as written (the evaluator folds its case)."""

    name: str
    args: tuple["Expr", ...]


@dataclass(frozen=True)
class Member:
    """An access path on a non-context value (`fromJSON(x).y`, `(a || b).c`)."""

    base: "Expr"
    path: tuple[Segment, ...]


@dataclass(frozen=True)
class Unary:
    op: str
    operand: "Expr"


@dataclass(frozen=True)
class Binary:
    op: str
    left: "Expr"
    right: "Expr"


Expr = Union[Literal, ContextRef, Call, Member, Unary, Binary]


@dataclass(frozen=True)
class Template:
    """A string with its `${{ }}` expressions parsed: `spans[i]` is the
    `(start, end)` of `exprs[i]` in the source, delimiters included."""

    exprs: tuple[Expr, ...]
    spans: tuple[tuple[int, int], ...]


@dataclass(frozen=True)
class _Tok:
    kind: str  # "str" | "num" | "ident" | "punct"
    text: str
    value: object = None


class _ParseError(Exception):
    pass


def _tokenize(text: str, start: int, templated: bool) -> tuple[list[_Tok], int]:
    """Tokens from `text[start:]` up to the closing `}}` (`templated`) or the
    end of `text`; returns the tokens and the index just past the body."""
    toks: list[_Tok] = []
    i, n = start, len(text)
    while True:
        while i < n and text[i] in " \t\r\n":
            i += 1
        if i >= n:
            if templated:
                raise _ParseError("unterminated `${{` — no closing `}}` outside a string literal")
            return toks, i
        c = text[i]
        if templated and text.startswith("}}", i):
            return toks, i + 2
        if c == "'":
            j, buf = i + 1, []
            while True:
                if j >= n:
                    raise _ParseError("unterminated string literal")
                if text[j] == "'":
                    if text.startswith("''", j):
                        buf.append("'")
                        j += 2
                        continue
                    break
                buf.append(text[j])
                j += 1
            toks.append(_Tok("str", text[i : j + 1], "".join(buf)))
            i = j + 1
            continue
        after_value = bool(toks) and (toks[-1].kind != "punct" or toks[-1].text in (")", "]"))
        m = _NUMBER_RE.match(text, i)
        if m and (c.isdigit() or c in "-." and not after_value and len(m.group(0)) > 1):
            toks.append(_Tok("num", m.group(0), m.group(0)))
            i = m.end()
            continue
        m = _IDENT_RE.match(text, i)
        if m:
            toks.append(_Tok("ident", m.group(0)))
            i = m.end()
            continue
        for p in _PUNCT:
            if text.startswith(p, i):
                toks.append(_Tok("punct", p))
                i += len(p)
                break
        else:
            raise _ParseError(f"character {c!r} is outside the expression grammar")


class _Parser:
    def __init__(self, toks: list[_Tok]) -> None:
        self.toks = toks
        self.i = 0
        self.depth = 0

    def peek(self, text: str | None = None, kind: str = "punct") -> bool:
        if self.i >= len(self.toks):
            return False
        t = self.toks[self.i]
        return t.kind == kind and (text is None or t.text == text)

    def take(self) -> _Tok:
        if self.i >= len(self.toks):
            raise _ParseError("expression ends early")
        t = self.toks[self.i]
        self.i += 1
        return t

    def expect(self, text: str) -> None:
        if not self.peek(text):
            got = self.toks[self.i].text if self.i < len(self.toks) else "end of expression"
            raise _ParseError(f"expected {text!r}, found {got!r}")
        self.i += 1

    def nest(self) -> None:
        self.depth += 1
        if self.depth > EXPRESSION_DEPTH_LIMIT:
            raise _ParseError(f"expression nests deeper than {EXPRESSION_DEPTH_LIMIT}")

    def parse(self) -> Expr:
        if not self.toks:
            raise _ParseError("empty expression")
        e = self.or_()
        if self.i != len(self.toks):
            raise _ParseError(f"unexpected {self.toks[self.i].text!r} after a complete expression")
        return e

    def _binary(self, ops: tuple[str, ...], sub) -> Expr:
        left = sub()
        while any(self.peek(op) for op in ops):
            op = self.take().text
            left = Binary(op, left, sub())
        return left

    def or_(self) -> Expr:
        return self._binary(("||",), self.and_)

    def and_(self) -> Expr:
        return self._binary(("&&",), self.eq)

    def eq(self) -> Expr:
        return self._binary(("==", "!="), self.cmp)

    def cmp(self) -> Expr:
        return self._binary(("<=", ">=", "<", ">"), self.unary)

    def unary(self) -> Expr:
        if self.peek("!"):
            self.take()
            self.nest()
            e = Unary("!", self.unary())
            self.depth -= 1
            return e
        return self.postfix()

    def postfix(self) -> Expr:
        base, path = self.primary(), []
        while True:
            if self.peek("."):
                self.take()
                if self.peek("*"):
                    self.take()
                    path.append(Star())
                elif self.peek(kind="ident"):
                    path.append(Prop(self.take().text))
                else:
                    raise _ParseError("`.` is not followed by a property name or `*`")
            elif self.peek("["):
                self.take()
                self.nest()
                if self.peek("*"):
                    self.take()
                    path.append(Star())
                else:
                    path.append(Index(self.or_()))
                self.expect("]")
                self.depth -= 1
            else:
                break
        if not path:
            return base
        if isinstance(base, ContextRef) and not base.path:
            return ContextRef(base.ctx, tuple(path))
        return Member(base, tuple(path))

    def primary(self) -> Expr:
        t = self.take()
        if t.kind == "str":
            return Literal(t.value, True)
        if t.kind == "num":
            return Literal(t.value, False)
        if t.kind == "ident":
            if self.peek("("):
                self.take()
                self.nest()
                args: list[Expr] = []
                if not self.peek(")"):
                    args.append(self.or_())
                    while self.peek(","):
                        self.take()
                        args.append(self.or_())
                self.expect(")")
                self.depth -= 1
                return Call(t.text, tuple(args))
            if t.text.casefold() in _KEYWORDS:
                return Literal(_KEYWORDS[t.text.casefold()], False)
            return ContextRef(t.text, ())
        if t.text == "(":
            self.nest()
            e = self.or_()
            self.expect(")")
            self.depth -= 1
            return e
        raise _ParseError(f"unexpected {t.text!r}")


def parse_expression(text: str) -> Expr | Refusal:
    """One bare expression (an `if:` value written without `${{ }}`)."""
    try:
        toks, _ = _tokenize(text, 0, templated=False)
        return _Parser(toks).parse()
    except _ParseError as e:
        return Refusal(str(e))


def parse_template(text: str) -> Template | Refusal:
    """Every `${{ ... }}` expression in `text`, parsed. An expression ends at
    the first `}}` outside a string literal; any body outside the grammar
    refuses the whole text."""
    exprs: list[Expr] = []
    spans: list[tuple[int, int]] = []
    i = 0
    while True:
        start = text.find("${{", i)
        if start < 0:
            return Template(tuple(exprs), tuple(spans))
        try:
            toks, end = _tokenize(text, start + 3, templated=True)
            exprs.append(_Parser(toks).parse())
        except _ParseError as e:
            return Refusal(f"expression at offset {start}: {e}")
        spans.append((start, end))
        i = end


def parse_condition(text: str) -> Template | Refusal:
    """An `if:` value: a template when it holds `${{`, else one bare
    expression (the text outside a template's expressions is never
    evaluated)."""
    if "${{" in text:
        return parse_template(text)
    e = parse_expression(text)
    return e if isinstance(e, Refusal) else Template((e,), ((0, len(text)),))


def walk(e: Expr):
    """Every node of `e`, `e` first; index expressions included."""
    stack: list[Expr] = [e]
    while stack:
        node = stack.pop()
        yield node
        if isinstance(node, (ContextRef, Member)):
            if isinstance(node, Member):
                stack.append(node.base)
            stack.extend(s.expr for s in node.path if isinstance(s, Index))
        elif isinstance(node, Call):
            stack.extend(node.args)
        elif isinstance(node, Unary):
            stack.append(node.operand)
        elif isinstance(node, Binary):
            stack.extend((node.left, node.right))
