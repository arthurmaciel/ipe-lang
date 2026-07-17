#!/usr/bin/env python3
"""Regression tests for scripts/lib/equivalence_normalize_html.py.

Covers BACKLOG #110 sub-item 1: `equivalence_normalize_html.py` used to mask every
SVG coordinate attribute to the literal `'#'` before diffing Go-oracle output
against skyc/Rust output. That is a Rule-1 false-green hole (see
docs/architecture/go-oracle-fixture-corpus-plan.md §3.3 and
docs/architecture/class2-tier1-sweep-fix-spec-2026-07-09.md §3.1): a genuine
skyc SVG-coordinate regression (wrong scale, off-by-one, wrong precision)
would render as an empty diff under the blanket mask.

The tests below prove:
  1. the OLD blanket-mask behaviour is what created the hole (`test_old_mask_
     would_have_hidden_a_coordinate_regression` demonstrates the masked-'#'
     strategy treating two genuinely different coordinate sets as identical);
  2. the NEW `norm_svg_coord` canonicalisation still collapses harmless
     float-formatting noise (trailing zeros, negative zero, a leading-dot
     SVG-path number);
  3. the NEW canonicalisation does NOT collapse a genuine coordinate value
     regression — this is the single most important assertion in this file,
     since a fix to a false-green hole that isn't itself verified to catch
     the bug it targets is not actually fixed.

Run: python3 scripts/lib/test_equivalence_normalize_html.py
     (or: python3 -m unittest scripts.lib.test_equiv_normalize_html -v)
"""
import os
import sys
import tempfile
import unittest

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))

import equivalence_normalize_html as m  # noqa: E402


def old_mask(v):
    """The pre-fix behaviour, reimplemented standalone for comparison: every
    SVG coordinate attribute value was blanked to the literal string '#'
    regardless of its actual value."""
    return '#'


def page_with_bar(x, y, width, height):
    """A minimal Sky.Live #sky-root page carrying one SVG <rect> — the shape
    Std.Ui.Chart.bar emits (crates/skyc/stdlib/Std/Ui/Chart.sky)."""
    return (
        '<html><body><div id="sky-root">'
        '<svg width="480" height="200">'
        '<rect x="%s" y="%s" width="%s" height="%s" fill="#4080e0"></rect>'
        '</svg>'
        '</div></body></html>' % (x, y, width, height)
    )


class NormSvgCoordUnitTests(unittest.TestCase):
    """Direct tests of the new `norm_svg_coord` / `_canon_svg_num` helpers."""

    def test_bare_integer_unchanged(self):
        self.assertEqual(m.norm_svg_coord('42'), '42')

    def test_trailing_zero_noise_collapses(self):
        self.assertEqual(m.norm_svg_coord('12.000000'), m.norm_svg_coord('12'))
        self.assertEqual(m.norm_svg_coord('12.000000'), '12')

    def test_sub_tolerance_float_noise_collapses(self):
        # A hypothetical ULP-level libm divergence well inside tolerance —
        # both values round to the same 6-decimal-digit bucket.
        a = m.norm_svg_coord('100.12345601')
        b = m.norm_svg_coord('100.12345604')
        self.assertEqual(a, b)

    def test_negative_zero_collapses_to_zero(self):
        self.assertEqual(m.norm_svg_coord('-0.0000000'), '0')
        self.assertEqual(m.norm_svg_coord('-0.0000000'), m.norm_svg_coord('0'))

    def test_svg_path_grammar_preserved_around_numbers(self):
        # `d`'s mini-language: command letters + comma/space separated numbers,
        # including the leading-dot SVG number form.
        got = m.norm_svg_coord('M 10.5,20 L -3.25 .5 Z')
        self.assertEqual(got, 'M 10.5,20 L -3.25 0.5 Z')

    def test_points_list_preserved(self):
        got = m.norm_svg_coord('0,10 20.000,30 40,50.5')
        self.assertEqual(got, '0,10 20,30 40,50.5')

    def test_real_value_difference_beyond_tolerance_does_not_collapse(self):
        # This is the core Rule-1 assertion: an actual coordinate BUG (here, a
        # 1-pixel-scale error, i.e. the class of regression the old mask
        # would have hidden) must NOT normalise to the same string.
        self.assertNotEqual(m.norm_svg_coord('120'), m.norm_svg_coord('121'))
        self.assertNotEqual(m.norm_svg_coord('60.0'), m.norm_svg_coord('30.0'))  # 2x scale bug

    def test_nan_and_inf_pass_through_unmasked(self):
        # Never silently equalise unparsable/non-finite content — surfacing it
        # as a raw diff is strictly better than hiding it behind a mask.
        self.assertEqual(m.norm_svg_coord('NaN'), 'NaN')
        self.assertEqual(m.norm_svg_coord('+Inf'), '+Inf')

    def test_non_numeric_svg_attr_untouched(self):
        self.assertEqual(m.norm_svg_coord('none'), 'none')


class OldMaskWasAFalseGreenHoleTests(unittest.TestCase):
    """Demonstrates the exact hole BACKLOG #110 sub-item 1 flags: under the
    OLD masking strategy, two genuinely different bar charts collapse to an
    identical normalized string (false green). Under the NEW normalizer they
    do not (true positive)."""

    def _norm_with(self, fn, html):
        """Run the Norm HTMLParser pass with SVG_COORD routed through `fn`
        instead of the module's real norm_svg_coord — isolates exactly the
        one line of behaviour under test (scripts/lib/equivalence_normalize_html.py
        `_emit`'s `elif in_svg and k in SVG_COORD:` branch)."""
        orig = m.norm_svg_coord
        m.norm_svg_coord = fn
        try:
            p = m.Norm()
            p.feed(m.extract_sky_root(html))
            p.close()
            return ''.join(p.out)
        finally:
            m.norm_svg_coord = orig

    def test_old_mask_would_have_hidden_a_coordinate_regression(self):
        correct = page_with_bar(x=10, y=20, width=40, height=80)
        # A skyc regression that emits the WRONG bar height (e.g. the
        # Math.min/max float-truncation class of bug, or a wrong-scale bug):
        regressed = page_with_bar(x=10, y=20, width=40, height=999)

        old_correct = self._norm_with(old_mask, correct)
        old_regressed = self._norm_with(old_mask, regressed)
        # This is the bug: the blanket '#' mask makes a real 999-vs-80 height
        # difference invisible.
        self.assertEqual(
            old_correct, old_regressed,
            'sanity check: the OLD blanket mask is expected to hide this '
            'regression — if this fails, old_mask() no longer models the '
            'pre-fix behaviour this test documents')

        new_correct = self._norm_with(m.norm_svg_coord, correct)
        new_regressed = self._norm_with(m.norm_svg_coord, regressed)
        # This is the fix: the same regression now trips a real mismatch.
        self.assertNotEqual(
            new_correct, new_regressed,
            'REGRESSION: the SVG-coordinate regression that BACKLOG #110.1 '
            'exists to catch is once again being masked to an empty diff')

    def test_new_normalizer_still_ignores_harmless_formatting_noise(self):
        # Same *value*, formatted with float noise on one side (as if two
        # independent backends' float->string kernels differed cosmetically).
        a = page_with_bar(x=10, y=20, width=40, height=80)
        b = page_with_bar(x=10, y=20, width=40, height='80.000000')
        got_a = self._norm_with(m.norm_svg_coord, a)
        got_b = self._norm_with(m.norm_svg_coord, b)
        self.assertEqual(got_a, got_b)


class FullPipelineTests(unittest.TestCase):
    """End-to-end through `normalize()` (file → canonical #sky-root text),
    the same call path `scripts/lib/checks.sh`'s `_norm_body_for_equiv` uses
    in the real sweep."""

    def _write_and_normalize(self, html):
        with tempfile.NamedTemporaryFile(
                mode='w', suffix='.html', delete=False, encoding='utf-8') as f:
            f.write(html)
            path = f.name
        try:
            return m.normalize(path)
        finally:
            os.unlink(path)

    def test_identical_bars_normalize_identically(self):
        html = page_with_bar(x=10, y=20, width=40, height=80)
        self.assertEqual(
            self._write_and_normalize(html),
            self._write_and_normalize(html))

    def test_genuinely_different_bar_height_is_not_masked_away(self):
        good = page_with_bar(x=10, y=20, width=40, height=80)
        bad = page_with_bar(x=10, y=20, width=40, height=40)  # half-height bug
        self.assertNotEqual(
            self._write_and_normalize(good),
            self._write_and_normalize(bad),
            'a genuine SVG coordinate regression must survive normalisation '
            'as a real diff, not collapse to an empty one')

    def test_go_vs_rust_style_float_spelling_of_same_value_is_equivalent(self):
        # Simulates the one legitimate source of surface noise the tolerance
        # exists for: same value, two spellings.
        go_style = page_with_bar(x=10, y=20, width=40, height=80)
        rust_style = page_with_bar(x='10.0', y='20.0', width='40.0', height='80.0')
        self.assertEqual(
            self._write_and_normalize(go_style),
            self._write_and_normalize(rust_style))


if __name__ == '__main__':
    unittest.main(verbosity=2)
