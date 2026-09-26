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
fn guarded_near_line(lines: &[&str], target_line: usize, window_lines: usize) -> bool {
    let lo = target_line.saturating_sub(window_lines);
    let hi = (target_line + window_lines).min(lines.len());
    lines[lo..hi]
        .iter()
        .any(|l| GUARDS.iter().any(|g| l.contains(g)))
}

#[test]
fn external_conn_postgres_dial_is_guarded() {
    let src = include_str!("../src/external_conn.rs");
    let lines: Vec<&str> = src.lines().collect();
    // Only production code dials — stop at the test module (`#[cfg(test)]`) so a
    // test fixture's own VettedDial token can never vouch for a production dial.
    let cfg_test_line = lines
        .iter()
        .position(|l| l.trim() == "#[cfg(test)]")
        .unwrap_or(lines.len());
    for (i, line) in lines.iter().enumerate() {
        if i >= cfg_test_line {
            break;
        }
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
    let lines: Vec<&str> = src.lines().collect();
    // Find the line range of `async fn build_pool` — only that function dials
    // a raw network URL.  Skip test-module lines (after `#[cfg(test)]`).
    let cfg_test_line = lines
        .iter()
        .position(|l| l.trim() == "#[cfg(test)]")
        .unwrap_or(lines.len());
    for (i, line) in lines.iter().enumerate() {
        // Only production code, not the test module.
        if i >= cfg_test_line {
            break;
        }
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
    let lines: Vec<&str> = src.lines().collect();
    // Skip test module lines.
    let cfg_test_line = lines
        .iter()
        .position(|l| l.trim() == "#[cfg(test)]")
        .unwrap_or(lines.len());
    for (i, line) in lines.iter().enumerate() {
        if i >= cfg_test_line {
            break;
        }
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
        let lines: Vec<&str> = src.lines().collect();
        // Production code only: the test module may open fixture pools.
        let tests_start = lines
            .iter()
            .position(|l| l.trim_start().starts_with("mod tests"))
            .unwrap_or(lines.len());
        for (i, line) in lines.iter().enumerate().take(tests_start) {
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
