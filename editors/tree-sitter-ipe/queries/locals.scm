; Local scopes and definitions for Ipê.
;
; Drives scope-aware highlighting and rename in editors that consume `locals`
; (Helix, nvim-treesitter). A scope is opened by each construct that binds new
; names; definitions are the binders, references are ordinary variable uses.

; ── Scopes ────────────────────────────────────────────────────────────────
(value_declaration) @local.scope
(lambda) @local.scope
(let_expr) @local.scope
(case_branch) @local.scope
(do_expr) @local.scope

; ── Definitions ─────────────────────────────────────────────────────────────
(value_declaration name: (lower_identifier) @local.definition.function)
(value_declaration parameter: (var_pattern (lower_identifier) @local.definition.parameter))
(lambda parameter: (var_pattern (lower_identifier) @local.definition.parameter))
(let_binding name: (var_pattern (lower_identifier) @local.definition.var))
(var_pattern (lower_identifier) @local.definition.var)
(record_pattern (lower_identifier) @local.definition.var)

; ── References ────────────────────────────────────────────────────────────────
(value_reference (lower_identifier) @local.reference)
(field_access field: (lower_identifier) @local.reference)
