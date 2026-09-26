//! Grep-gate: every raw network-dial call site in the runtime must be
//! accompanied by an SSRF guard (`VettedDial` or `ssrf_apply`).
//!
//! This test scans the four files that are the closed set of network-dial call
//! sites and asserts each `.connect(` / `builder_dangerous(` on a network path
//! has a guard adjacent in the same function.  A newly-added ungated dial fails
//! this test, keeping the egress class closed.

const GUARDS: &[&str] = &["VettedDial", "ssrf_apply"];

/// True when any guard marker appears within `window_lines` lines of `target_line`
/// in the line-oriented source view.  Using lines (not bytes) avoids slicing into
/// multi-byte UTF-8 chars in adjacent comments.
fn guarded_near_line(lines: &[String], target_line: usize, window_lines: usize) -> bool {
    let lo = target_line.saturating_sub(window_lines);
    let hi = (target_line + window_lines).min(lines.len());
    lines[lo..hi]
        .iter()
        .any(|l| GUARDS.iter().any(|g| l.contains(g)))
}

/// Blanks a line's string-literal contents and any trailing `//` comment.
///
/// So brace-counting never trips on a brace inside a string or a comment.
/// This is a line-oriented approximation, not a lexer: a string split across
/// lines, or a `"` inside a `//` comment, can still confuse it. No gated item
/// in this crate's test modules does that.
fn code_only(line: &str) -> String {
    let mut out = String::with_capacity(line.len());
    let mut chars = line.chars().peekable();
    let mut in_string = false;
    while let Some(c) = chars.next() {
        if in_string {
            if c == '\\' {
                chars.next();
            } else if c == '"' {
                in_string = false;
            }
            continue;
        }
        if c == '"' {
            in_string = true;
            continue;
        }
        if c == '/' && chars.peek() == Some(&'/') {
            break;
        }
        out.push(c);
    }
    out
}

/// Blanks every `#[cfg(test)]`-gated item in `src`, keeping line numbers.
///
/// Every dial-scan in this file must run over this helper's output, never raw
/// `src.lines()`: a test fixture's own guard token or dial call must never be
/// mistaken for a production one, and a `#[cfg(test)]` item that is NOT the
/// whole trailing test module (an inline test-only helper mid-file, as in
/// `db.rs`) must not truncate the scan of the production code that follows
/// it. The item's extent is found by brace-depth tracking (via [`code_only`]),
/// starting at the item's opening `{`, or — for a brace-free item such as
/// `#[cfg(test)] use path;` — at its terminating `;`.
fn strip_cfg_test_items(src: &str) -> Vec<String> {
    let raw: Vec<&str> = src.lines().collect();
    let mut out: Vec<String> = raw.iter().map(|l| (*l).to_string()).collect();
    let mut i = 0;
    while i < raw.len() {
        if raw[i].trim() != "#[cfg(test)]" {
            i += 1;
            continue;
        }
        let start = i;
        let mut j = i + 1;
        // Stacked attributes between `#[cfg(test)]` and the item itself.
        while j < raw.len() && raw[j].trim_start().starts_with('#') {
            j += 1;
        }
        let mut depth: i32 = 0;
        let mut opened = false;
        while j < raw.len() {
            let code = code_only(raw[j]);
            for ch in code.chars() {
                match ch {
                    '{' => {
                        depth += 1;
                        opened = true;
                    }
                    '}' => depth -= 1,
                    _ => {}
                }
            }
            j += 1;
            let done = if opened {
                depth <= 0
            } else {
                code.trim_end().ends_with(';')
            };
            if done {
                break;
            }
        }
        for line in out.iter_mut().take(j).skip(start) {
            line.clear();
        }
        i = j;
    }
    out
}

#[test]
fn strip_cfg_test_items_keeps_production_code_after_an_early_test_module() {
    let fixture = "#[cfg(test)]\n\
mod t {\n\
    fn helper() {\n\
        let _ = PgPool::connect(\"unguarded-test-only\");\n\
    }\n\
}\n\
\n\
fn production() {\n\
    PgPool::connect(\"unguarded-production\");\n\
}\n";
    let stripped = strip_cfg_test_items(fixture);
    let joined = stripped.join("\n");
    assert!(
        joined.contains("PgPool::connect(\"unguarded-production\")"),
        "production code after an early #[cfg(test)] module must survive stripping:\n{joined}"
    );
    assert!(
        !joined.contains("unguarded-test-only"),
        "code inside the #[cfg(test)] module must be stripped:\n{joined}"
    );
}

#[test]
fn strip_cfg_test_items_handles_a_single_line_gated_item_and_braces_in_strings() {
    let fixture = "#[cfg(test)]\n\
fn only_in_tests() { let s = \"{ not a real brace }\"; }\n\
\n\
fn build_pool() {\n\
    PgPool::connect(url);\n\
}\n";
    let stripped = strip_cfg_test_items(fixture);
    let joined = stripped.join("\n");
    assert!(
        joined.contains("PgPool::connect(url)"),
        "production code after a single-line gated item must survive stripping:\n{joined}"
    );
    assert!(
        !joined.contains("only_in_tests"),
        "the single-line gated item must be stripped:\n{joined}"
    );
}

#[test]
fn external_conn_postgres_dial_is_guarded() {
    let src = include_str!("../src/external_conn.rs");
    // Only production code dials — a test fixture's own VettedDial token must
    // never vouch for a production dial.
    let lines = strip_cfg_test_items(src);
    for (i, line) in lines.iter().enumerate() {
        if !line.contains(".connect(") {
            continue;
        }
        // SQLite path has no host to gate — skip it.
        let ctx_start = i.saturating_sub(10);
        let ctx: String = lines[ctx_start..=i].join("\n");
        if ctx.contains("Sqlite") || ctx.contains("SqlitePool") {
            continue;
        }
        assert!(
            guarded_near_line(&lines, i, 60),
            "unguarded .connect( at line {} in external_conn.rs — add a VettedDial guard\n{}",
            i + 1,
            line
        );
    }
}

#[test]
fn db_pool_connect_is_guarded() {
    let src = include_str!("../src/db.rs");
    // Only production code, not any `#[cfg(test)]` item (inline test-only
    // helper or the trailing test module).
    let lines = strip_cfg_test_items(src);
    for (i, line) in lines.iter().enumerate() {
        if !line.contains(".connect(") {
            continue;
        }
        // SQLite / file / in-memory dials carry no host — exempt.
        let ctx_start = i.saturating_sub(30);
        let ctx: String = lines[ctx_start..=i].join("\n");
        if ctx.contains("sqlite") || ctx.contains("file") || ctx.contains(":memory:") {
            continue;
        }
        assert!(
            guarded_near_line(&lines, i, 60),
            "unguarded .connect( at line {} in db.rs — add a VettedDial guard\n{}",
            i + 1,
            line
        );
    }
}

#[test]
fn email_smtp_builder_dangerous_is_guarded() {
    let src = include_str!("../src/email.rs");
    // Skip any `#[cfg(test)]` item, not just a trailing test module.
    let lines = strip_cfg_test_items(src);
    for (i, line) in lines.iter().enumerate() {
        if !line.contains("builder_dangerous(") {
            continue;
        }
        assert!(
            guarded_near_line(&lines, i, 60),
            "unguarded builder_dangerous( at line {} in email.rs — add a VettedDial guard\n{}",
            i + 1,
            line
        );
    }
}

/// Raw sqlx pool openers: each bypasses `VettedPool::connect` (SSRF gate,
/// connection cap, version floor, credential-free errors).
const RAW_POOL_OPENERS: &[&str] = &[
    "Pool::connect",
    "PoolOptions",
    "connect_with(",
    "connect_lazy",
];

/// The files that open a pool from a caller-supplied connection URL must do so
/// only through `VettedPool::connect`, never a raw sqlx opener whose error may
/// echo the URL's credentials.
#[test]
fn caller_url_pools_open_only_through_vetted_pool() {
    let sources = [
        ("web/store.rs", include_str!("../src/web/store.rs")),
        ("external_conn.rs", include_str!("../src/external_conn.rs")),
    ];
    for (name, src) in sources {
        // Production code only: any `#[cfg(test)]` item may open fixture pools.
        let lines = strip_cfg_test_items(src);
        for (i, line) in lines.iter().enumerate() {
            if line.trim_start().starts_with("//") {
                continue;
            }
            let code = line.replace("VettedPool", "");
            for opener in RAW_POOL_OPENERS {
                assert!(
                    !code.contains(opener),
                    "raw pool opener `{opener}` at {name}:{} — open through VettedPool::connect\n{line}",
                    i + 1
                );
            }
        }
    }
}
