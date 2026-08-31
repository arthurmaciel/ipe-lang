/// A documented `IPE_*` environment variable.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EnvVar {
    /// The environment variable name, e.g. `IPE_LOG_LEVEL`.
    pub name: &'static str,
    /// Default value as a human-readable string, e.g. `"info"` or `"unset"`.
    pub default: &'static str,
    /// One-line description of what the variable controls.
    pub purpose: &'static str,
    /// The runtime subsystem that reads this variable.
    pub subsystem: Subsystem,
    /// Operational classification.
    pub class: Class,
}

/// Operational classification of an environment variable.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Class {
    /// A runtime knob that changes behaviour or performance. Safe to expose in
    /// documentation; the default is always the conservative choice.
    Tunable,
    /// A secret credential — token, key, or password. Provide via a secret
    /// manager; never commit or log the value.
    Secret,
    /// A security-boundary switch. Loosening widens the trust boundary;
    /// document the trade-off before changing from the default.
    SecurityTunable,
}

impl Class {
    /// Short label used in the generated reference table.
    #[must_use]
    pub const fn badge(self) -> &'static str {
        match self {
            Self::Tunable => "Tunable",
            Self::Secret => "Secret",
            Self::SecurityTunable => "SecurityTunable",
        }
    }
}

/// The subsystem that reads the variable — used for grouping in the reference.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Subsystem {
    /// `ipe build` / static-build pipeline.
    Build,
    /// Type solver and compiler internals.
    Compiler,
    /// Console / developer-console proxy and auth.
    Console,
    /// CSV serialisation.
    Csv,
    /// Database connection pools.
    Db,
    /// Email sending.
    Email,
    /// File I/O limits.
    File,
    /// FFI / Rust-crate binding sandbox.
    Ffi,
    /// Outbound HTTP client.
    Http,
    /// Observability: logging, tracing, telemetry export.
    Observability,
    /// Compression (gzip / zstd decompression).
    Compression,
    /// Config-file loading.
    Config,
    /// Runtime embedding and home directory.
    Runtime,
    /// Web server — session, CSRF, routing, static assets.
    Web,
    /// WebSocket (client and server).
    Ws,
    /// `ipe doc` / documentation server.
    Doc,
}

impl Subsystem {
    /// Human-readable display name for the generated reference.
    #[must_use]
    pub const fn display(self) -> &'static str {
        match self {
            Self::Build => "Build",
            Self::Compiler => "Compiler",
            Self::Console => "Console",
            Self::Csv => "CSV",
            Self::Db => "Database",
            Self::Doc => "Doc",
            Self::Email => "Email",
            Self::Ffi => "FFI",
            Self::File => "File",
            Self::Http => "HTTP client",
            Self::Observability => "Observability",
            Self::Compression => "Compression",
            Self::Config => "Config",
            Self::Runtime => "Runtime",
            Self::Web => "Web server",
            Self::Ws => "WebSocket",
        }
    }
}

/// The complete annotated table of `IPE_*` environment variables.
///
/// Variables are listed in alphabetical order within each subsystem. The drift
/// gate test asserts that every `IPE_*` literal read in the codebase appears
/// here; the generator renders this table into `docs/reference/env.md`.
///
/// Excluded from this table:
/// - Test-only variables (`IPE_TEST_*`, `IPE_BLESS`, `IPE_RUN_WITH_TEST_VAR`,
///   `IPE_LOAD_ENV_PROBE_VAR`, `IPE_HTTP_TEST_URL`, `IPE_ORACLE_SHARED_TARGET`,
///   `IPE_DEBUG_TODO_SUBPROCESS`).
/// - Deprecated `IPE_LIVE_*` aliases (being removed; documented as "deprecated
///   alias" in the `purpose` field of their canonical `IPE_WEB_*` replacement).
/// - Build-time baked vars set by `option_env!` only
///   (`IPE_BUILD_COMMIT`, `IPE_BUILD_AT`, `IPE_VERSION`) — documented here for
///   operator awareness but never read via `std::env::var` at runtime.
pub static ENV_VARS: &[EnvVar] = &[
    // ── Build ─────────────────────────────────────────────────────────────────
    EnvVar {
        name: "IPE_ALLOC",
        default: "unset (system allocator)",
        purpose: "Select the memory allocator: `mimalloc`, `jemalloc`, or `system`. \
                  Mirrors `--allocator`; env wins over `package.ipe [rust] allocator`.",
        subsystem: Subsystem::Build,
        class: Class::Tunable,
    },
    EnvVar {
        name: "IPE_BIN",
        default: "unset",
        purpose: "Path to the `ipe` binary used by the build driver when invoking itself \
                  recursively. Set automatically by the wrapper; operator override is rarely needed.",
        subsystem: Subsystem::Build,
        class: Class::Tunable,
    },
    EnvVar {
        name: "IPE_BUILD_AT",
        default: "unknown",
        purpose: "Build timestamp baked in by CI (`option_env!`). Surfaced at \
                  `GET /_ipe/buildinfo`. Not read at runtime via `env::var`.",
        subsystem: Subsystem::Build,
        class: Class::Tunable,
    },
    EnvVar {
        name: "IPE_BUILD_CACHE",
        default: "on",
        purpose: "Set to `0`, `off`, or `false` to disable the incremental build cache. \
                  Default is on; the cache directory is `<out>/.ipe-cache` unless \
                  `IPE_BUILD_CACHE_DIR` is set.",
        subsystem: Subsystem::Build,
        class: Class::Tunable,
    },
    EnvVar {
        name: "IPE_BUILD_CACHE_DIR",
        default: "unset (<out>/.ipe-cache)",
        purpose: "Explicit path for the incremental build cache directory. Takes effect \
                  only when the cache is enabled (`IPE_BUILD_CACHE` not `off`).",
        subsystem: Subsystem::Build,
        class: Class::Tunable,
    },
    EnvVar {
        name: "IPE_BUILD_COMMIT",
        default: "dev",
        purpose: "Git commit SHA baked in by CI (`option_env!`). Surfaced at \
                  `GET /_ipe/buildinfo`. Not read at runtime via `env::var`.",
        subsystem: Subsystem::Build,
        class: Class::Tunable,
    },
    EnvVar {
        name: "IPE_CFREE",
        default: "unset (false)",
        purpose: "Set to `1` or `true` to build without linking any C code. Mirrors \
                  `--cfree`; incompatible with allocators that require C (e.g. mimalloc).",
        subsystem: Subsystem::Build,
        class: Class::Tunable,
    },
    EnvVar {
        name: "IPE_EMBED_APP",
        default: "unset",
        purpose: "Path to the compiled app binary embedded into a wrapper binary. Set \
                  by the build driver; not intended for operator use.",
        subsystem: Subsystem::Build,
        class: Class::Tunable,
    },
    EnvVar {
        name: "IPE_EMBED_PROFILE",
        default: "unset",
        purpose: "Build profile string embedded into a wrapper binary alongside \
                  `IPE_EMBED_APP`. Set by the build driver.",
        subsystem: Subsystem::Build,
        class: Class::Tunable,
    },
    EnvVar {
        name: "IPE_INDEX_DIR",
        default: "unset (~/.ipe/index)",
        purpose: "Override the root directory of the package-index checkout used by \
                  `ipe add` / `ipe install`. Points to a local mirror of the \
                  ipe-registry index. Useful for air-gapped environments.",
        subsystem: Subsystem::Build,
        class: Class::Tunable,
    },
    EnvVar {
        name: "IPE_STATIC",
        default: "unset (dynamic build)",
        purpose: "Set to `1` or `true` to request a fully-static (musl) binary. Mirrors \
                  `--static`; env wins over `package.ipe [rust] static`.",
        subsystem: Subsystem::Build,
        class: Class::Tunable,
    },
    EnvVar {
        name: "IPE_TARGET",
        default: "unset (native)",
        purpose: "Cross-compilation target triple, e.g. `wasm32-unknown-unknown` or \
                  `aarch64-unknown-linux-musl`. Set to `wasm` as a shorthand for \
                  `wasm32-unknown-unknown`.",
        subsystem: Subsystem::Build,
        class: Class::Tunable,
    },
    EnvVar {
        name: "IPE_VERSION",
        default: "dev",
        purpose: "Compiler version baked in by CI (`option_env!`). Surfaced at \
                  `GET /_ipe/buildinfo` and used to locate the cached console binary. \
                  Not read at runtime via `env::var`.",
        subsystem: Subsystem::Build,
        class: Class::Tunable,
    },
    EnvVar {
        name: "IPE_WATCH_HOT_APPEARANCE",
        default: "unset (off)",
        purpose: "Set to any non-empty value other than `0` to enable dev-loop \
                  appearance hot-swap: an `ipe watch` edit to a style literal is \
                  pushed to the browser without a rebuild. Dev-only; no effect on a \
                  release build.",
        subsystem: Subsystem::Build,
        class: Class::Tunable,
    },
    EnvVar {
        name: "IPE_WATCH_HOT_TOKEN",
        default: "unset",
        purpose: "Per-process control token for the dev-only appearance-hot-swap \
                  endpoint. `ipe watch` sets it and sends it as `X-Ipe-Hot-Token`; a \
                  request whose token does not constant-time-match is refused. \
                  Dev-only; the endpoint is never mounted in a release build.",
        subsystem: Subsystem::Build,
        class: Class::Secret,
    },
    EnvVar {
        name: "IPE_WATCH_TIMING",
        default: "unset (off)",
        purpose: "Set to `1` or `true` to print a per-phase `ipe watch` rebuild \
                  breakdown (emit, cargo, restart, reconnect) to stderr. Dev-loop \
                  instrumentation; has no effect outside `ipe watch`.",
        subsystem: Subsystem::Build,
        class: Class::Tunable,
    },
    // ── Console ───────────────────────────────────────────────────────────────
    EnvVar {
        name: "IPE_ADMIN_TOKEN",
        default: "unset",
        purpose: "Bearer token granting access to the embedded developer console in \
                  production. Provide via your secret manager; never commit. Falls back \
                  to `IPE_CONSOLE_TOKEN`, then `IPE_METRICS_TOKEN`.",
        subsystem: Subsystem::Console,
        class: Class::Secret,
    },
    EnvVar {
        name: "IPE_CONSOLE_AUTH",
        default: "unset (token in production, off in dev)",
        purpose: "Console authentication mode: `token` (bearer-token gate), `off` \
                  (disable auth — dev only). Unset uses the production/dev heuristic.",
        subsystem: Subsystem::Console,
        class: Class::SecurityTunable,
    },
    EnvVar {
        name: "IPE_CONSOLE_BATCH_INTERVAL_MS",
        default: "2000",
        purpose: "Flush cadence (ms) for telemetry batches shipped to the Hub. Reduce \
                  for lower latency at the cost of more HTTP round-trips.",
        subsystem: Subsystem::Console,
        class: Class::Tunable,
    },
    EnvVar {
        name: "IPE_CONSOLE_BIN",
        default: "unset (~/.cache/ipe/rust-console/<version>/ipe-console)",
        purpose: "Explicit path to the `ipe-console` binary. Overrides the default \
                  cache location resolved from `IPE_VERSION`.",
        subsystem: Subsystem::Console,
        class: Class::Tunable,
    },
    EnvVar {
        name: "IPE_CONSOLE_DB_PATH",
        default: "unset (per-process temp file)",
        purpose: "Path to the SQLite database the console uses to store telemetry \
                  (logs, spans). Set automatically when embedding the console; \
                  operator override selects a persistent path.",
        subsystem: Subsystem::Console,
        class: Class::Tunable,
    },
    EnvVar {
        name: "IPE_CONSOLE_EMBED",
        default: "unset (on in development)",
        purpose: "Set to `off`, `0`, or `false` to disable the automatic embedded \
                  developer console. The console is never embedded in a sub-app \
                  (sub-app detection is automatic).",
        subsystem: Subsystem::Console,
        class: Class::Tunable,
    },
    EnvVar {
        name: "IPE_CONSOLE_HUB",
        default: "unset",
        purpose: "Base URL of a remote Ipê Hub OTLP collector. When set, the console \
                  ships telemetry there. Leave unset unless you operate a Hub.",
        subsystem: Subsystem::Console,
        class: Class::Tunable,
    },
    EnvVar {
        name: "IPE_CONSOLE_HUB_DB",
        default: "unset",
        purpose: "SQLite path the console child reads as its Hub data source. Set \
                  automatically by the console proxy when wiring the child; not for \
                  operator use.",
        subsystem: Subsystem::Console,
        class: Class::Tunable,
    },
    EnvVar {
        name: "IPE_CONSOLE_HUB_TOKEN",
        default: "unset",
        purpose: "Bearer token sent to the Hub OTLP collector. Must be at least 32 \
                  bytes; shorter tokens are refused. Provide via your secret manager.",
        subsystem: Subsystem::Console,
        class: Class::Secret,
    },
    EnvVar {
        name: "IPE_CONSOLE_TOKEN",
        default: "unset",
        purpose: "Deprecated alias for `IPE_ADMIN_TOKEN`. Prefer `IPE_ADMIN_TOKEN`. \
                  Provide via your secret manager; never commit.",
        subsystem: Subsystem::Console,
        class: Class::Secret,
    },
    EnvVar {
        name: "IPE_CONSOLE_URL",
        default: "unset (auto-detected sub-path)",
        purpose: "Explicit URL at which the developer console is reachable. Overrides \
                  the auto-detected `/_ipe/console` path for proxied deployments.",
        subsystem: Subsystem::Console,
        class: Class::Tunable,
    },
    EnvVar {
        name: "IPE_DEV_BANNER",
        default: "unset (on in development)",
        purpose: "Set to `off` or `0` to suppress the development-mode banner injected \
                  into HTML responses. The banner is never shown in production.",
        subsystem: Subsystem::Console,
        class: Class::Tunable,
    },
    EnvVar {
        name: "IPE_INGEST_TOKEN",
        default: "unset",
        purpose: "Bearer token the parent ingest gate checks on `X-Ipê-Ingest-Token`. \
                  Required when a sub-app pushes telemetry to a parent app's \
                  `/_ipe/ingest` endpoint. Provide via your secret manager.",
        subsystem: Subsystem::Console,
        class: Class::Secret,
    },
    EnvVar {
        name: "IPE_METRICS_TOKEN",
        default: "unset",
        purpose: "Deprecated alias for `IPE_ADMIN_TOKEN`. Prefer `IPE_ADMIN_TOKEN`. \
                  Provide via your secret manager; never commit.",
        subsystem: Subsystem::Console,
        class: Class::Secret,
    },
    // ── Compiler ──────────────────────────────────────────────────────────────
    EnvVar {
        name: "IPE_SOLVER_BUDGET",
        default: "5000000",
        purpose: "Maximum unification steps the type solver takes before giving up and \
                  emitting a budget-exceeded error. Set to `0` for no limit (escape hatch \
                  for programs with very large type graphs). Raise when the compiler \
                  reports a budget-exceeded diagnostic.",
        subsystem: Subsystem::Compiler,
        class: Class::Tunable,
    },
    // ── Compression ───────────────────────────────────────────────────────────
    EnvVar {
        name: "IPE_DECOMPRESS_MAX_BYTES",
        default: "268435456 (256 MiB)",
        purpose: "Maximum number of bytes that may be produced by a single decompression \
                  operation. Prevents zip-bomb / decompression-bomb exhaustion of memory.",
        subsystem: Subsystem::Compression,
        class: Class::Tunable,
    },
    // ── Config ────────────────────────────────────────────────────────────────
    EnvVar {
        name: "IPE_CONFIG_MAX_BYTES",
        default: "16777216 (16 MiB)",
        purpose: "Maximum size (bytes) of a config file loaded via `Config.load*`. \
                  Prevents memory exhaustion from unexpectedly large config files.",
        subsystem: Subsystem::Config,
        class: Class::Tunable,
    },
    EnvVar {
        name: "IPE_YAML_MAX_BYTES",
        default: "16777216 (16 MiB)",
        purpose: "Maximum YAML source size (bytes) that `Config.loadYaml` will parse. \
                  A separate ceiling from `IPE_CONFIG_MAX_BYTES`.",
        subsystem: Subsystem::Config,
        class: Class::Tunable,
    },
    // ── CSV ───────────────────────────────────────────────────────────────────
    EnvVar {
        name: "IPE_CSV_MAX_ROWS",
        default: "10000000 (10 M)",
        purpose: "Maximum rows parsed from a single CSV input. Prevents OOM from \
                  unbounded CSV streams.",
        subsystem: Subsystem::Csv,
        class: Class::Tunable,
    },
    EnvVar {
        name: "IPE_CSV_SANITIZE_FORMULAS",
        default: "unset (false)",
        purpose: "Set to `1`, `on`, `true`, or `yes` to prefix formula-injection \
                  characters (`=`, `+`, `-`, `@`) with a single quote in CSV output, \
                  preventing spreadsheet formula injection.",
        subsystem: Subsystem::Csv,
        class: Class::SecurityTunable,
    },
    // ── Database ──────────────────────────────────────────────────────────────
    EnvVar {
        name: "IPE_DB_MAX_CONNECTIONS",
        default: "10",
        purpose: "Maximum connections per database pool. Raise for high-concurrency \
                  workloads; lower to reduce database load.",
        subsystem: Subsystem::Db,
        class: Class::Tunable,
    },
    EnvVar {
        name: "IPE_DB_MAX_POOLS",
        default: "4",
        purpose: "Maximum number of distinct database pools (one per unique connection \
                  string). Raise if your app connects to many distinct databases.",
        subsystem: Subsystem::Db,
        class: Class::Tunable,
    },
    EnvVar {
        name: "IPE_DB_OP",
        default: "unset",
        purpose: "Database CLI op mode. Set to `migrate` to run pending migrations and \
                  exit. Intended for container entrypoints and deployment pipelines.",
        subsystem: Subsystem::Db,
        class: Class::Tunable,
    },
    // ── Doc ───────────────────────────────────────────────────────────────────
    EnvVar {
        name: "IPE_DOC_NO_OPEN",
        default: "unset",
        purpose: "Set to any non-empty value to prevent `ipe doc` from opening a \
                  browser tab. The URL is always printed to stdout. Useful in CI or \
                  headless environments.",
        subsystem: Subsystem::Doc,
        class: Class::Tunable,
    },
    // ── Email ─────────────────────────────────────────────────────────────────
    EnvVar {
        name: "IPE_EMAIL_DRY_RUN",
        default: "unset (false)",
        purpose: "Set to `1` to skip actual SMTP delivery and return a synthetic \
                  message ID. Useful in integration tests and staging environments.",
        subsystem: Subsystem::Email,
        class: Class::Tunable,
    },
    // ── FFI ───────────────────────────────────────────────────────────────────
    EnvVar {
        name: "IPE_FFI_ALLOW_UNSANDBOXED",
        default: "unset (false)",
        purpose: "Set to `1` to allow `ipe add` / `ipe install` to run the FFI \
                  inspector without a bubblewrap (`bwrap`) sandbox. Widens the trust \
                  boundary — untrusted build scripts execute without confinement. \
                  Never set in CI or production.",
        subsystem: Subsystem::Ffi,
        class: Class::SecurityTunable,
    },
    EnvVar {
        name: "IPE_FFI_CPU_SECS",
        default: "unset (sandbox default)",
        purpose: "CPU-time limit (seconds) for each sandboxed FFI inspector phase. \
                  Raise only when inspecting crates with very long compile phases.",
        subsystem: Subsystem::Ffi,
        class: Class::SecurityTunable,
    },
    EnvVar {
        name: "IPE_FFI_DBG",
        default: "unset",
        purpose: "Set to any non-empty value to enable verbose debug output from the \
                  FFI inspector. For developer diagnostics only.",
        subsystem: Subsystem::Ffi,
        class: Class::Tunable,
    },
    EnvVar {
        name: "IPE_FFI_FD_CAP",
        default: "unset (sandbox default)",
        purpose: "File-descriptor limit for each sandboxed FFI inspector phase.",
        subsystem: Subsystem::Ffi,
        class: Class::SecurityTunable,
    },
    EnvVar {
        name: "IPE_FFI_INSPECTOR",
        default: "unset (auto-located beside test binary)",
        purpose: "Explicit path to the `ipe-ffi-inspector` binary used in integration \
                  tests. Not needed in normal use.",
        subsystem: Subsystem::Ffi,
        class: Class::Tunable,
    },
    EnvVar {
        name: "IPE_FFI_OUT_CAP_MB",
        default: "unset (sandbox default)",
        purpose: "Output-size cap (MB) for each sandboxed FFI inspector phase. \
                  Prevents inspector stdout from exhausting memory.",
        subsystem: Subsystem::Ffi,
        class: Class::SecurityTunable,
    },
    EnvVar {
        name: "IPE_FFI_PROBE_DIR",
        default: "unset (per-run temp dir)",
        purpose: "Root directory for the FFI inspector's probe workspace. Setting a \
                  stable path allows Cargo to reuse dependency build artefacts across \
                  repeated `ipe add` invocations.",
        subsystem: Subsystem::Ffi,
        class: Class::Tunable,
    },
    EnvVar {
        name: "IPE_FFI_PROC_CAP",
        default: "unset (sandbox default)",
        purpose: "Process-count limit for each sandboxed FFI inspector phase.",
        subsystem: Subsystem::Ffi,
        class: Class::SecurityTunable,
    },
    EnvVar {
        name: "IPE_FFI_RSS_MB",
        default: "unset (sandbox default)",
        purpose: "RSS memory limit (MB) for each sandboxed FFI inspector phase.",
        subsystem: Subsystem::Ffi,
        class: Class::SecurityTunable,
    },
    EnvVar {
        name: "IPE_FFI_WALL_SECS",
        default: "unset (sandbox default)",
        purpose: "Wall-clock time limit (seconds) for each sandboxed FFI inspector phase.",
        subsystem: Subsystem::Ffi,
        class: Class::SecurityTunable,
    },
    EnvVar {
        name: "IPE_FFI_XC_LOAD",
        default: "unset",
        purpose: "Path to a pre-generated cross-compilation manifest to load instead \
                  of running the inspector. Developer / CI optimisation.",
        subsystem: Subsystem::Ffi,
        class: Class::Tunable,
    },
    EnvVar {
        name: "IPE_FFI_XC_SAVE",
        default: "unset",
        purpose: "Path at which to save the generated cross-compilation manifest after \
                  an inspector run. Developer / CI optimisation.",
        subsystem: Subsystem::Ffi,
        class: Class::Tunable,
    },
    // ── File ──────────────────────────────────────────────────────────────────
    EnvVar {
        name: "IPE_FILE_READ_MAX",
        default: "16777216 (16 MiB)",
        purpose: "Maximum bytes read by `File.read*` in a single call. Prevents OOM \
                  from unexpectedly large files.",
        subsystem: Subsystem::File,
        class: Class::Tunable,
    },
    // ── HTTP client ───────────────────────────────────────────────────────────
    EnvVar {
        name: "IPE_HTTP_BIND",
        default: "unset (loopback in dev, all-interfaces in release)",
        purpose: "Override the host address the HTTP server binds. Takes precedence \
                  over the `Host.bind` setting and the build-profile default. \
                  The conservative loopback default keeps a dev server off the LAN.",
        subsystem: Subsystem::Http,
        class: Class::SecurityTunable,
    },
    EnvVar {
        name: "IPE_HTTP_DENY_PRIVATE",
        default: "unset (auto: on in production, off in dev)",
        purpose: "Set to `1`, `on`, or `true` to block all outbound HTTP / SMTP / \
                  database connections to RFC-1918 private, loopback, and link-local \
                  addresses, closing the SSRF attack surface. In production the guard \
                  is on by default; set to `0` to disable explicitly in dev.",
        subsystem: Subsystem::Http,
        class: Class::SecurityTunable,
    },
    EnvVar {
        name: "IPE_HTTP_MAX_BODY_BYTES",
        default: "33554432 (32 MiB)",
        purpose: "Maximum request-body size (bytes) for outbound `Http.*` calls. \
                  Prevents OOM from unexpectedly large responses.",
        subsystem: Subsystem::Http,
        class: Class::Tunable,
    },
    // ── Observability ─────────────────────────────────────────────────────────
    EnvVar {
        name: "IPE_ENV",
        default: "unset (development)",
        purpose: "Deployment environment marker. Any non-empty value other than `dev`, \
                  `development`, or `local` activates production mode: SSRF guard on, \
                  console requires a token, Secure cookies, no dev banner. Also \
                  accepted as bare `ENV`.",
        subsystem: Subsystem::Observability,
        class: Class::Tunable,
    },
    EnvVar {
        name: "IPE_LOG_FORMAT",
        default: "unset (human-readable)",
        purpose: "Set to `json` to emit structured JSON log lines instead of the \
                  default human-readable format. Recommended for log aggregation \
                  pipelines (Datadog, Loki, etc.).",
        subsystem: Subsystem::Observability,
        class: Class::Tunable,
    },
    EnvVar {
        name: "IPE_LOG_LEVEL",
        default: "unset (info)",
        purpose: "Minimum log level: `debug`, `info`, `warn`, or `error`. Takes \
                  precedence over an installed `Log.level` setting.",
        subsystem: Subsystem::Observability,
        class: Class::Tunable,
    },
    EnvVar {
        name: "IPE_OBSERVABILITY_BUFFER",
        default: "1024",
        purpose: "Bounded queue depth for the parent-push telemetry exporter. Overflow \
                  drops and warns rather than blocking the application.",
        subsystem: Subsystem::Observability,
        class: Class::Tunable,
    },
    EnvVar {
        name: "IPE_OBSERVABILITY_PUSH_INTERVAL_MS",
        default: "2000",
        purpose: "Flush cadence (ms) for telemetry shipped from a sub-app to its \
                  parent's `/_ipe/ingest` endpoint.",
        subsystem: Subsystem::Observability,
        class: Class::Tunable,
    },
    EnvVar {
        name: "IPE_PARENT_URL",
        default: "unset",
        purpose: "Base URL of the parent app to which this sub-app pushes telemetry. \
                  Presence of this variable activates the push exporter.",
        subsystem: Subsystem::Observability,
        class: Class::Tunable,
    },
    EnvVar {
        name: "IPE_RECURSION_LIMIT",
        default: "10000",
        purpose: "Maximum Ipê call-stack depth before a recursion-limit error is \
                  raised. Prevents stack-overflow crashes from unbounded recursion.",
        subsystem: Subsystem::Observability,
        class: Class::Tunable,
    },
    EnvVar {
        name: "IPE_SERVICE_NAME",
        default: "unset (binary name)",
        purpose: "Service name attached as the `service.name` resource attribute on \
                  telemetry records shipped to the Hub or the telemetry SQLite spill.",
        subsystem: Subsystem::Observability,
        class: Class::Tunable,
    },
    EnvVar {
        name: "IPE_TRACE",
        default: "unset (false)",
        purpose: "Set to any non-empty truthy value (`1`, `true`, etc.) to emit \
                  `Trace.span` timings to stderr. Off by default — no noise in \
                  production.",
        subsystem: Subsystem::Observability,
        class: Class::Tunable,
    },
    // ── Process ───────────────────────────────────────────────────────────────
    EnvVar {
        name: "IPE_PROCESS_OUTPUT_MAX",
        default: "16777216 (16 MiB)",
        purpose: "Maximum bytes buffered from a subprocess's stdout or stderr by \
                  `Process.run`. Prevents OOM when a child writes without bound.",
        subsystem: Subsystem::File,
        class: Class::Tunable,
    },
    // ── Runtime ───────────────────────────────────────────────────────────────
    EnvVar {
        name: "IPE_ALLOW_UNSANDBOXED",
        default: "unset (false)",
        purpose: "When `bwrap` confinement is unavailable, set to `1` to allow \
                  `ipe run` to proceed unconfined instead of refusing. Widens the \
                  trust boundary. Never set in CI or production.",
        subsystem: Subsystem::Runtime,
        class: Class::SecurityTunable,
    },
    EnvVar {
        name: "IPE_HOME",
        default: "unset ($XDG_DATA_HOME/ipe, then $HOME/.ipe)",
        purpose: "Root directory for materialised runtime source, config, and cached \
                  binaries. Overrides the XDG / home-directory fallback.",
        subsystem: Subsystem::Runtime,
        class: Class::Tunable,
    },
    EnvVar {
        name: "IPE_RUNTIME_DIR",
        default: "unset (embedded / in-repo)",
        purpose: "Explicit path to the runtime crate source directory. Overrides the \
                  embedded fallback. Used in tests and in-repo development.",
        subsystem: Subsystem::Runtime,
        class: Class::Tunable,
    },
    EnvVar {
        name: "IPE_RUNTIME_VENDORED",
        default: "unset (false)",
        purpose: "Set to `1` to declare that the runtime is vendored (already present \
                  on disk) and skip materialization. Used during packaging.",
        subsystem: Subsystem::Runtime,
        class: Class::Tunable,
    },
    // ── Web server ────────────────────────────────────────────────────────────
    EnvVar {
        name: "IPE_AUTH_MAX_LIFETIME",
        default: "28800 (8 h)",
        purpose: "Absolute lifetime cap (seconds) for a signed session token. A stolen \
                  but unrevoked token is worthless after this deadline. Takes \
                  precedence over `Web.authMaxLifetime`.",
        subsystem: Subsystem::Web,
        class: Class::Tunable,
    },
    EnvVar {
        name: "IPE_AUTH_REVOCATION",
        default: "unset (Off)",
        purpose: "Session-token revocation mode. Set to `store` or `1` to enable the \
                  in-process revocation store, which checks each request against a list \
                  of revoked token IDs. `off` or `0` disables; default is Off \
                  (zero overhead).",
        subsystem: Subsystem::Web,
        class: Class::Tunable,
    },
    EnvVar {
        name: "IPE_AUTH_SLIDE_WINDOW",
        default: "1800 (30 min)",
        purpose: "Rolling re-issue window (seconds) for a signed session token. A \
                  request within this window of expiry re-issues a fresh token, \
                  keeping an active session alive without a full login. Clamped so \
                  `slide_window < max_lifetime`. Takes precedence over \
                  `Web.authSlideWindow`.",
        subsystem: Subsystem::Web,
        class: Class::Tunable,
    },
    EnvVar {
        name: "IPE_AUTH_TOKEN_SECRET",
        default: "unset",
        purpose: "HMAC signing secret for session tokens. Must be at least 32 bytes. \
                  Rotate with care — outstanding tokens signed with the old secret \
                  become invalid. Provide via your secret manager; never commit.",
        subsystem: Subsystem::Web,
        class: Class::Secret,
    },
    EnvVar {
        name: "IPE_CSRF",
        default: "unset (on)",
        purpose: "Set to `off`, `0`, or `false` to disable CSRF protection. \
                  Disabling widens the trust boundary — only safe on loopback in \
                  automated tests.",
        subsystem: Subsystem::Web,
        class: Class::SecurityTunable,
    },
    EnvVar {
        name: "IPE_REVOCATION_CAPACITY",
        default: "1048576 (2^20)",
        purpose: "Maximum number of entries in the per-process token revocation store. \
                  Each entry is roughly 64 bytes; the default cap holds ~64 MB. Raise \
                  for very high user volumes with token revocation enabled.",
        subsystem: Subsystem::Web,
        class: Class::Tunable,
    },
    EnvVar {
        name: "IPE_TRUSTED_PROXY",
        default: "unset (false)",
        purpose: "Set to any truthy value to trust `X-Forwarded-For` / \
                  `X-Forwarded-Proto` headers for remote-address and TLS detection. \
                  Enable only when a trusted reverse proxy sits in front of this \
                  process; leaving unset prevents clients from forging these headers.",
        subsystem: Subsystem::Web,
        class: Class::SecurityTunable,
    },
    EnvVar {
        name: "IPE_WEB_BANNER",
        default: "unset (on in dev)",
        purpose: "Set to `off`, `0`, or `false` to disable the reconnection-status \
                  banner in the browser client. Deprecated alias: `IPE_LIVE_BANNER`.",
        subsystem: Subsystem::Web,
        class: Class::Tunable,
    },
    EnvVar {
        name: "IPE_WEB_BASE_PATH",
        default: "unset (root-mounted)",
        purpose: "Sub-app mount prefix, e.g. `/billing`. All session-cookie, \
                  CSRF-cookie, and asset paths are scoped to this prefix. Set \
                  automatically when mounting a sub-app. Deprecated alias: \
                  `IPE_LIVE_BASE_PATH`.",
        subsystem: Subsystem::Web,
        class: Class::Tunable,
    },
    EnvVar {
        name: "IPE_WEB_CSRF_ORIGIN_CHECK",
        default: "unset (off)",
        purpose: "Set to `on` to enforce strict `Origin`-header cross-origin checking \
                  on top of the double-submit CSRF token. Deprecated alias: \
                  `IPE_LIVE_CSRF_ORIGIN_CHECK`.",
        subsystem: Subsystem::Web,
        class: Class::SecurityTunable,
    },
    EnvVar {
        name: "IPE_WEB_FRAME_ANCESTORS",
        default: "unset (no embedding allowed)",
        purpose: "Space-separated `Content-Security-Policy: frame-ancestors` allow-list, \
                  e.g. `https://app.example.com`. Enables embedding this app in a \
                  third-party iframe; also sets `SameSite=None; Secure` on session \
                  cookies. Deprecated alias: `IPE_LIVE_FRAME_ANCESTORS`.",
        subsystem: Subsystem::Web,
        class: Class::SecurityTunable,
    },
    EnvVar {
        name: "IPE_WEB_HEARTBEAT_TTL_MS",
        default: "35000",
        purpose: "SSE heartbeat interval (ms) the browser uses to detect a stale \
                  connection. Deprecated alias: `IPE_LIVE_HEARTBEAT_TTL_MS`.",
        subsystem: Subsystem::Web,
        class: Class::Tunable,
    },
    EnvVar {
        name: "IPE_WEB_HELLO_TIMEOUT_MS",
        default: "8000",
        purpose: "Timeout (ms) for the initial SSE hello handshake. The browser \
                  closes and retries if this deadline passes. Deprecated alias: \
                  `IPE_LIVE_HELLO_TIMEOUT_MS`.",
        subsystem: Subsystem::Web,
        class: Class::Tunable,
    },
    EnvVar {
        name: "IPE_WEB_MAX_BODY_BYTES",
        default: "33554432 (32 MiB)",
        purpose: "Maximum inbound request-body size (bytes) for `/_ipe/event`. Raise \
                  for large file uploads; lower to tighten the DoS floor. Deprecated \
                  alias: `IPE_LIVE_MAX_BODY_BYTES`.",
        subsystem: Subsystem::Web,
        class: Class::Tunable,
    },
    EnvVar {
        name: "IPE_WEB_MAX_SESSIONS",
        default: "50000",
        purpose: "Maximum concurrent web sessions before new connections are rejected. \
                  Prevents unbounded memory growth under a session-creation flood. \
                  Deprecated alias: `IPE_LIVE_MAX_SESSIONS`.",
        subsystem: Subsystem::Web,
        class: Class::Tunable,
    },
    EnvVar {
        name: "IPE_WEB_PORT",
        default: "8000",
        purpose: "TCP port the web server listens on. Deprecated alias: `IPE_LIVE_PORT`.",
        subsystem: Subsystem::Web,
        class: Class::Tunable,
    },
    EnvVar {
        name: "IPE_WEB_QUEUE_MAX",
        default: "50",
        purpose: "Maximum queued events per session before back-pressure is applied. \
                  Deprecated alias: `IPE_LIVE_QUEUE_MAX`.",
        subsystem: Subsystem::Web,
        class: Class::Tunable,
    },
    EnvVar {
        name: "IPE_WEB_RETRY_BASE_MS",
        default: "500",
        purpose: "Initial retry interval (ms) for client reconnection after a \
                  disconnect. Deprecated alias: `IPE_LIVE_RETRY_BASE_MS`.",
        subsystem: Subsystem::Web,
        class: Class::Tunable,
    },
    EnvVar {
        name: "IPE_WEB_RETRY_FAST_MS",
        default: "200",
        purpose: "Fast-retry interval (ms) used during the fast-retry window after a \
                  hot-reload disconnect.",
        subsystem: Subsystem::Web,
        class: Class::Tunable,
    },
    EnvVar {
        name: "IPE_WEB_RETRY_FAST_WINDOW_MS",
        default: "3000",
        purpose: "Duration (ms) of the fast-retry window after a disconnect. Set to \
                  `8000` automatically by `ipe watch` to accommodate server restart \
                  time. Deprecated alias: `IPE_LIVE_RETRY_FAST_WINDOW_MS`.",
        subsystem: Subsystem::Web,
        class: Class::Tunable,
    },
    EnvVar {
        name: "IPE_WEB_RETRY_MAX_ATTEMPTS",
        default: "10",
        purpose: "Maximum reconnection attempts before the client stops retrying. \
                  Deprecated alias: `IPE_LIVE_RETRY_MAX_ATTEMPTS`.",
        subsystem: Subsystem::Web,
        class: Class::Tunable,
    },
    EnvVar {
        name: "IPE_WEB_RETRY_MAX_MS",
        default: "16000",
        purpose: "Maximum retry interval (ms) — the exponential back-off ceiling. \
                  Deprecated alias: `IPE_LIVE_RETRY_MAX_MS`.",
        subsystem: Subsystem::Web,
        class: Class::Tunable,
    },
    EnvVar {
        name: "IPE_WEB_SHUTDOWN_GRACE_MS",
        default: "1500",
        purpose: "Grace period (ms) between receiving SIGTERM and closing active \
                  connections. Allows in-flight requests to complete. Set to `0` for \
                  immediate shutdown. Deprecated alias: `IPE_LIVE_SHUTDOWN_GRACE_MS`.",
        subsystem: Subsystem::Web,
        class: Class::Tunable,
    },
    EnvVar {
        name: "IPE_WEB_SSE_BUFFER",
        default: "16",
        purpose: "SSE channel buffer capacity per session (clamped 1–1024). A full \
                  buffer applies TCP backpressure rather than dropping events. \
                  Deprecated alias: `IPE_LIVE_SSE_BUFFER`.",
        subsystem: Subsystem::Web,
        class: Class::Tunable,
    },
    EnvVar {
        name: "IPE_WEB_STATIC_DIR",
        default: "unset",
        purpose: "Directory served at `/static/*`. Populated from `package.ipe [web] \
                  static`. Path traversal is blocked by construction. Deprecated alias: \
                  `IPE_LIVE_STATIC_DIR`.",
        subsystem: Subsystem::Web,
        class: Class::Tunable,
    },
    EnvVar {
        name: "IPE_WEB_STORE",
        default: "memory",
        purpose: "Session-store backend for the web server: `memory` (per-process, \
                  lost on restart) or `sqlite` (persisted to `IPE_WEB_STORE_PATH`). \
                  `ipe watch` selects `sqlite` so a rebuild preserves live sessions.",
        subsystem: Subsystem::Web,
        class: Class::Tunable,
    },
    EnvVar {
        name: "IPE_WEB_STORE_PATH",
        default: "unset (temp file)",
        purpose: "Filesystem path for the `sqlite` session store. Ignored when \
                  `IPE_WEB_STORE` is `memory`. Unset uses a per-process temporary file.",
        subsystem: Subsystem::Web,
        class: Class::Tunable,
    },
    EnvVar {
        name: "IPE_WEB_TTL",
        default: "1800 (30 min)",
        purpose: "Session idle TTL. Accepts seconds (`1800`) or duration strings \
                  (`30m`, `1h`). Takes precedence over `Web.sessionTtl`. Deprecated \
                  alias: `IPE_LIVE_TTL`.",
        subsystem: Subsystem::Web,
        class: Class::Tunable,
    },
    // ── WebSocket ─────────────────────────────────────────────────────────────
    EnvVar {
        name: "IPE_WS_HEARTBEAT",
        default: "30",
        purpose: "WebSocket ping interval (seconds). A peer that does not respond \
                  within two intervals is considered dead and disconnected.",
        subsystem: Subsystem::Ws,
        class: Class::Tunable,
    },
    EnvVar {
        name: "IPE_WS_MAX_MESSAGE_BYTES",
        default: "1048576 (1 MiB)",
        purpose: "Maximum WebSocket message size (bytes) for both client and server \
                  connections. Messages larger than this limit are rejected.",
        subsystem: Subsystem::Ws,
        class: Class::Tunable,
    },
    EnvVar {
        name: "IPE_WS_SEND_BUFFER",
        default: "256",
        purpose: "Per-connection outbound frame buffer depth. A full buffer applies \
                  backpressure (the send kernel returns `Err`) rather than dropping \
                  frames.",
        subsystem: Subsystem::Ws,
        class: Class::Tunable,
    },
];

/// Set of variable names that are intentionally excluded from the drift gate
/// because they are test-only, build-time baked, or deprecated aliases.
///
/// The drift gate checks that every `IPE_*` string literal read at runtime
/// appears in `ENV_VARS` OR in this exclusion set.
pub static EXCLUDED_NAMES: &[&str] = &[
    // Test harness variables — not operator-facing.
    "IPE_ALLOWED_E2E", // Windows jail e2e test sentinel
    "IPE_ANYTHING",    // used only in a denylist assertion string
    "IPE_BLESS",
    "IPE_CAPABILITY_FLOOR", // a linker-retained static symbol, not an env var
    "IPE_DEBUG_TODO_SUBPROCESS",
    "IPE_E2E",        // CI gate for enabling e2e test suites
    "IPE_E2E_SECRET", // macOS jail e2e test sentinel
    "IPE_E2E_STATIC", // CI gate for static-binary e2e tests
    "IPE_HTTP_TEST_URL",
    "IPE_LOAD_ENV_PROBE_VAR",
    "IPE_ORACLE_SHARED_TARGET",
    "IPE_RUN_WITH_TEST_VAR",
    "IPE_SECRET_E2E", // Windows jail e2e test sentinel
    "IPE_TEST_BOOL_BAD",
    "IPE_TEST_BOOL_F",
    "IPE_TEST_BOOL_T",
    "IPE_TEST_BOOL_UNSET",
    "IPE_TEST_GETENV_PRESENT",
    "IPE_TEST_GETENV_UNSET_XYZ_",
    "IPE_TEST_GETENV_UNSET_XYZ_42", // variant with numeric suffix in proptest
    "IPE_TEST_INT_BAD",
    "IPE_TEST_INT_OK",
    "IPE_TEST_INT_UNSET",
    "IPE_TEST_PG_URL",
    "IPE_TEST_REDIS_URL",
    // Deprecated IPE_LIVE_* aliases — documented in the canonical IPE_WEB_* entry.
    "IPE_LIVE_BANNER",
    "IPE_LIVE_BASE_PATH",
    "IPE_LIVE_CSRF_ORIGIN_CHECK",
    "IPE_LIVE_FRAME_ANCESTORS",
    "IPE_LIVE_HEARTBEAT_TTL_MS",
    "IPE_LIVE_HELLO_TIMEOUT_MS",
    "IPE_LIVE_MAX_BODY_BYTES",
    "IPE_LIVE_MAX_SESSIONS",
    "IPE_LIVE_PORT",
    "IPE_LIVE_QUEUE_MAX",
    "IPE_LIVE_RETRY_BASE_MS",
    "IPE_LIVE_RETRY_FAST_WINDOW_MS",
    "IPE_LIVE_RETRY_MAX_ATTEMPTS",
    "IPE_LIVE_RETRY_MAX_MS",
    "IPE_LIVE_SHUTDOWN_GRACE_MS",
    "IPE_LIVE_SSE_BUFFER",
    "IPE_LIVE_STATIC_DIR",
    "IPE_LIVE_STORE",
    "IPE_LIVE_STORE_PATH",
    "IPE_LIVE_TTL",
    // Test-only server port var — set by the watch driver in tests.
    "IPE_SERVER_PORT",
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn env_vars_sorted_within_subsystem() {
        // Collect subsystem-buckets in the order they appear.
        let mut last: Option<(&str, Subsystem)> = None;
        for v in ENV_VARS {
            if let Some((prev_name, prev_sub)) = last
                && v.subsystem == prev_sub
            {
                assert!(
                    v.name >= prev_name,
                    "ENV_VARS: within subsystem {:?}, '{}' must come after '{}' (alphabetical)",
                    v.subsystem,
                    v.name,
                    prev_name,
                );
            }
            last = Some((v.name, v.subsystem));
        }
    }

    #[test]
    fn env_var_names_unique() {
        let mut seen = std::collections::HashSet::new();
        for v in ENV_VARS {
            assert!(seen.insert(v.name), "ENV_VARS: duplicate name '{}'", v.name);
        }
    }

    #[test]
    fn excluded_names_no_overlap_with_registry() {
        let registered: std::collections::HashSet<&str> = ENV_VARS.iter().map(|v| v.name).collect();
        for name in EXCLUDED_NAMES {
            assert!(
                !registered.contains(name),
                "EXCLUDED_NAMES: '{name}' also appears in ENV_VARS — remove it from one",
            );
        }
    }
}
