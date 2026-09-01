//! The rule registry — the single source of truth for every shipped rule's
//! name, one-line summary, default severity, and whether it can auto-fix.
//!
//! `ipe lint --help` and the `lint.ipe` unknown-rule check both read this table,
//! so a rule that runs but is undescribed — or a name accepted in `lint.ipe`
//! that no rule implements — cannot exist. Adding a rule is one entry here plus
//! its implementation in [`crate::rules`].

use crate::finding::Severity;

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
    /// True when the rule can emit a semantics-preserving [`crate::Fix`] that
    /// `ipe lint --fix` applies. Advisory rules (a design suggestion needing
    /// call-site threading) are `false` and never rewrite source.
    pub fixable: bool,
}

/// Every shipped rule, described exactly once, in a stable order.
pub const RULES: &[RuleInfo] = &[
    RuleInfo {
        name: "prim-param",
        summary: "an exported signature takes a bare primitive where a domain newtype fits",
        default_severity: Severity::Warn,
        fixable: false,
    },
    RuleInfo {
        name: "adjacent-bools",
        summary: "two or more adjacent Bool parameters call sites cannot tell apart",
        default_severity: Severity::Warn,
        fixable: false,
    },
    RuleInfo {
        name: "wrapper-consistency",
        summary: "a shape wrapped as a newtype by sibling APIs is left bare in one",
        default_severity: Severity::Warn,
        fixable: false,
    },
    RuleInfo {
        name: "unsafe-convention",
        summary: "an unsafe* escape-hatch call, flagged so its use is deliberate and visible",
        default_severity: Severity::Warn,
        fixable: false,
    },
    RuleInfo {
        name: "prefer-pipeline",
        summary: "a nested call chain that reads clearer left-to-right as a |> pipeline",
        default_severity: Severity::Warn,
        fixable: true,
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
