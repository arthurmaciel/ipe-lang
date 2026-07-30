# Strings: ordinary and triple-quoted

Ipê has two string forms. An **ordinary string** `"..."` is a single-line
literal whose escape sequences the lexer resolves eagerly (`\n`, `\t`, `\r`,
`\\`, `\"`, `\'`, `\0`); an unrecognised escape such as `\q` is kept verbatim as
backslash-plus-character. A **triple-quoted string** `"""..."""` spans multiple
lines and supports `{{expr}}` interpolation.

## Triple-quoted strings

A triple-quoted string opens and closes with three double-quotes:

```ipe
greeting =
    """
    Hello, world.
    A single " does not close the string.
    """
```

Inside the delimiters everything is **literal content, preserved verbatim** —
including newlines, leading indentation, and a lone `"` or a pair `""` (only the
exact sequence `"""` closes the string). This is what makes triple-quoted
strings the right tool for embedded HTML, SQL, or any text with its own quoting.

The lexer captures the raw body without resolving anything; the canonicaliser
then does two things to it, in this order:

1. splits the body on `{{` … `}}` interpolation markers, and
2. resolves the two backslash escapes below.

### Interpolation — `{{expr}}`

`{{expr}}` splices a value into the string. The reference between the braces is
resolved as an expression, converted to text with `Basics.toString`, and joined
to the surrounding literal segments with `++`:

```ipe
name = "Ada"
count = 3

line =
    """Hi {{name}}, you have {{count}} messages."""
-- ⇒ "Hi Ada, you have 3 messages."
```

The whole triple-quoted string desugars to a left-folded `++` chain — one
literal expression per non-interpolated segment, and `Basics.toString expr` for
each `{{expr}}`. A string with no interpolation is a single literal; an empty
body is the empty string `""`.

Only **simple** references are interpolable, kept deliberately small so a glance
tells you what a `{{...}}` can do:

| Form | Example | Resolves to |
|---|---|---|
| bare identifier | `{{name}}` | the local variable (or an in-scope import) |
| record field | `{{user.email}}` | `user.email` |
| qualified name | `{{Color.red}}` | the module member, if known |
| single application | `{{String.fromInt n}}` | `f x` |
| integer / float literal | `{{54}}` | the numeric literal |

Anything more complex (a multi-argument call, an operator expression, an unknown
qualified name) is **not** an error: it falls back to the literal text
`{{...}}`, a clear signal that only simple expressions interpolate.

### Escapes

Two backslash escapes are recognised inside a triple-quoted string, resolved by
the canonicaliser (not the lexer):

| Source | Result |
|---|---|
| `\{{` | a literal `{{` (no interpolation is started) |
| `\\` | a literal `\` |

Any other backslash sequence is passed through unchanged. An unclosed `{{` with
no matching `}}` is treated as ordinary literal content.

```ipe
doc =
    """Write \{{name}} to interpolate the variable name."""
-- ⇒ "Write {{name}} to interpolate the variable name."
```

### Indentation is preserved

The body is stored exactly as written: leading whitespace on each line is part
of the string. Ipê does **not** strip a common left margin or drop a leading
newline — a triple-quoted string is the literal text between the delimiters. If
you want a block without the source indentation, keep the content
left-anchored.
