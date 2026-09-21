; Indent captures for Ipê.
;
; nvim-treesitter uses @indent.begin / @indent.end to decide where to add or
; remove a level of indentation when Enter is pressed or when re-indenting a
; range. Helix uses the same file with @indent / @outdent captures; both sets
; are provided so either editor works from this single file.

; ── let expressions ──────────────────────────────────────────────────────────
; Opening a let block increases indentation; the `in` keyword closes it.
(let_expr) @indent.begin
(let_expr) @indent

; ── case expressions ─────────────────────────────────────────────────────────
; Each case branch body should be indented relative to the `->`.
(case_expr) @indent.begin
(case_expr) @indent

(case_branch) @indent.begin
(case_branch) @indent

; ── do expressions ────────────────────────────────────────────────────────────
; Each do statement is a continuation; indent the block body.
(do_expr) @indent.begin
(do_expr) @indent

; ── lambda ────────────────────────────────────────────────────────────────────
(lambda) @indent.begin
(lambda) @indent

; ── if / then / else ─────────────────────────────────────────────────────────
(if_expr) @indent.begin
(if_expr) @indent

; ── record literals ───────────────────────────────────────────────────────────
(record_expr) @indent.begin
(record_expr) @indent

; ── list literals ─────────────────────────────────────────────────────────────
(list_expr) @indent.begin
(list_expr) @indent

; ── Outdent ───────────────────────────────────────────────────────────────────
; Helix `@outdent` captures: closing delimiters that step back one level.
"}" @outdent
"]" @indent.end
"}" @indent.end
