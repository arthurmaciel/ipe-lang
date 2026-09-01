//! The lint vocabulary shared by every rule: [`Severity`], [`Finding`], and
//! [`Fix`].
//!
//! A rule is a pure function from the typed program to a list of [`Finding`]s.
//! A finding names the rule that raised it, points at a source [`Span`],
//! carries a one-line message plus optional teaching `help` lines, and — when
//! and only when a semantics-preserving rewrite exists — a machine-applicable
//! [`Fix`]. Findings sort by `(module, span, rule)` so the report and every
//! golden are deterministic regardless of the order rules ran.

use std::cmp::Ordering;

use ipe_diagnostics::Span;

/// A rule's default reporting register, and the axis the CI gate compares against.
///
/// `Allow` silences a rule; `Warn` reports it without failing a build; `Deny`
/// reports it and, when it survives suppression, fails the gate. Ordered
/// `Allow < Warn < Deny` so "at or above the gate level" is a plain comparison.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug, Hash)]
pub enum Severity {
    /// The rule is off — it produces no findings.
    Allow,
    /// The rule reports, but a surviving finding does not fail the CI gate.
    Warn,
    /// The rule reports, and a surviving finding fails the CI gate.
    Deny,
}

impl Severity {
    /// The lower-case word used in `lint.ipe` (`Lint.warn "…"`) and in output.
    #[must_use]
    pub const fn word(self) -> &'static str {
        match self {
            Self::Allow => "allow",
            Self::Warn => "warn",
            Self::Deny => "deny",
        }
    }
}

/// A machine-applicable, semantics-preserving source edit.
///
/// A `Fix` replaces the half-open byte range `span` of the module's source with
/// `replacement`. It is emitted ONLY when the rewrite is provably equivalent to
/// the original (an idiom reshuffle, never a semantic change), so applying every
/// fix and re-running the linter reports strictly fewer findings and never a new
/// one. A rule whose remedy would change an exported signature or need call-site
/// threading carries no `Fix` — it stays advisory.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Fix {
    /// A one-line description of what the edit does (`rewrite as a pipeline`).
    pub describe: String,
    /// The byte range in the owning module's source that `replacement` supplants.
    pub span: Span,
    /// The exact text that replaces `span`.
    pub replacement: String,
}

/// One reported lint observation over a valid program.
///
/// `rule` is the stable, hyphenated rule name (`prim-param`) used in output, in
/// `lint.ipe`, and in the inline `-- ipe-lint: allow <rule>` suppression.
/// `module` is the owning module's dotted path segments — carried so findings
/// from a multi-module project sort and render deterministically. `span` points
/// at the offending source; `message` is the one-line prose; `help` is zero or
/// more teaching follow-up lines (why, the idiom, how to suppress). `fix` is
/// present only for a semantics-preserving rewrite.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Finding {
    /// The stable rule name (e.g. `prim-param`).
    pub rule: &'static str,
    /// The owning module's dotted path segments (e.g. `["Main"]`).
    pub module: Vec<String>,
    /// The offending source location in the owning module.
    pub span: Span,
    /// The one-line, second-person message.
    pub message: String,
    /// Teaching follow-up lines, rendered under the snippet.
    pub help: Vec<String>,
    /// A semantics-preserving rewrite, when one exists; otherwise `None`.
    pub fix: Option<Fix>,
}

impl Finding {
    /// Deterministic total order: by owning module, then source position, then
    /// rule name. Two findings from distinct rules at the identical span still
    /// order stably by rule name, so a report and its golden never flap.
    #[must_use]
    pub fn order_key(&self) -> (&[String], u32, u32, &str) {
        (&self.module, self.span.lo, self.span.hi, self.rule)
    }
}

impl PartialOrd for Finding {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for Finding {
    fn cmp(&self, other: &Self) -> Ordering {
        self.order_key().cmp(&other.order_key())
    }
}
