//! The rule registry — the single source of truth for every shipped rule's
//! name, one-line summary, default severity, and whether it can auto-fix.
//!
//! `ipe lint --help` and the `lint.ipe` unknown-rule check both read this table,
//! so a rule that runs but is undescribed — or a name accepted in `lint.ipe`
//! that no rule implements — cannot exist. Adding a rule is one entry here plus
//! its implementation in [`crate::rules`].

use crate::finding::Severity;

/// Whether a rule's findings carry a machine-applicable fix.
///
/// There is no default: every rule in [`RULES`] names one variant explicitly,
/// so a rule cannot ship without deciding whether `ipe lint --fix` — and the
/// generic LSP code action it powers — can act on its findings. `NotFixable`
/// carries the reason, so the decision reads as documentation, not a bare
/// `false`.
#[derive(Clone, Copy, Debug)]
pub enum Fixability {
    /// Every finding from this rule carries a [`crate::Fix`] (a local,
    /// single-module text edit) or a [`crate::SigFix`] (a cross-module
    /// call-site rewrite) the engine can apply mechanically.
    Fixable,
    /// No finding from this rule carries a fix. The rewrite needs human
    /// judgement the rule cannot make safely on its own.
    NotFixable(&'static str),
}

/// A shipped rule's metadata. The engine keys behaviour on `name`; the CLI and
/// `lint.ipe` reader read the rest.
#[derive(Clone, Copy, Debug)]
pub struct RuleInfo {
    /// The stable, hyphenated rule name used everywhere (`prim-param`).
    pub name: &'static str,
    /// A one-line description shown by `ipe lint --help`.
    pub summary: &'static str,
    /// The severity the rule reports at unless `lint.ipe` overrides it.
    pub default_severity: Severity,
    /// Whether findings from this rule carry a machine-applicable fix, and if
    /// not, why.
    pub fixability: Fixability,
}

/// Every shipped rule, described exactly once, in a stable order.
pub const RULES: &[RuleInfo] = &[
    RuleInfo {
        name: "prim-param",
        summary: "an exported signature takes a bare primitive where a domain newtype fits",
        default_severity: Severity::Warn,
        fixability: Fixability::Fixable,
    },
    RuleInfo {
        name: "adjacent-bools",
        summary: "two or more adjacent Bool parameters call sites cannot tell apart",
        default_severity: Severity::Warn,
        fixability: Fixability::Fixable,
    },
    RuleInfo {
        name: "wrapper-consistency",
        summary: "a shape wrapped as a newtype by sibling APIs is left bare in one",
        default_severity: Severity::Warn,
        fixability: Fixability::Fixable,
    },
    RuleInfo {
        name: "unsafe-convention",
        summary: "an unsafe* escape-hatch call, flagged so its use is deliberate and visible",
        default_severity: Severity::Warn,
        fixability: Fixability::NotFixable(
            "the fix is either keeping the escape hatch or restructuring the call to avoid it — a design choice only the author can make",
        ),
    },
    RuleInfo {
        name: "prefer-pipeline",
        summary: "a call chain nested two paren levels deep that reads clearer as a `|>` or `<|` pipe chain",
        default_severity: Severity::Warn,
        fixability: Fixability::Fixable,
    },
    RuleInfo {
        name: "unknown-suppression",
        summary: "an inline `-- ipe-lint: allow` comment names a rule that does not exist",
        default_severity: Severity::Warn,
        fixability: Fixability::NotFixable(
            "the fix is the rule name the author meant to write, which the rule cannot recover from a typo",
        ),
    },
    RuleInfo {
        name: "unused-imports",
        summary: "an import declaration whose bound names never appear in the module body",
        default_severity: Severity::Warn,
        fixability: Fixability::Fixable,
    },
    RuleInfo {
        name: "unused-bindings",
        summary: "a `let` binding whose name is never referenced in the enclosing scope",
        default_severity: Severity::Warn,
        fixability: Fixability::Fixable,
    },
    RuleInfo {
        name: "wrapper-consistency-cross",
        summary: "a shape wrapped as a newtype by APIs in other modules is left bare here",
        default_severity: Severity::Warn,
        fixability: Fixability::Fixable,
    },
    RuleInfo {
        name: "no-silent-outline-none",
        summary: "outline:none in a focus pseudo-class removes the keyboard focus indicator without a visible replacement",
        default_severity: Severity::Warn,
        fixability: Fixability::NotFixable(
            "the fix is a visible focus style, which the rule cannot choose on the author's behalf",
        ),
    },
    RuleInfo {
        name: "no-empty-icon-button-label",
        summary: "a Ui.iconButton label that is empty or whitespace-only leaves the control nameless to assistive tech",
        default_severity: Severity::Warn,
        fixability: Fixability::NotFixable(
            "the fix is the accessible label text, which only the author knows",
        ),
    },
    RuleInfo {
        name: "multiline-lambda-arg",
        summary: "a lambda spanning several lines passed inline as a call argument instead of bound by name",
        default_severity: Severity::Warn,
        fixability: Fixability::NotFixable(
            "naming the lambda is a judgment call only the author can make well",
        ),
    },
    RuleInfo {
        name: "no-bool-literal-compare",
        summary: "an `==` / `/=` comparison against a `True` / `False` literal",
        default_severity: Severity::Warn,
        fixability: Fixability::NotFixable(
            "the substitution can drop a comment or misjudge precedence inside the compared expression, which the rule does not check",
        ),
    },
    RuleInfo {
        name: "no-redundant-bool-if",
        summary: "an `if` whose branches are both `Bool` literals, restating its condition",
        default_severity: Severity::Warn,
        fixability: Fixability::NotFixable(
            "the substitution can drop a comment or misjudge precedence inside a branch, which the rule does not check",
        ),
    },
    RuleInfo {
        name: "no-simple-let-body",
        summary: "a `let` whose body only returns the name its last binding introduces",
        default_severity: Severity::Warn,
        fixability: Fixability::NotFixable(
            "the substitution can drop a comment attached to the binding or its body, which the rule does not check",
        ),
    },
    RuleInfo {
        name: "simplify-double-not",
        summary: "`not (not x)`, which restates `x`",
        default_severity: Severity::Warn,
        fixability: Fixability::Fixable,
    },
    RuleInfo {
        name: "simplify-map-identity",
        summary: "`List.map identity xs`, which restates `xs`",
        default_severity: Severity::Warn,
        fixability: Fixability::Fixable,
    },
    RuleInfo {
        name: "simplify-cons-append",
        summary: "`[ a ] ++ xs`, which restates `a :: xs`",
        default_severity: Severity::Warn,
        fixability: Fixability::Fixable,
    },
    RuleInfo {
        name: "no-redundant-cons",
        summary: "consing onto a list literal, which restates a longer list literal",
        default_severity: Severity::Warn,
        fixability: Fixability::Fixable,
    },
    RuleInfo {
        name: "no-redundant-concat",
        summary: "`List.concat` / `String.concat` of a single-element list, which restates that element",
        default_severity: Severity::Warn,
        fixability: Fixability::Fixable,
    },
    RuleInfo {
        name: "no-missing-type-annotation",
        summary: "a top-level declaration with no `: T` signature",
        default_severity: Severity::Allow,
        fixability: Fixability::NotFixable(
            "the fix is the inferred type signature, which this syntax-only rule has no type information to produce",
        ),
    },
    RuleInfo {
        name: "no-exposing-everything",
        summary: "a `module M exposing (..)` header, which exports every top-level declaration",
        default_severity: Severity::Warn,
        fixability: Fixability::NotFixable(
            "the explicit list a rewrite would write is the module's resolved export surface, which is canonicalisation's knowledge, not the parser's — and choosing what to hide is the author's decision the rule exists to prompt",
        ),
    },
    RuleInfo {
        name: "no-importing-everything",
        summary: "an `import M exposing (..)`, which brings every exported name into unqualified scope",
        default_severity: Severity::Warn,
        fixability: Fixability::NotFixable(
            "the explicit list a rewrite would write needs the dependency's resolved export surface, which is canonicalisation's knowledge, not the parser's",
        ),
    },
    RuleInfo {
        name: "no-unused-parameters",
        summary: "a function or lambda parameter the body never reads",
        default_severity: Severity::Warn,
        fixability: Fixability::Fixable,
    },
    RuleInfo {
        name: "no-unused-patterns",
        summary: "a variable a `case` arm, `let` destructure or `do` bind never reads",
        default_severity: Severity::Warn,
        fixability: Fixability::Fixable,
    },
];

/// The metadata for `name`, or `None` when no such rule ships.
#[must_use]
pub fn lookup(name: &str) -> Option<&'static RuleInfo> {
    RULES.iter().find(|r| r.name == name)
}

/// True when `name` is a shipped rule.
#[must_use]
pub fn is_known(name: &str) -> bool {
    lookup(name).is_some()
}
