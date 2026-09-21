; Textobject captures for Ipê.
;
; Helix and nvim-treesitter use these for "select inside/around" motions
; (e.g. `mif` — select inside function, `maf` — select around function).
; Capture names follow the Helix / nvim-treesitter textobject convention.

; ── Function / value declaration ─────────────────────────────────────────────
; inside: the body expression only
(value_declaration
  body: (_) @function.inside)

; around: the whole declaration (name + params + body)
(value_declaration) @function.around

; ── Parameters ────────────────────────────────────────────────────────────────
; inside: pattern bound as a parameter in a top-level declaration
(value_declaration
  parameter: (_) @parameter.inside)

; inside: pattern bound as a parameter in a lambda
(lambda
  parameter: (_) @parameter.inside)

; inside: pattern bound as a name in a let binding
(let_binding
  name: (_) @parameter.inside)

; around: include the surrounding lambda or let-binding node
(lambda) @parameter.around
(let_binding) @parameter.around

; ── Comments ──────────────────────────────────────────────────────────────────
(line_comment) @comment.inside
(line_comment) @comment.around

(block_comment) @comment.inside
(block_comment) @comment.around

(doc_comment) @comment.inside
(doc_comment) @comment.around

; ── Class-level: type and type-alias declarations ─────────────────────────────
; inside: the constructor list of a custom type
(type_declaration
  (constructor_list) @class.inside)

; around: the entire type declaration
(type_declaration) @class.around

; around: the entire type alias declaration
(type_alias_declaration) @class.around

; inside: the body of a type alias (the aliased type expression)
(type_alias_declaration
  body: (_) @class.inside)

; ── Block: let and case expressions ───────────────────────────────────────────
; inside: the in-body of a let expression
(let_expr
  body: (_) @block.inside)

; around: the whole let expression
(let_expr) @block.around

; inside: the body of a single case branch
(case_branch
  body: (_) @block.inside)

; around: a case branch (pattern + arrow + body)
(case_branch) @block.around

; around: the entire case expression
(case_expr) @block.around

; ── Do expression ─────────────────────────────────────────────────────────────
(do_expr) @block.around

(do_statement) @block.inside
