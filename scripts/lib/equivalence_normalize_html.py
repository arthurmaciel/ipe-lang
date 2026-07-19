#!/usr/bin/env python3
"""Canonicalise a Ipe.Live page's #ipe-root subtree for Go≡Rust equivalence.

The Go and Rust backends are committed to BEHAVIOURAL parity, not byte-identical
output: several surface forms are legitimate implementation freedoms that this
normaliser collapses so a diff shows only behaviourally-meaningful divergences:

  * ipe-id separators — Go `r.1#div.15`, Rust `r_1_div_15` encode the SAME
    structural path; collapse `#`/`.` → `_`. (machine-internal id; never user-seen)
  * attribute order — both sort for self-determinism (map/HashMap randomisation);
    the specific order is arbitrary. Sort alphabetically on both sides.
  * event wire-encoding — Go `ipe-click="Dec"` (Msg) + `_click`-suffixed hid vs
    Rust `ipe-click="click"` + `data-ipe-on`. Same behaviour; canonicalise to the
    SET of event TYPES the element handles (`data-events="click,input"`).
  * pseudo-class / media-query / animation / transition STYLE DELIVERY — Go emits
    a scoped <style> child; Rust emits `data-ipe-*-rules` attributes the client
    turns into CSS. Same visual; drop both delivery forms.
  * SVG chart coordinates — NUMERICALLY CANONICALISED, not masked. The upstream
    Go bug this used to hide (`Math.min`/`Math.max` truncating Float args to Int,
    anzellai/ipe PR #136) landed a fix in Go `v0.17.1`; the pinned oracle
    (`tools/oracle/README.md`, currently `v0.17.3`) is newer, so the Go side no
    longer produces truncated coordinates. `Ipe.String.fromFloat` in this
    repo's Rust runtime (`src/runtime/rust/src/ipe_runtime/string.rs`) is a byte-for-byte
    port of Go's `strconv.FormatFloat(f, 'g', -1, 64)`, verified against real
    oracle probes — so there is no float-*formatting* divergence between the two
    backends to paper over either. A blanket `'#'` mask on every SVG coordinate
    was therefore a Rule-1 false-green hole (BACKLOG #110 sub-item 1): a genuine
    ipe coordinate regression (wrong scale, off-by-one, wrong precision) would
    render as an empty diff. Numeric tokens inside a coordinate-bearing SVG attr
    (`d`, `x`, `y`, `points`, `viewBox`, …) are rounded to a fixed tolerance
    (`SVG_COORD_TOLERANCE_DIGITS`) and re-serialised canonically: this still
    collapses harmless sub-tolerance float noise (`12.000000` vs `12`) but a
    genuine VALUE difference beyond that tolerance still trips a MISMATCH. See
    `norm_svg_coord` below.

What SURVIVES normalisation (the meaningful surface a regression test must guard):
element structure + nesting, text content (e.g. a <textarea>'s value as content),
inline `style=` layout, user attrs (data-test-id, href, …), which events each
element handles, and now SVG coordinate VALUES (within tolerance). The textarea-
value and console-badge regressions both surface here.

Usage:  equivalence_normalize_html.py <page.html>   # prints the canonical #ipe-root form
"""
import sys
import re
from html.parser import HTMLParser

IPEID_KEYS = ('ipe-id', 'data-ipe-hid', 'data-ipe-pc', 'data-ipe-mq',
              'data-ipe-anim', 'data-ipe-tr', 'data-ipe-key')
VOID = {'area', 'base', 'br', 'col', 'embed', 'hr', 'img', 'input',
        'link', 'meta', 'param', 'source', 'track', 'wbr'}
DELIVERY_ATTRS = ('data-ipe-pc-rules', 'data-ipe-mq-q', 'data-ipe-mq-rules',
                  'data-ipe-tr-rules', 'data-ipe-tr-respect', 'data-ipe-anim-name',
                  'data-ipe-anim-rules', 'data-ipe-anim-keyframes')
GO_STYLE_SCOPE = ('data-ipe-pc', 'data-ipe-mq', 'data-ipe-anim', 'data-ipe-tr')
# NB: keys MUST be lowercase — HTMLParser lowercases every attribute name during
# parsing, so `k in SVG_COORD` always sees the lowercased form ('viewbox', not
# 'viewBox'). A camelCase entry here would be dead (never match).
SVG_COORD = {'d', 'x', 'y', 'x1', 'y1', 'x2', 'y2', 'cx', 'cy', 'r', 'rx', 'ry',
             'width', 'height', 'points', 'fill-opacity', 'stroke-width',
             'offset', 'viewbox', 'dx', 'dy'}
SVG_TAGS = ('svg', 'path', 'rect', 'circle', 'line', 'polyline', 'polygon', 'text', 'g')

# Numeric-token matcher for SVG coordinate-bearing attrs. Handles the SVG path
# mini-language's number grammar (leading-dot forms like `.5`, signs, no
# thousands separators, optional exponent) embedded inside command letters /
# commas / whitespace, e.g. `d="M 10.5,20 L -3.2e1 .5 Z"`.
_SVG_NUM_RE = re.compile(r'-?(?:\d+\.\d*|\.\d+|\d+)(?:[eE][+-]?\d+)?')

# Sub-tolerance float noise (e.g. `12.000000` vs `12`, or a theoretical ULP-
# level libm difference) collapses at this many decimal digits; a real
# coordinate bug (wrong scale, off-by-one, wrong precision) differs by many
# orders of magnitude more than 1e-6 of a pixel and still trips a MISMATCH.
SVG_COORD_TOLERANCE_DIGITS = 6


def _canon_svg_num(token, ndigits=SVG_COORD_TOLERANCE_DIGITS):
    """Round one numeric token to a fixed tolerance and re-serialise it in a
    canonical (trailing-zero-free, no-negative-zero) decimal form. Falls back
    to the raw token on anything `float()` can't parse (NaN/Inf spellings,
    stray text) — never silently equalises unparsable content."""
    try:
        f = float(token)
    except (ValueError, OverflowError):
        return token
    if f != f or f in (float('inf'), float('-inf')):  # NaN / Inf: never mask
        return token
    r = round(f, ndigits)
    if r == 0:
        r = 0.0  # collapse -0.0 -> 0.0
    s = ('%.*f' % (ndigits, r)).rstrip('0').rstrip('.')
    return s if s not in ('', '-') else '0'


def norm_svg_coord(v):
    """Canonicalise every numeric token inside an SVG coordinate attribute
    value (a lone number, or a mixed string like `d`'s path-command grammar or
    `points`'s coordinate-pair list) to SVG_COORD_TOLERANCE_DIGITS of decimal
    precision. Non-numeric characters (command letters, commas, whitespace,
    `%` on a percentage) pass through untouched. Replaces the pre-fix blanket
    `'#'` mask — see the module docstring's SVG-coordinate bullet."""
    return _SVG_NUM_RE.sub(lambda m: _canon_svg_num(m.group(0)), v)


def norm_ipeid(v):
    return v.replace('#', '_').replace('.', '_')


def esc_attr(v):
    # HTMLParser unescapes entities inside attribute values, so re-serialising the
    # raw value would corrupt the markup (an embedded `"`/`&`/`<`/`>` breaks the
    # quoting and can make two distinct inputs collide). Re-escape for a faithful,
    # unambiguous round-trip. `&` first so already-escaped output isn't double-hit.
    return (v.replace('&', '&amp;').replace('<', '&lt;')
             .replace('>', '&gt;').replace('"', '&quot;'))


def norm_style_text(t):
    return re.sub(r'ipe-id="([^"]*)"', lambda m: 'ipe-id="%s"' % norm_ipeid(m.group(1)), t)


class Norm(HTMLParser):
    def __init__(self):
        super().__init__(convert_charrefs=False)
        self.out = []
        self.in_style = False
        self.style_buf = []
        self.svg_depth = 0
        self.suppress = 0

    def handle_starttag(self, tag, attrs):
        self._emit(tag, attrs, tag in VOID)

    def handle_startendtag(self, tag, attrs):
        self._emit(tag, attrs, True)

    def _emit(self, tag, attrs, selfclose):
        # Drop a Go pseudo/mq/anim/tr <style> delivery child entirely.
        if tag == 'style' and any(k in GO_STYLE_SCOPE for k, _ in attrs):
            self.suppress += 0 if selfclose else 1
            return
        if tag == 'svg':
            self.svg_depth += 1
        norm = []
        events = set()
        in_svg = self.svg_depth > 0 or tag in SVG_TAGS
        for k, v in attrs:
            if v is None:
                v = ''
            if k in ('data-ipe-on', 'data-ipe-hid') or k in DELIVERY_ATTRS:
                continue
            if k.startswith('ipe-') and k != 'ipe-id' and k != 'ipe-key':
                events.add(k[4:])
                continue
            if k in IPEID_KEYS:
                v = norm_ipeid(v)
            elif in_svg and k in SVG_COORD:
                v = norm_svg_coord(v)  # tolerance-round, don't mask (BACKLOG #110.1)
            norm.append((k, v))
        if events:
            norm.append(('data-events', ','.join(sorted(events))))
        norm.sort(key=lambda kv: kv[0])
        s = '<' + tag
        for k, v in norm:
            s += ' %s="%s"' % (k, esc_attr(v))
        s += ' />' if selfclose else '>'
        self.out.append(s)
        if tag == 'style':
            self.in_style = True
            self.style_buf = []

    def handle_endtag(self, tag):
        if tag == 'svg' and self.svg_depth > 0:
            self.svg_depth -= 1
        if tag == 'style' and self.suppress > 0:
            self.suppress -= 1
            return
        if tag == 'style' and self.in_style:
            self.out.append(norm_style_text(''.join(self.style_buf)))
            self.in_style = False
        self.out.append('</%s>' % tag)

    def handle_data(self, d):
        if self.suppress > 0:
            return
        (self.style_buf if self.in_style else self.out).append(d)

    def handle_entityref(self, n):
        if self.suppress > 0:
            return
        (self.style_buf if self.in_style else self.out).append('&%s;' % n)

    def handle_charref(self, n):
        if self.suppress > 0:
            return
        (self.style_buf if self.in_style else self.out).append('&#%s;' % n)


class RootExtractor(HTMLParser):
    """Capture the outerHTML of the element carrying id="ipe-root" via a real
    stack-based parse. Robust against `>` inside attribute values (a greedy
    `[^>]*?` regex truncates on those) and free of the O(n^2) regex tag-scan; the
    parser advances linearly. Reconstructs faithfully (raw start tags via
    get_starttag_text, entities re-emitted) so the downstream Norm parse is
    equivalent to feeding the original subtree substring."""

    def __init__(self):
        super().__init__(convert_charrefs=False)
        self.parts = []
        self.depth = 0
        self.capturing = False
        self.done = False

    def _is_root(self, attrs):
        return not self.capturing and any(
            k == 'id' and v == 'ipe-root' for k, v in attrs)

    def handle_starttag(self, tag, attrs):
        if self.done:
            return
        if self._is_root(attrs):
            self.capturing = True
            self.depth = 0
        if self.capturing:
            self.parts.append(self.get_starttag_text())
            if tag in VOID:
                if self.depth == 0:
                    self.done = True
            else:
                self.depth += 1

    def handle_startendtag(self, tag, attrs):
        if self.done:
            return
        if self._is_root(attrs):
            self.parts.append(self.get_starttag_text())
            self.done = True
            return
        if self.capturing:
            self.parts.append(self.get_starttag_text())

    def handle_endtag(self, tag):
        if self.done or not self.capturing:
            return
        self.parts.append('</%s>' % tag)
        if tag not in VOID:
            self.depth -= 1
            if self.depth <= 0:
                self.done = True

    def handle_data(self, d):
        if self.capturing and not self.done:
            self.parts.append(d)

    def handle_entityref(self, n):
        if self.capturing and not self.done:
            self.parts.append('&%s;' % n)

    def handle_charref(self, n):
        if self.capturing and not self.done:
            self.parts.append('&#%s;' % n)


def extract_ipe_root(html):
    """Return the #ipe-root element subtree (the rendered Ipe.Ui view), or '' —
    we compare the VIEW, not the page shell (Go inlines client JS, Rust externalises
    it; the shell legitimately differs)."""
    if 'id="ipe-root"' not in html:
        return ''
    ex = RootExtractor()
    ex.feed(html)
    ex.close()
    return ''.join(ex.parts)


def normalize(path):
    html = open(path, encoding='utf-8', errors='replace').read()
    root = extract_ipe_root(html)
    p = Norm()
    p.feed(root)
    p.close()
    # Insert newlines between adjacent tags for readability, then strip blank
    # lines. Blank lines arise from whitespace text nodes that surrounded
    # suppressed Go <style> delivery children; stripping them is safe (blank
    # lines are never semantically significant in HTML) and necessary so the
    # Go output with <style>-adjacent whitespace compares equal to the Rust
    # output that never had those <style> children.
    raw = re.sub(r'>(?=<)', '>\n', ''.join(p.out))
    return '\n'.join(line for line in raw.splitlines() if line.strip())


if __name__ == '__main__':
    if len(sys.argv) < 2:
        sys.stderr.write('usage: equivalence_normalize_html.py <page.html>\n')
        sys.exit(2)
    sys.stdout.write(normalize(sys.argv[1]))
