//! The Ipê-native `lint.ipe` config and inline per-site suppression.
//!
//! `lint.ipe` is written in Ipê, not TOML: a single `lint` value built from a
//! `Lint.config` seed threaded through `Lint.allow` / `Lint.warn` / `Lint.deny`
//! / `Lint.gate` stages, exactly as `package.ipe` threads `Package.*` stages.
//! It is READ, never evaluated — the reader walks the parsed AST of the sole
//! `lint` binding and recognises each blessed `Lint.*` builder by name. An
//! unknown rule name fails closed with a `file:line:col` message.
//!
//! Inline suppression is a source comment `-- ipe-lint: allow <rule>`; a finding
//! whose rule matches a suppression on (or just above) its line is dropped.

use std::collections::BTreeMap;

use ipe_diagnostics::Span;
use ipe_intern::Interner;
use ipe_syntax::{Expr, Expr_, Module};

use crate::finding::Severity;
use crate::registry;

/// A resolved `lint.ipe`: the per-rule severity overrides and the CI gate level.
///
/// A rule absent from `overrides` reports at its registry default. `gate` is the
/// severity at or above which a surviving finding fails CI (`Deny` by default —
/// only denied findings gate).
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct LintConfig {
    /// Rule name → the severity `lint.ipe` set for it (overriding the default).
    overrides: BTreeMap<String, Severity>,
    /// The CI gate level: a surviving finding at or above this severity fails.
    gate: Severity,
}

impl Default for LintConfig {
    fn default() -> Self {
        Self {
            overrides: BTreeMap::new(),
            gate: Severity::Deny,
        }
    }
}

impl LintConfig {
    /// The effective severity for `rule`: its `lint.ipe` override, else its
    /// registry default. An unknown rule (never happens for a shipped rule name)
    /// reports at `Warn`.
    #[must_use]
    pub fn severity_of(&self, rule: &str) -> Severity {
        if let Some(sev) = self.overrides.get(rule) {
            return *sev;
        }
        registry::lookup(rule).map_or(Severity::Warn, |r| r.default_severity)
    }

    /// The CI gate level — a surviving finding at or above this fails the gate.
    #[must_use]
    pub const fn gate(&self) -> Severity {
        self.gate
    }
}

/// A `lint.ipe` reader failure.
///
/// Mirrors the `package.ipe` reader's fail-closed, position-anchored rejection:
/// a shape/rule error carries a ready-to-print `file:line:col: reason` message,
/// and a raw parse failure carries the compiler [`Diagnostic`] so the caller
/// renders it with the shared renderer.
#[derive(Clone, Debug)]
pub enum ConfigError {
    /// A malformed shape, an unknown rule, or a non-literal stage — anchored to
    /// `path:line:col` and ready to print verbatim.
    Rejected(String),
    /// The `lint.ipe` source did not parse; the caller renders the diagnostic
    /// against `path`/source.
    Parse {
        /// The file whose parse failed (for the rendered location line).
        path: String,
        /// The full source, for the rendered snippet.
        src: String,
        /// The parse diagnostic.
        diag: Box<ipe_diagnostics::Diagnostic>,
    },
}

impl std::fmt::Display for ConfigError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Rejected(message) => f.write_str(message),
            Self::Parse { path, src, diag } => {
                f.write_str(&ipe_diagnostics::render(diag, path, src))
            }
        }
    }
}

impl std::error::Error for ConfigError {}

/// Read a `lint.ipe` source into a [`LintConfig`].
///
/// `path` names the file for diagnostics only. The source is parsed with the
/// compiler front-end and its sole `lint` binding walked; no expression is ever
/// evaluated. An unknown rule name, a non-literal argument, or an unexpected
/// declaration is a fail-closed [`ConfigError`].
///
/// # Errors
/// [`ConfigError`] on a parse failure, an unexpected module shape, an unknown
/// rule name, or a non-literal / non-blessed stage.
pub fn read_lint_config(src: &str, path: &str) -> Result<LintConfig, ConfigError> {
    let mut interner = Interner::new();
    let module =
        ipe_parse::parse_module(src, &mut interner).map_err(|diag| ConfigError::Parse {
            path: path.to_owned(),
            src: src.to_owned(),
            diag: Box::new(diag),
        })?;
    let reader = Reader {
        interner: &interner,
        src,
        path,
    };
    reader.read_module(&module)
}

/// The borrowed context each walk step shares.
struct Reader<'a> {
    interner: &'a Interner,
    src: &'a str,
    path: &'a str,
}

impl Reader<'_> {
    fn text(&self, sym: ipe_intern::Symbol) -> &str {
        self.interner.resolve(sym).unwrap_or("")
    }

    fn reject(&self, span: Span, reason: &str) -> ConfigError {
        let (line, col) = line_col(self.src, span.lo);
        ConfigError::Rejected(format!("{}:{line}:{col}: {reason}", self.path))
    }

    /// Walk the whole module: no imports, no type declarations, exactly one
    /// top-level `lint` value binding taking no parameters.
    fn read_module(&self, module: &Module) -> Result<LintConfig, ConfigError> {
        if let Some(import) = module.imports.first() {
            return Err(self.reject(
                import.name.span,
                "a lint.ipe may not `import` anything — the `Lint.*` vocabulary is recognised by \
                 name, never imported",
            ));
        }
        if let Some(union) = module.unions.first() {
            return Err(self.reject(
                union.value.name.span,
                "a lint.ipe declares only the `lint` value — a `type` declaration is not allowed",
            ));
        }

        let mut lint_value: Option<&ipe_diagnostics::Located<ipe_syntax::Value>> = None;
        for value in &module.values {
            if self.text(value.value.name.value) == "lint" {
                if lint_value.is_some() {
                    return Err(self.reject(
                        value.value.name.span,
                        "a lint.ipe declares the `lint` value exactly once",
                    ));
                }
                lint_value = Some(value);
            } else {
                return Err(self.reject(
                    value.value.name.span,
                    "a lint.ipe declares only the `lint` value — an extra top-level binding is not \
                     allowed",
                ));
            }
        }

        let Some(lint) = lint_value else {
            return Err(ConfigError::Rejected(format!(
                "{}: no top-level `lint = …` binding found",
                self.path
            )));
        };
        if !lint.value.patterns.is_empty() {
            return Err(self.reject(
                lint.value.name.span,
                "`lint` must be a value binding, not a function — it takes no parameters",
            ));
        }

        let mut config = LintConfig::default();
        for stage in self.linearise_pipeline(&lint.value.body)? {
            self.apply_stage(stage, &mut config)?;
        }
        Ok(config)
    }

    /// Linearise the `|>` spine into ordered stages. A bare `Lint.config` head
    /// (no pipeline) is a single-stage list. Only `|>` may thread the pipeline.
    fn linearise_pipeline<'e>(&self, body: &'e Expr) -> Result<Vec<&'e Expr>, ConfigError> {
        match &body.value {
            Expr_::Binops(ops, last) => {
                let mut stages = Vec::with_capacity(ops.len() + 1);
                for (operand, op) in ops {
                    if self.text(op.value) != "|>" {
                        return Err(self
                            .reject(op.span, "the lint pipeline may only be threaded with `|>`"));
                    }
                    stages.push(operand);
                }
                stages.push(last.as_ref());
                Ok(stages)
            }
            _ => Ok(vec![body]),
        }
    }

    /// Apply one `Lint.*` stage to the accumulating config.
    fn apply_stage(&self, stage: &Expr, config: &mut LintConfig) -> Result<(), ConfigError> {
        let (module, name, args) = self.expect_blessed_call(stage)?;
        match (module, name) {
            // The pipeline seed; carries no field.
            ("Lint", "config") => Ok(()),
            ("Lint", "allow") => self.set_severity(stage.span, name, args, Severity::Allow, config),
            ("Lint", "warn") => self.set_severity(stage.span, name, args, Severity::Warn, config),
            ("Lint", "deny") => self.set_severity(stage.span, name, args, Severity::Deny, config),
            ("Lint", "gate") => {
                let word = self.one_string(stage.span, name, args)?;
                config.gate = self.read_gate(&word, stage.span)?;
                Ok(())
            }
            _ => Err(self.reject(
                stage.span,
                &format!(
                    "`{module}.{name}` is not a lint-pipeline stage — expected a `Lint.*` builder"
                ),
            )),
        }
    }

    /// Set one rule's severity, rejecting an unknown rule name fail-closed.
    fn set_severity(
        &self,
        span: Span,
        builder: &str,
        args: &[Expr],
        sev: Severity,
        config: &mut LintConfig,
    ) -> Result<(), ConfigError> {
        let rule = self.one_string(span, builder, args)?;
        if !registry::is_known(&rule) {
            return Err(self.reject(
                span,
                &format!(
                    "`{rule}` is not a known lint rule — see `ipe lint --help` for the rule set"
                ),
            ));
        }
        config.overrides.insert(rule, sev);
        Ok(())
    }

    /// Read a gate word (`allow` / `warn` / `deny`) into a [`Severity`].
    fn read_gate(&self, word: &str, span: Span) -> Result<Severity, ConfigError> {
        match word {
            "allow" => Ok(Severity::Allow),
            "warn" => Ok(Severity::Warn),
            "deny" => Ok(Severity::Deny),
            other => Err(self.reject(
                span,
                &format!("`Lint.gate {other:?}` — the gate level must be \"allow\", \"warn\", or \"deny\""),
            )),
        }
    }

    /// Require a `Module.name` call, returning `(module, name, args)`. A local
    /// name, lambda, or computed callee is rejected — no user function may appear.
    fn expect_blessed_call<'e>(
        &self,
        expr: &'e Expr,
    ) -> Result<(&str, &str, &'e [Expr]), ConfigError> {
        match &expr.value {
            Expr_::Call(callee, args) => match &callee.value {
                Expr_::VarQual(m, n) => Ok((self.text(*m), self.text(*n), args.as_slice())),
                _ => Err(self.reject(
                    callee.span,
                    "a lint stage's callee must be a blessed `Lint.builder` name",
                )),
            },
            Expr_::VarQual(m, n) => Ok((self.text(*m), self.text(*n), &[])),
            _ => Err(self.reject(
                expr.span,
                "expected a blessed `Lint.*` call in the lint pipeline",
            )),
        }
    }

    /// Require exactly one string-literal argument.
    fn one_string(&self, span: Span, builder: &str, args: &[Expr]) -> Result<String, ConfigError> {
        if args.len() != 1 {
            return Err(self.reject(
                span,
                &format!(
                    "`{builder}` takes exactly one string argument, got {}",
                    args.len()
                ),
            ));
        }
        let arg = args.first().ok_or_else(|| {
            self.reject(span, &format!("`{builder}` is missing its string argument"))
        })?;
        match &arg.value {
            Expr_::Str(s) => Ok(s.clone()),
            _ => Err(self.reject(
                arg.span,
                "expected a string literal — a lint.ipe field may only be written as a literal, \
                 never computed",
            )),
        }
    }
}

/// 1-based line and column for a byte offset, clamped so an out-of-range offset
/// degrades to `1:1` rather than panicking.
fn line_col(src: &str, byte: u32) -> (usize, usize) {
    let byte = (byte as usize).min(src.len());
    let mut clamped = byte;
    while clamped > 0 && !src.is_char_boundary(clamped) {
        clamped -= 1;
    }
    let before = src.get(..clamped).unwrap_or("");
    let line = before.bytes().filter(|&b| b == b'\n').count() + 1;
    let line_start = before.rfind('\n').map_or(0, |i| i + 1);
    let col = src.get(line_start..clamped).unwrap_or("").chars().count() + 1;
    (line, col)
}

/// The 0-based line numbers carrying an inline `-- ipe-lint: allow <rule>`
/// suppression, mapped to the set of rule names suppressed there.
///
/// A suppression silences a finding on its own line and on the next
/// source line — so a comment placed on the line above a signature suppresses a
/// finding that points at the signature. `all` suppresses every rule on the line.
#[derive(Clone, Debug, Default)]
pub struct Suppressions {
    /// 0-based line number → rule names suppressed for that line's findings.
    by_line: BTreeMap<usize, RuleSet>,
}

/// The rules suppressed at one line: an explicit set, or everything (`all`).
#[derive(Clone, Debug)]
enum RuleSet {
    /// Only these named rules.
    Named(std::collections::BTreeSet<String>),
    /// Every rule (`-- ipe-lint: allow all`).
    All,
}

/// The marker introducing an inline suppression comment.
const MARKER: &str = "-- ipe-lint: allow ";

impl Suppressions {
    /// Scan `src` for inline suppression comments.
    #[must_use]
    pub fn scan(src: &str) -> Self {
        let mut by_line: BTreeMap<usize, RuleSet> = BTreeMap::new();
        for (line_no, line) in src.lines().enumerate() {
            let Some(idx) = line.find(MARKER) else {
                continue;
            };
            let rest = line.get(idx + MARKER.len()..).unwrap_or("").trim();
            if rest == "all" {
                by_line.insert(line_no, RuleSet::All);
                continue;
            }
            // Comma- or space-separated rule names after the marker.
            let names: std::collections::BTreeSet<String> = rest
                .split(|c: char| c == ',' || c.is_whitespace())
                .filter(|s| !s.is_empty())
                .map(str::to_owned)
                .collect();
            if !names.is_empty() {
                by_line.insert(line_no, RuleSet::Named(names));
            }
        }
        Self { by_line }
    }

    /// True when a finding for `rule` at 0-based `line` is suppressed — by a
    /// comment on that line or on the line immediately above it.
    #[must_use]
    pub fn suppresses(&self, rule: &str, line: usize) -> bool {
        self.line_suppresses(rule, line)
            || line
                .checked_sub(1)
                .is_some_and(|above| self.line_suppresses(rule, above))
    }

    fn line_suppresses(&self, rule: &str, line: usize) -> bool {
        match self.by_line.get(&line) {
            Some(RuleSet::All) => true,
            Some(RuleSet::Named(names)) => names.iter().any(|n| n == rule),
            None => false,
        }
    }
}
