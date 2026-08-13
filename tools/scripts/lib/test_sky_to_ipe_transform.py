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

    def test_line_comment_with_full_call_shape_survives_verbatim(self) -> None:
        """A `-- example: Ffi.kernel "Middleware_withCors"` line comment writes the
        WHOLE call shape — head AND kernel-name string — yet is prose, not a call:
        span-pairing keeps it verbatim while the real code call below is rehomed."""
        src = (
            "import Ipe.Ffi as Ffi\n"
            '-- example: Ffi.kernel "Middleware_withCors"\n'
            'withCors =\n    Ffi.kernel "Middleware_withCors"\n'
        )
        out = _mod.rehome_kernel_alias(src)
        self.assertIn('-- example: Ffi.kernel "Middleware_withCors"', out)
        self.assertIn("withCors =\n    Middleware.withCors", out)
        # Exactly one rehome: the comment head was not rewritten.
        self.assertEqual(out.count("Middleware.withCors"), 1)

    def test_block_comment_with_full_call_shape_survives_verbatim(self) -> None:
        """The block-comment (`{- -}`) variant of the same full-call-shape mention
        is likewise left untouched while the real call is rehomed."""
        src = (
            "import Ipe.Ffi as Ffi\n"
            '{- see Ffi.kernel "Middleware_withCors" -}\n'
            'withCors =\n    Ffi.kernel "Middleware_withCors"\n'
        )
        out = _mod.rehome_kernel_alias(src)
        self.assertIn('{- see Ffi.kernel "Middleware_withCors" -}', out)
        self.assertIn("withCors =\n    Middleware.withCors", out)
        self.assertEqual(out.count("Middleware.withCors"), 1)


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


class HoistEntryEffect(unittest.TestCase):
    def test_trailing_effect_discard_becomes_body(self) -> None:
        """`main = let x = pure … _ = <effect> in ()` hoists the effect to the
        `let` body, so `main` returns the Task the runtime then runs."""
        src = (
            "main =\n"
            "    let\n"
            "        z = Zipper.singleton 42\n"
            "        _ = Io.println (String.fromInt (Zipper.current z))\n"
            "    in\n"
            "        ()\n"
        )
        out = _mod.hoist_entry_effect(src)
        self.assertEqual(
            out,
            "main =\n"
            "    let\n"
            "        z = Zipper.singleton 42\n"
            "    in\n"
            "    Io.println (String.fromInt (Zipper.current z))\n",
        )

    def test_sole_effect_discard_collapses_let(self) -> None:
        """When the discard is the only binding, the `let` collapses to
        `main = <effect>`."""
        src = (
            "main =\n"
            "    let\n"
            '        _ = Io.println "hi"\n'
            "    in\n"
            "        ()\n"
        )
        out = _mod.hoist_entry_effect(src)
        self.assertEqual(out, 'main =\n    Io.println "hi"\n')

    def test_task_run_bridge_discard_is_left_untouched(self) -> None:
        """A `_ = <alias>.run <var>` discard is the entry bridge handled by
        return_entry_task / ipe-edits, not a plain effect to hoist."""
        src = (
            "main =\n"
            "    let\n"
            "        pipeline = Db.connect () |> Task.andThen runApp\n"
            "        _ = Task.run pipeline\n"
            "    in\n"
            "        ()\n"
        )
        self.assertEqual(_mod.hoist_entry_effect(src), src)

    def test_pure_value_discard_is_left_untouched(self) -> None:
        """A dead pure-value discard (`_ = xs`, no application) is not an effect
        to hoist; it is left for the discard-dropping pass."""
        src = (
            "main =\n"
            "    let\n"
            "        _ = unusedValue\n"
            "    in\n"
            "        ()\n"
        )
        self.assertEqual(_mod.hoist_entry_effect(src), src)

    def test_non_unit_body_is_a_noop(self) -> None:
        """A `let` whose body is not `()` is a real value expression, untouched."""
        src = (
            "main =\n"
            "    let\n"
            "        _ = Io.println x\n"
            "    in\n"
            "        result\n"
        )
        self.assertEqual(_mod.hoist_entry_effect(src), src)

    def test_non_main_binding_is_a_noop(self) -> None:
        """Only the top-level `main` binding's own entry idiom is rewritten."""
        src = (
            "helper =\n"
            "    let\n"
            "        _ = Io.println x\n"
            "    in\n"
            "        ()\n"
        )
        self.assertEqual(_mod.hoist_entry_effect(src), src)


class StripIssueRefComments(unittest.TestCase):
    def test_leading_issue_ref_block_is_removed(self) -> None:
        """A leading `--` block whose first line cites an upstream issue is dropped
        in full."""
        src = (
            "-- anzellai/sky#153 — build + run must succeed with the\n"
            "-- parametric `Zipper a` defined in a sibling module.\n"
            "module Main exposing (main)\n"
        )
        self.assertEqual(
            _mod.strip_issue_ref_comments(src),
            "module Main exposing (main)\n",
        )

    def test_bare_sky_ref_is_also_recognised(self) -> None:
        src = "-- Regression for sky#42: cross-module ADT.\ntype T = T\n"
        self.assertEqual(_mod.strip_issue_ref_comments(src), "type T = T\n")

    def test_comment_without_issue_ref_is_kept(self) -> None:
        src = "-- A plain doc comment.\n-- Second line.\nmodule M exposing (x)\n"
        self.assertEqual(_mod.strip_issue_ref_comments(src), src)

    def test_ref_only_in_string_is_a_noop(self) -> None:
        """`sky#N` inside a string literal is not a comment reference."""
        src = 'msg = "see sky#7 for details"\n'
        self.assertEqual(_mod.strip_issue_ref_comments(src), src)


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


class RewriteDiscardBindings(unittest.TestCase):
    """Tests for `rewrite_discard_bindings` — `let _ = e` → `do` block."""

    def _run(self, src: str) -> str:
        return _mod.rewrite_discard_bindings(src)

    def test_multi_discard_let_becomes_do(self) -> None:
        """A `let` with multiple `_ = e` bindings is rewritten to a `do` block."""
        src = (
            "missingTool tool hint =\n"
            "    let\n"
            '        _ = Io.eprintln ""\n'
            "        _ = Io.eprintln (tool ++ \" not found\")\n"
            "        _ = Io.eprintln hint\n"
            "    in\n"
            "        Task.succeed (System.exit 1)\n"
        )
        out = self._run(src)
        self.assertIn("    do\n", out)
        self.assertNotIn("let\n", out)
        self.assertNotIn("_ =", out)
        self.assertIn('        Io.eprintln ""\n', out)
        self.assertIn("        Task.succeed (System.exit 1)\n", out)

    def test_pure_only_let_is_unchanged(self) -> None:
        """A `let` with no `_ =` bindings is left untouched."""
        src = (
            "helper x =\n"
            "    let\n"
            "        y = x + 1\n"
            "        z = y * 2\n"
            "    in\n"
            "        z\n"
        )
        self.assertEqual(self._run(src), src)

    def test_mixed_pure_and_discard(self) -> None:
        """A `let` with both named and discard bindings: named stay, discards become bare."""
        src = (
            "reportError e =\n"
            "    let\n"
            "        msg = Error.toString e\n"
            '        _ = Log.errorWith "op" [ "error", msg ]\n'
            '        _ = Io.println ("Error: " ++ msg)\n'
            "    in\n"
            "        System.exit 1\n"
        )
        out = self._run(src)
        self.assertIn("    do\n", out)
        self.assertIn("        msg = Error.toString e\n", out)
        self.assertNotIn("_ =", out)
        self.assertIn("        System.exit 1\n", out)

    def test_multiline_discard_value(self) -> None:
        """A `_ =` whose value spans continuation lines is emitted as a bare expression."""
        src = (
            "initDb _ =\n"
            "    let\n"
            "        _ =\n"
            "            runInit\n"
            '                "users"\n'
            "                (Db.exec [] [])\n"
            "    in\n"
            "        ()\n"
        )
        out = self._run(src)
        self.assertIn("    do\n", out)
        # The binding `_ =` inside the let must be gone; only the function
        # signature's `initDb _ =` may remain.
        self.assertNotIn("        _ =\n", out)
        self.assertIn("        runInit\n", out)
        self.assertIn("        ()\n", out)

    def test_nested_let_inside_continuation_is_also_rewritten(self) -> None:
        """A `let _ = e` nested inside a continuation block is rewritten by the fixed point."""
        src = (
            "f x =\n"
            "    let\n"
            "        _ =\n"
            "            case x of\n"
            "                Err e ->\n"
            "                    let\n"
            '                        _ = Io.println ("err: " ++ e)\n'
            "                    in\n"
            "                        ()\n"
            "    in\n"
            "        Task.succeed ()\n"
        )
        out = self._run(src)
        self.assertNotIn("_ =", out)

    def test_discard_inside_task_map_lambda(self) -> None:
        """A `let _ = e in ()` inside a `Task.map (\\_ -> …)` lambda is rewritten."""
        src = (
            "addTodo conn title =\n"
            "    Db.exec conn \"INSERT\" [ title ]\n"
            "        |> Task.map\n"
            "               (\\_ ->\n"
            "                   let\n"
            '                       _ = Io.println ("Added: " ++ title)\n'
            "                   in\n"
            "                       ())\n"
        )
        out = self._run(src)
        self.assertNotIn("_ =", out)
        self.assertIn("                   do\n", out)

    def test_pure_value_discard_is_dropped_not_runified(self) -> None:
        """`_ = list` (a pure value) is DROPPED, not turned into a `do` statement.

        Runifying a non-Task value type-errors at emit (a `do` statement must be
        a `Task`). The sibling effect `Io.println` still drives the conversion;
        the dead marker just disappears.
        """
        src = (
            "main =\n"
            "    let\n"
            "        list = [ 1, 2, 3 ]\n"
            "        _ = list\n"
            "    in\n"
            '        Io.println "List ready"\n'
        )
        out = self._run(src)
        self.assertNotIn("_ =", out)
        self.assertIn("        list = [ 1, 2, 3 ]\n", out)
        # The dropped discard must NOT survive as a bare `list` do-statement.
        self.assertNotIn("        list\n", out)
        self.assertIn("    do\n", out)

    def test_effect_discard_still_runified(self) -> None:
        """An application discard (`_ = Io.println …`) is still runified (has a space)."""
        src = (
            "main =\n"
            "    let\n"
            '        _ = Io.println "hi"\n'
            "    in\n"
            "        Task.succeed ()\n"
        )
        out = self._run(src)
        self.assertNotIn("_ =", out)
        self.assertIn('        Io.println "hi"\n', out)


if __name__ == "__main__":
    unittest.main()
