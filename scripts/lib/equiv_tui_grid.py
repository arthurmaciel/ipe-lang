#!/usr/bin/env python3
"""Render a Sky.Tui ANSI capture to a STYLED cell grid for Go≡Rust equivalence.

A Tui frame is ANSI written to the alternate screen. The behaviourally-meaningful
surface is what the user SEES: the per-cell character AND its styling (fg/bg/bold/
italic/underline/reverse). We feed the raw capture through a terminal emulator
(pyte) into an 80×rows screen and emit, per row, the text plus a parallel
style-annotation, so a diff catches BOTH layout (grid-vs-stacked, borders, wrap)
AND styling (typography SGR, input background) regressions.

Determinism: the kitchen-sink view is static (no timestamps/random on screen), so
two runs at the same winsize render identically. Capture the INITIAL frame at a
fixed size (the harness sets 80×rows via TIOCSWINSZ).

Usage:  equiv_tui_grid.py <capture.raw> [rows]   # prints text grid + style grid
"""
import sys

try:
    import pyte
except ImportError:
    sys.stderr.write("equiv_tui_grid: pyte not installed (pip install pyte)\n")
    sys.exit(2)


def style_tag(cell):
    """Compact per-cell style signature: flags + fg/bg. 'default' bg/fg collapse
    to '-' so an unstyled cell is just '------'.

    A BLANK cell (space / empty) has no glyph, so its foreground colour AND text
    attributes (bold/italic/underline/strike) are INVISIBLE — only the background
    is observable. We collapse those to default on a blank cell so the comparison
    is by what's actually seen, not redundant style bytes one backend happens to
    set on empty cells. (`reverse` keeps fg/bg both relevant — it swaps them.)"""
    bg = cell.bg if cell.bg and cell.bg != 'default' else '-'
    ch = cell.data or ' '
    blank = ch == ' ' and not cell.reverse
    if blank:
        return '-----/-/%s' % bg
    flags = ''
    flags += 'b' if cell.bold else '-'
    flags += 'i' if cell.italics else '-'
    flags += 'u' if cell.underscore else '-'
    flags += 's' if getattr(cell, 'strikethrough', False) else '-'
    flags += 'r' if cell.reverse else '-'
    fg = cell.fg if cell.fg and cell.fg != 'default' else '-'
    return '%s/%s/%s' % (flags, fg, bg)


def render(path, rows, cols=80):
    screen = pyte.Screen(cols, rows)
    stream = pyte.ByteStream(screen)
    stream.feed(open(path, 'rb').read())
    text_rows = []
    style_rows = []
    for y in range(rows):
        line = screen.buffer[y]
        chars = []
        # Run-length the style across the row so the style grid is diff-readable.
        styles = []
        prev = None
        run_len = 0
        run_tag = None
        for x in range(cols):
            cell = line[x]
            chars.append(cell.data or ' ')
            tag = style_tag(cell)
            if tag == run_tag:
                run_len += 1
            else:
                if run_tag is not None:
                    styles.append('%s×%d' % (run_tag, run_len))
                run_tag = tag
                run_len = 1
        if run_tag is not None:
            styles.append('%s×%d' % (run_tag, run_len))
        text_rows.append(''.join(chars).rstrip())
        # only emit a style row if it carries any non-default styling
        srow = ' '.join(styles)
        style_rows.append('' if srow.replace('-', '').replace('/', '').replace('×', '').strip(
            '0123456789 ') == '' else srow)
    out = ['--- TEXT ---']
    out += text_rows
    out += ['--- STYLE ---']
    out += [s for s in style_rows]
    return '\n'.join(out)


if __name__ == '__main__':
    if len(sys.argv) < 2:
        sys.stderr.write(__doc__)
        sys.exit(2)
    rows = 50
    if len(sys.argv) > 2:
        try:
            rows = int(sys.argv[2])
        except ValueError:
            sys.stderr.write("equiv_tui_grid: rows must be an integer, got %r\n" % sys.argv[2])
            sys.exit(2)
        if rows <= 0:
            sys.stderr.write("equiv_tui_grid: rows must be positive, got %d\n" % rows)
            sys.exit(2)
    sys.stdout.write(render(sys.argv[1], rows))
