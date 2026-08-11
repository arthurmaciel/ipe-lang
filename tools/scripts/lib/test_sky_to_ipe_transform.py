#!/usr/bin/env python3
"""Unit tests for the sky-to-ipe transform's semantic passes (stdlib unittest,
no third-party dependency). Run: `python3 -m unittest` from this directory, or
`python3 tools/scripts/lib/test_sky_to_ipe_transform.py`.

The transform is otherwise exercised end-to-end by `regen --check` + the examples
sweep; these focused cases pin the passes whose logic a mirror diff would not
localise clearly (currently the kernel-alias re-home).
"""
from __future__ import annotations

import importlib.util
import pathlib
import unittest

_HERE = pathlib.Path(__file__).resolve().parent
_spec = importlib.util.spec_from_file_location(
    "sky_to_ipe_transform", _HERE / "sky-to-ipe-transform.py"
)
assert _spec is not None and _spec.loader is not None
_mod = importlib.util.module_from_spec(_spec)
_spec.loader.exec_module(_mod)


class RehomeKernelAlias(unittest.TestCase):
    def test_middleware_alias_routes_through_published_qualifier(self) -> None:
        """`Ffi.kernel "Middleware_withCors"` becomes `Middleware.withCors`, the
        `Ipe.Ffi` import is replaced by the qualifier import, and no `Ffi.kernel`
        remains — the capability gate (IPE-N0042) rejects the alias form."""
        src = (
            "module Middleware exposing (withCors)\n"
            "\n"
            "import Ipe.Ffi as Ffi\n"
            "import Ipe.Http.Server exposing (Request, Response)\n"
            "\n"
            "withCors : List String -> Handler -> Handler\n"
            "withCors =\n"
            '    Ffi.kernel "Middleware_withCors"\n'
        )
        out = _mod.rehome_kernel_alias(src)
        self.assertIn("import Ipe.Http.Middleware as Middleware", out)
        self.assertIn("withCors =\n    Middleware.withCors", out)
        self.assertNotIn("Ffi.kernel", out)
        self.assertNotIn("import Ipe.Ffi as Ffi", out)

    def test_all_middleware_members_rehome(self) -> None:
        src = (
            "import Ipe.Ffi as Ffi\n"
            'a =\n    Ffi.kernel "Middleware_withLogging"\n'
            'b =\n    Ffi.kernel "Middleware_withRateLimit"\n'
            'c =\n    Ffi.kernel "Middleware_withBasicAuth"\n'
        )
        out = _mod.rehome_kernel_alias(src)
        self.assertIn("Middleware.withLogging", out)
        self.assertIn("Middleware.withRateLimit", out)
        self.assertIn("Middleware.withBasicAuth", out)
        # The qualifier is imported exactly once for the whole file.
        self.assertEqual(out.count("import Ipe.Http.Middleware as Middleware"), 1)

    def test_unmapped_alias_is_left_untouched(self) -> None:
        """An alias whose module has no published qualifier is not rewritten — the
        pass never fabricates a route to an un-vouched kernel."""
        src = 'import Ipe.Ffi as Ffi\nx =\n    Ffi.kernel "Nonesuch_frobnicate"\n'
        self.assertEqual(_mod.rehome_kernel_alias(src), src)

    def test_no_ffi_import_is_a_noop(self) -> None:
        src = "module M exposing (x)\nx = 1\n"
        self.assertEqual(_mod.rehome_kernel_alias(src), src)

    def test_ffi_import_kept_when_still_referenced(self) -> None:
        """A file that reaches `Ffi` for something other than a re-homed alias
        keeps its `Ipe.Ffi` import alongside the injected qualifier import."""
        src = (
            "import Ipe.Ffi as Ffi\n"
            'a =\n    Ffi.kernel "Middleware_withCors"\n'
            "b =\n    Ffi.somethingElse\n"
        )
        out = _mod.rehome_kernel_alias(src)
        self.assertIn("import Ipe.Http.Middleware as Middleware", out)
        self.assertIn("import Ipe.Ffi as Ffi", out)
        self.assertIn("Middleware.withCors", out)

    def test_comment_reference_is_not_rewritten(self) -> None:
        """A `-- … `Ffi.kernel` …` comment mention (no trailing kernel-name
        string) is prose, not a call, and must survive verbatim."""
        src = (
            "import Ipe.Ffi as Ffi\n"
            "-- this re-binding via `Ffi.kernel` is the workaround\n"
            'withCors =\n    Ffi.kernel "Middleware_withCors"\n'
        )
        out = _mod.rehome_kernel_alias(src)
        self.assertIn("-- this re-binding via `Ffi.kernel` is the workaround", out)
        self.assertIn("Middleware.withCors", out)


if __name__ == "__main__":
    unittest.main()
