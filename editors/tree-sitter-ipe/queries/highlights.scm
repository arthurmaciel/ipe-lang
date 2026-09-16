; Syntax highlighting for Ipê.
;
; Capture names follow the standard tree-sitter highlight vocabulary (shared by
; Helix, Zed, nvim-treesitter, and Emacs treesit), covering the same categories
; the editors expect: keywords, types/constructors, functions, module names,
; strings, numbers, comments, and operators.

; ── Comments ────────────────────────────────────────────────────────────────
(line_comment) @comment
(block_comment) @comment
(doc_comment) @comment.documentation

; ── Literals ────────────────────────────────────────────────────────────────
(number_literal) @number
(string_literal) @string
(triple_string_literal) @string
(char_literal) @character
(escape_sequence) @string.escape
(interpolation) @punctuation.special

; ── Modules & imports ─────────────────────────────────────────────────────────
(module_name (upper_identifier) @module)
(import module: (module_name) @module)
(import alias: (upper_identifier) @module)

[
  "module"
  "import"
  "exposing"
  "as"
] @keyword.import

; ── Declaration keywords ──────────────────────────────────────────────────────
[
  "type"
  "alias"
  "foreign"
] @keyword.type

[
  "case"
  "of"
  "if"
  "then"
  "else"
  "let"
  "in"
  "do"
] @keyword.control

; ── Types & constructors ──────────────────────────────────────────────────────
(type_constructor (upper_identifier) @type)
(type_declaration name: (upper_identifier) @type)
(type_alias_declaration name: (upper_identifier) @type)
(record_type_field field: (lower_identifier) @variable.member)
(type_variable) @type.parameter

(constructor name: (upper_identifier) @constructor)
(constructor_reference (upper_identifier) @constructor)
(constructor_pattern_name (upper_identifier) @constructor)
(constructor_pattern_name (qualified_upper) @constructor)
(bool_pattern) @constant.builtin.boolean

; ── Functions ───────────────────────────────────────────────────────────────
(value_declaration name: (lower_identifier) @function)
(type_annotation name: (lower_identifier) @function)
(application function: (value_reference (lower_identifier) @function.call))
(application function: (value_qualified (qualified_lower) @function.call))

; ── Records & field access ────────────────────────────────────────────────────
(record_field name: (lower_identifier) @variable.member)
(field_access field: (lower_identifier) @variable.member)
(record_update base: (lower_identifier) @variable)

; ── Names ───────────────────────────────────────────────────────────────────
(value_reference (lower_identifier) @variable)
(value_qualified (qualified_lower) @variable)
(var_pattern (lower_identifier) @variable.parameter)
(wildcard_pattern) @comment.unused

; ── Operators & punctuation ─────────────────────────────────────────────────────
(operator) @operator

[
  "->"
  "<-"
  "\\"
  "|"
  "="
  ":"
  "::"
] @operator

(double_dot) @operator

[
  "("
  ")"
  "{"
  "}"
  "["
  "]"
] @punctuation.bracket

[
  ","
  "."
] @punctuation.delimiter
