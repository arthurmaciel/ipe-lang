; Code navigation tags for Ipê (ctags-style symbol index).
;
; Consumed by tree-sitter's `tags` feature and tools built on it (code
; navigation, symbol search). Definitions expose the top-level declarations a
; reader jumps to: functions/values, types, aliases, and constructors.

(value_declaration
  name: (lower_identifier) @name) @definition.function

(type_declaration
  name: (upper_identifier) @name) @definition.type

(type_alias_declaration
  name: (upper_identifier) @name) @definition.type

(constructor
  name: (upper_identifier) @name) @definition.constructor

(foreign_declaration
  name: (_) @name) @definition.type

(application
  function: (value_reference (lower_identifier) @name)) @reference.call
