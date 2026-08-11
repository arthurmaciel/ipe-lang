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


class WrapPubSubTopic(unittest.TestCase):
    def test_raw_string_topic_is_wrapped_in_typed_handle(self) -> None:
        """`PubSub.publish "t" x` becomes `PubSub.publish (PubSub.topic "t") x`,
        matching the typed `publish : Topic a -> a -> Task Error Int` surface."""
        src = (
            "import Ipe.PubSub as PubSub\n"
            'p todo =\n    PubSub.publish "todos.created" (encodeTodo todo)\n'
        )
        out = _mod.wrap_pubsub_topic(src)
        self.assertIn(
            'PubSub.publish (PubSub.topic "todos.created") (encodeTodo todo)', out
        )

    def test_publish_no_echo_is_wrapped(self) -> None:
        src = 'import Ipe.PubSub as PubSub\np = PubSub.publishNoEcho "room.1" payload\n'
        out = _mod.wrap_pubsub_topic(src)
        self.assertIn('PubSub.publishNoEcho (PubSub.topic "room.1") payload', out)

    def test_is_idempotent_and_leaves_handle_form_untouched(self) -> None:
        """A call already passing a `PubSub.topic …` handle is not a bare string
        literal, so a second run does not double-wrap it."""
        src = (
            "import Ipe.PubSub as PubSub\n"
            'p = PubSub.publish (PubSub.topic "t") payload\n'
        )
        self.assertEqual(_mod.wrap_pubsub_topic(src), src)
        once = _mod.wrap_pubsub_topic(
            'import Ipe.PubSub as PubSub\np = PubSub.publish "t" x\n'
        )
        self.assertEqual(_mod.wrap_pubsub_topic(once), once)

    def test_no_pubsub_import_is_a_noop(self) -> None:
        src = 'module M exposing (x)\nx = publish "t" y\n'
        self.assertEqual(_mod.wrap_pubsub_topic(src), src)

    def test_comment_and_string_mention_survive_verbatim(self) -> None:
        """A `-- PubSub.publish "t"` comment (no live call) and a string literal
        naming the call must not be rewritten — only a real code call is wrapped."""
        src = (
            "import Ipe.PubSub as PubSub\n"
            '-- fires PubSub.publish "todos.created" on insert\n'
            'p = PubSub.publish "todos.created" x\n'
        )
        out = _mod.wrap_pubsub_topic(src)
        self.assertIn('-- fires PubSub.publish "todos.created" on insert', out)
        self.assertIn('PubSub.publish (PubSub.topic "todos.created") x', out)
        # The comment line is untouched: exactly one wrapped call in the output.
        self.assertEqual(out.count("PubSub.topic"), 1)

    def test_other_alias_not_split(self) -> None:
        """A different alias ending in the PubSub alias text is not mis-matched."""
        src = 'import Ipe.PubSub as PubSub\np = MyPubSub.publish "t" x\n'
        out = _mod.wrap_pubsub_topic(src)
        self.assertNotIn("PubSub.topic", out)


class ReturnEntryTask(unittest.TestCase):
    def test_entry_task_run_is_stripped_and_task_returned(self) -> None:
        """The `main = let run = e … _ = Task.run run in ()` entry idiom becomes
        `main = e`, so the runtime is the single Task.run site (IPE-N0036)."""
        src = (
            "main =\n"
            "    let\n"
            "        run = entry () |> Task.onError reportError\n"
            "        _ = Task.run run\n"
            "    in\n"
            "        ()\n"
        )
        out = _mod.return_entry_task(src)
        self.assertIn(
            "main : Task Error ()\nmain =\n    entry () |> Task.onError reportError",
            out,
        )
        self.assertNotIn("Task.run", out)
        self.assertNotIn("let", out)

    def test_task_run_in_expression_position_is_untouched(self) -> None:
        """A synchronous-bridge `Task.run` inside a helper expression (still valid
        in Ipê) must survive — only the entry idiom is stripped."""
        src = (
            'x =\n    Result.withDefault "" (Task.run (System.getenv "X"))\n'
            "y =\n    case Task.run (Db.open a b) of\n        Ok c -> c\n"
        )
        self.assertEqual(_mod.return_entry_task(src), src)

    def test_no_entry_idiom_is_a_noop(self) -> None:
        src = "main =\n    entry () |> Task.onError reportError\n"
        self.assertEqual(_mod.return_entry_task(src), src)


class InjectMaybeImport(unittest.TestCase):
    def test_missing_maybe_import_is_injected(self) -> None:
        """A file that reaches `Maybe.withDefault` (via the dropped Prelude) but
        imports no `Ipe.Maybe` gets the import injected (IPE-N0034)."""
        src = (
            "module Main exposing (main)\n"
            "import Ipe.String as String\n"
            "c =\n    Maybe.withDefault 8000 (String.toInt x)\n"
        )
        out = _mod.inject_maybe_import(src)
        self.assertIn("import Ipe.Maybe as Maybe", out)
        # Injected into the import block, once.
        self.assertEqual(out.count("import Ipe.Maybe as Maybe"), 1)

    def test_existing_maybe_import_is_not_duplicated(self) -> None:
        src = (
            "import Ipe.Maybe as Maybe\n"
            "c =\n    Maybe.withDefault 0 x\n"
        )
        out = _mod.inject_maybe_import(src)
        self.assertEqual(out.count("import Ipe.Maybe"), 1)

    def test_no_maybe_use_is_a_noop(self) -> None:
        src = "import Ipe.String as String\nx = String.length y\n"
        self.assertEqual(_mod.inject_maybe_import(src), src)

    def test_maybe_only_in_comment_or_string_is_a_noop(self) -> None:
        """`Maybe.` appearing only in a comment or string is not a real qualified
        use, so no import is injected."""
        src = (
            "import Ipe.String as String\n"
            "-- fall back via Maybe.withDefault when absent\n"
            'msg = "use Maybe.withDefault here"\n'
            "x = String.length y\n"
        )
        self.assertEqual(_mod.inject_maybe_import(src), src)


if __name__ == "__main__":
    unittest.main()
