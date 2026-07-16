//! The typed IR node definitions (M0 subset). Widened in later milestones; for
//! M0 the surface is deliberately narrow so that every constructible value is a
//! well-formed program fragment.

use std::collections::{BTreeMap, BTreeSet};

use sky_diagnostics::{DResult, Diagnostic};
use sky_intern::Symbol;

/// A dotted module path, e.g. `Main` or `Sky.Core.Io`, as interned segments in
/// source order.
#[derive(
    Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Debug, serde::Serialize, serde::Deserialize,
)]
pub struct ModPath(pub Vec<Symbol>);

/// A function identifier, unique within a [`Program`].
#[derive(
    Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug, serde::Serialize, serde::Deserialize,
)]
pub struct FuncId(pub u32);

impl FuncId {
    #[must_use]
    pub const fn from_raw(n: u32) -> Self {
        Self(n)
    }

    #[must_use]
    pub const fn as_raw(self) -> u32 {
        self.0
    }
}

/// A whole compiled program: an ordered list of modules.
//
// `Eq` is not derived: a module's functions hold [`Expr`] bodies that may carry
// a float literal (only `PartialEq`).
#[derive(Clone, PartialEq, Debug, serde::Serialize, serde::Deserialize)]
pub struct Program {
    pub modules: Vec<Module>,
}

/// A single module: its declared types and functions, plus an optional entry
/// point (the `main` function, when this module carries it).
//
// `Eq` is not derived: `funcs` hold [`Expr`] bodies that may carry a float
// literal (only `PartialEq`).
#[derive(Clone, PartialEq, Debug, serde::Serialize, serde::Deserialize)]
#[allow(clippy::struct_excessive_bools)]
pub struct Module {
    pub name: ModPath,
    pub types: Vec<TypeDef>,
    pub funcs: Vec<Func>,
    pub entry: Option<FuncId>,
    /// Every CLOSED record shape the module's expressions construct or read,
    /// each an [`IrType::Record`]. The lowerer surfaces these (it alone has the
    /// solved types) so the backend can synthesise one Rust struct per shape —
    /// record literals live inside function bodies, where the type does not
    /// otherwise appear in a signature. Non-record entries are ignored by the
    /// backend, so the field stays robust to a stray shape.
    pub records: Vec<IrType>,
    /// `true` when the lowerer detected at least one TEA kernel call
    /// (`Cmd.none / batch / perform`, `Sub.none / batch / every`, `Time.every`)
    /// in the module's function bodies.
    ///
    /// Set by `sky_lower::lower::Lowerer::run` when any call site resolves to a
    /// `KernelFn::is_tea()` variant.  The backend reads this flag to decide
    /// whether to append `pub mod tea; pub use tea::*;` to the emitted
    /// `sky_runtime/mod.rs` and to add `SkyCmd` / `SkySub` type aliases.
    pub uses_tea: bool,
    /// `true` when the lowerer detected at least one Sky.Http.Server kernel call
    /// (`Server.get/post/put/delete/any/api/static/listen`, response builders,
    /// extractors, cookie helpers, middleware, `RateLimit.allow`) in the module's
    /// function bodies.
    ///
    /// Set by `sky_lower::lower::Lowerer::run` when any call site resolves to a
    /// `KernelFn::is_server()` variant.  The backend reads this flag to decide
    /// whether to inject the `server` feature in the emitted `Cargo.toml` and to
    /// append `pub mod server; pub use server::*; pub mod server_stream; pub use
    /// server_stream::*;` to the emitted `sky_runtime/mod.rs`.
    pub uses_server: bool,
    /// `true` when the lowerer detected at least one `Std.Ui` / `Std.Html`
    /// kernel call (`Ui.layout`, `Ui.layoutWith`, `Html.render`, etc.) in the
    /// module's function bodies.
    ///
    /// Set by `sky_lower` when any call site resolves to a
    /// `KernelFn::is_ui()` variant.  The backend reads this flag to decide
    /// whether to add `pub mod ui;` to the emitted `sky_runtime/mod.rs`.
    pub uses_ui: bool,
    /// `true` when the lowerer detected at least one `Std.Live` / `Sky.Live`
    /// kernel call (`Live.app`, `Live.appRouted`, `Live.route`, etc.) in the
    /// module's function bodies.
    ///
    /// Set by `sky_lower` when any call site resolves to a
    /// `KernelFn::is_live()` variant.
    pub uses_live: bool,
    /// `true` when the lowerer detected at least one `Std.Tui` / `Sky.Tui`
    /// kernel call (`Tui.app`, `Tui.program`) in the module's function bodies.
    ///
    /// Set by `sky_lower` when any call site resolves to a
    /// `KernelFn::is_tui()` variant.
    pub uses_tui: bool,
    /// `true` when the lowerer detected at least one `Std.Webview`
    /// kernel call (`Webview.app`) in the module's function bodies.
    ///
    /// Set by `sky_lower` when any call site resolves to a
    /// `KernelFn::is_webview()` variant.  Implies `uses_live` for the
    /// runtime dependency chain (webview pulls live transitively).
    pub uses_webview: bool,
    /// `true` when the lowerer detected at least one `Sky.Core.CssSafety`
    /// leaf security kernel (`CssSafety.safeValue` / `safePropName` /
    /// `safeSelector` / `stripStyleClose` — the `Std.Css` backing, #47) in the
    /// module's function bodies.
    ///
    /// Set by `sky_lower` when any call site resolves to a
    /// `KernelFn::is_css()` variant.  The backend reads this flag to decide
    /// whether to declare `css_safety` / `css` (`pub use css::*`) in the emitted
    /// `sky_runtime/mod.rs` even when `uses_ui` is `false` — a pure `Std.Css`
    /// program uses the css kernels without any `Std.Ui` / `Std.Html` render
    /// kernel, so the UI append would never fire and the bare kernel names would
    /// be out of scope (E0425).
    pub uses_css: bool,
    /// `true` when the lowerer detected at least one `Std.Auth` kernel call
    /// (`Auth.hashPassword`, `Auth.verifyPassword`, `Auth.signToken`,
    /// `Auth.verifyToken`, `Auth.register`, `Auth.login`, `Auth.setRole`, etc.)
    /// in the module's function bodies.
    ///
    /// Set by `sky_lower` when any call site resolves to a
    /// `KernelFn::is_auth()` variant.  The backend reads this flag to decide
    /// whether to append `pub mod auth; pub use auth::*;` to the emitted
    /// `sky_runtime/mod.rs`.
    pub uses_auth: bool,
}

/// A user-declared type. The IR models user types as enums (Sky's `type`
/// declarations); a nullary-only enum is the M0 case, a payload-carrying and/or
/// generic enum the M3a case.
#[derive(Clone, PartialEq, Eq, Debug, serde::Serialize, serde::Deserialize)]
pub enum TypeDef {
    Enum(EnumDef),
}

/// An enum (algebraic data type) declaration.
///
/// A variant may carry payload fields and the type may be generic over a
/// list of type parameters (`type Maybe a = Just a | Nothing`). A nullary-only,
/// non-generic enum (`type Msg = Increment | Decrement`) has every variant's
/// `fields` empty and an empty `type_params` — that path stays byte-identical to
/// the M0 backend output.
#[derive(Clone, PartialEq, Eq, Debug, serde::Serialize, serde::Deserialize)]
pub struct EnumDef {
    pub name: Symbol,
    /// The module that DEFINES this type (its *home*), not the entry module the
    /// linker merged it into. Two modules may each declare a `type Color`; they
    /// intern the same bare `name` [`Symbol`], so `(home, name)` — not `name`
    /// alone — is the type's nominal identity. The backend derives the emitted
    /// Rust enum name from `home` (`["Std","Palette"] + Shade` → `StdPaletteShade`,
    /// `["Lib"] + Color` → `LibColor`, `["Main"] + Msg` → `MainMsg`), so a
    /// single-module program (home == entry) is byte-identical to the pre-#100
    /// output while two same-short-named types from different modules mangle to
    /// distinct Rust enums. Empty for backend-unit-test IR built by hand.
    pub home: ModPath,
    /// The type variables this enum quantifies, in declaration order. Each is a
    /// Sky type-variable [`Symbol`] that appears as an [`IrType::Generic`] in a
    /// variant's field types. A non-generic enum has an empty list.
    ///
    /// The order is load-bearing: the backend derives each parameter's Rust
    /// generic name (`T1`, `T2`, …) from its *position* here — exactly as for
    /// [`Func::type_params`] — so the emitted `enum Name<T1, T2>` agrees with
    /// every field type and use-site instantiation regardless of source naming.
    pub type_params: Vec<Symbol>,
    pub variants: Vec<Variant>,
}

/// One constructor of an [`EnumDef`]: its name and its ordered payload field
/// types.
///
/// A nullary constructor (`Increment`, `Nothing`) has an empty `fields`. A
/// payload constructor (`Just a`, `Rect Float Float`, `Node Tree Int Tree`)
/// lists one [`IrType`] per positional field, in source order. A field whose
/// type is the enum being declared (direct self-recursion) is rendered boxed by
/// the backend so the Rust enum stays finite-sized; the IR carries the bare
/// recursive type and leaves the boxing to emission.
#[derive(Clone, PartialEq, Eq, Debug, serde::Serialize, serde::Deserialize)]
pub struct Variant {
    pub name: Symbol,
    pub fields: Vec<IrType>,
}

/// The set of Rust trait bounds a generic type parameter carries, held as a
/// compact bit set.
///
/// An unconstrained type variable — one the body only passes through (`id x =
/// x`) — has the empty set [`BoundSet::UNBOUNDED`], which the backend emits as a
/// bare generic (`T1`), byte-identical to a structurally-parametric M2a
/// function. A variable the body *constrains* by applying an operation to it
/// carries the matching bounds, so the emitted generic is `T1: <bounds>`.
///
/// Each flag maps a Sky super-type capability to the Rust standard-library
/// trait that realises it, with no new runtime trait:
///
/// * `add` / `sub` / `mul` realise Sky's **Number** super-type (`Int` or
///   `Float`). They are split per arithmetic operator because Sky's
///   `Basics.add` / `sub` / `mul` already lower to Rust's `+` / `-` / `*`, and
///   each operator demands exactly its own `::core::ops` trait — a body that
///   only adds needs only `Add`, so the bound stays minimal rather than
///   over-constraining a caller.
/// * `ord` realises Sky's **Comparable** super-type (`Int` / `Float` / `Char` /
///   `String` / `Bool`) for the ordering comparisons `<` `>` `<=` `>=`, mapping
///   to Rust's `PartialOrd`.
/// * `eq` realises Sky's **Equatable** super-type (every non-function type) for
///   the equality comparisons `==` `/=`, mapping to Rust's `PartialEq`. Unlike
///   `ord` / the arithmetic traits it adds no `copy`: `PartialEq::eq` takes
///   `&self`, so an equated value is borrowed, never moved.
/// * `copy` is added when a bound value is used more than once and is a
///   bit-copyable primitive (every `Number` / `Comparable` primitive except
///   `String`), so the generated body can reuse it without a move error.
/// * `clone` is the non-`Copy` counterpart — added when a reused value's type
///   may be `String`, where `Clone` is the available duplication trait.
/// * `ord_total` realises a `Set` element's Rust requirement: `BTreeSet<A>`
///   needs `A : Ord` (the TOTAL order), which is strictly stronger than the
///   `ord` flag's `PartialOrd`. A generic `a -> Set a` carries `ord_total`.
/// * `hash` realises a `Dict` key's Rust requirement: `HashMap<K, V>` needs
///   `K : Hash + Eq`. Paired with `ord_total` on a Dict key (so the
///   determinism-sorted `Dict.keys` / `Dict.toList` also compile, and `Eq`
///   arrives as `Ord`'s supertrait) a generic `a -> Dict a v` carries
///   `hash | ord_total | clone`.
///
/// The flags are independent and compose: a Comparable-and-reused variable
/// carries `ord | copy`; a numeric-add-and-reused variable carries `add | copy`.
/// The `with_*` builders set a flag and return the updated set, so a bound set
/// is assembled fluently (`BoundSet::UNBOUNDED.with_add().with_copy()`); the
/// `has_*` predicates read one flag back.
#[derive(
    Clone, Copy, PartialEq, Eq, Debug, Default, Hash, serde::Serialize, serde::Deserialize,
)]
pub struct BoundSet(u16);

impl BoundSet {
    const ADD: u16 = 1 << 0;
    const SUB: u16 = 1 << 1;
    const MUL: u16 = 1 << 2;
    const ORD: u16 = 1 << 3;
    const COPY: u16 = 1 << 4;
    const CLONE: u16 = 1 << 5;
    const EQ: u16 = 1 << 6;
    const ORD_TOTAL: u16 = 1 << 7;
    const HASH: u16 = 1 << 8;
    const SHOW: u16 = 1 << 9;
    /// The SQL-bind-parameter bound: realises the type checker's
    /// `TyBounds::sql_param` obligation as Rust `Into<sky_runtime::db::SqlParam>`
    /// — a generic wrapper around `Db.exec` / `Db.query` / `Db.queryDecode`
    /// (`Database.exec label sql args` in `examples/17-skymon`'s `Std.Db`
    /// access layer) needs this bound on its own emitted type parameter so its
    /// body's `SqlParam::from`-style projection type-checks for the CALLER's
    /// concrete element type, not just the one instantiation the function
    /// happened to be lowered against.
    const SQL_PARAM: u16 = 1 << 10;
    /// The `SkyRow` bound: a wildcard `any` generic that flows into a
    /// `Db.get*` field accessor (`Db.getString`/`getInt`/`getBool`/`getField`)
    /// gains `sky_runtime::db::SkyRow` so the generic body type-checks and
    /// monomorphises per call site against the row's real shape (a query result
    /// `Dict String String`, a pub/sub `Dict` payload, or the typed `LiveReq` an
    /// `init` handler receives). The runtime's `db_get_*` helpers are generic
    /// over `R: SkyRow`; without this bound the emitted body's `db_get_string(_,
    /// &payload)` call cannot prove `payload: SkyRow` (E0277). Added ONLY to the
    /// wildcard `any` variable and ONLY when the body actually calls a `db_get_*`
    /// — no blast radius on genuine named type variables (`a`, `msg`).
    const SKY_ROW: u16 = 1 << 11;
    /// The `std::fmt::Display` bound: a generic type-param that flows into
    /// a `Basics.toString` kernel application (runtime `basics_to_string<T:
    /// std::fmt::Display>`) gains `std::fmt::Display` so the emitted body's
    /// `basics_to_string(x)` call type-checks and monomorphises per call site.
    /// Without it the enclosing generic carries only `<T{n}: Clone>` and the
    /// body's `basics_to_string(x)` cannot prove `T{n}: Display` (E0277).
    ///
    /// This is the SAME structural machinery as [`Self::SKY_ROW`] — a
    /// kernel applied, alias-transparently, to a Var/CloneVar reference to a
    /// param whose type is `Generic(tv)` obliges the kernel's Rust bound on that
    /// tv — generalised to a kernel→bound map. Unlike `SKY_ROW`, the bound lands
    /// on WILDCARD `any` AND genuine named tvars alike: `toString` is legitimate
    /// on a polymorphic value, and the resulting `T: Display` is satisfiable by
    /// every scalar caller (Int/Float/Bool/String) — the overwhelming common
    /// case, matching Go exactly. A composite (record/ADT) argument has no
    /// `Display` impl and fails at COMPILE time (E0277 at the caller's
    /// instantiation), never at runtime — the runtime's deliberate policy (see
    /// `runtime/src/sky_runtime/basics.rs:174-182`). NOT a new SEAL violation:
    /// `skyc` already exit-0'd on composite-`toString` before this bound existed
    /// (the monomorphic `basics_to_string` call already required `Display`); the
    /// bound only relocates that pre-existing cargo error from the callee body
    /// to the caller's instantiation, never introducing one where there was
    /// none.
    const DISPLAY: u16 = 1 << 12;
    /// The `'static` lifetime bound: a generic type-param that flows,
    /// INSIDE the function body, into a value boxed as a boxed `dyn Fn` trait
    /// object (`Box<dyn Fn(..) -> .. + Send + 'static>`, or the `Arc` +Sync
    /// variant) — a callback (`FuncValue` / lambda) passed to a higher-order
    /// kernel like `List.map` — whose own type still mentions that type-param.
    /// Coercing a concrete-but-`tv`-generic function item to a `+ 'static` trait
    /// object requires `tv: 'static`, so the emitted generic must carry it.
    /// Without it the enclosing generic is only `<T{n}: Clone>` and the box
    /// coercion fails E0310 ("the parameter type may not live long enough") —
    /// well-typed to `skyc`, a `cargo build` break (a SEAL violation).
    ///
    /// This is a LIFETIME bound, not a trait: it renders as the leading
    /// `'static` in the bound list (`T{n}: 'static + Clone`), where Rust
    /// requires lifetime bounds to precede trait bounds. Unlike the trait
    /// obligations above it is not sourced from a kernel→bound map keyed on a
    /// PARAM value binder — the type-param appears in the boxed callback's TYPE,
    /// not as the accessed value — so it has its own structural walk
    /// (`body_boxes_generic_callback` in `sky_lower`). It lands on wildcard `any`
    /// AND named tvars alike: every concrete Sky type the caller substitutes is
    /// `'static` (emitted values never borrow), so `T: 'static` is satisfied by
    /// every real instantiation — no caller-side failure, matching the reference
    /// (Go boxes with no lifetime concern). NOT a new SEAL violation: it only
    /// relocates the pre-existing E0310 from the callee body to a bound that
    /// makes acceptance-by-`skyc` prove the box coercion type-checks.
    const STATIC: u16 = 1 << 13;

    /// The empty bound set: an unconstrained, structurally-parametric variable.
    pub const UNBOUNDED: Self = Self(0);

    /// This set with the `Ord` (total order, `BTreeSet` element) bound. Strictly
    /// stronger than [`Self::with_ord`]'s `PartialOrd`.
    #[must_use]
    pub const fn with_ord_total(self) -> Self {
        Self(self.0 | Self::ORD_TOTAL)
    }

    /// This set with the `::core::hash::Hash` (`HashMap` key) bound.
    #[must_use]
    pub const fn with_hash(self) -> Self {
        Self(self.0 | Self::HASH)
    }

    /// Whether the `Ord` (total-order) bound is set.
    #[must_use]
    pub const fn has_ord_total(self) -> bool {
        self.0 & Self::ORD_TOTAL != 0
    }

    /// Whether the `Hash` bound is set.
    #[must_use]
    pub const fn has_hash(self) -> bool {
        self.0 & Self::HASH != 0
    }

    /// Whether this set carries no bound at all — the variable is a true
    /// parametric pass-through and emits as a bare generic.
    #[must_use]
    pub const fn is_unbounded(self) -> bool {
        self.0 == 0
    }

    /// This set with the `::core::ops::Add<Output = Self>` (Number `+`) bound.
    #[must_use]
    pub const fn with_add(self) -> Self {
        Self(self.0 | Self::ADD)
    }

    /// This set with the `::core::ops::Sub<Output = Self>` (Number `-`) bound.
    #[must_use]
    pub const fn with_sub(self) -> Self {
        Self(self.0 | Self::SUB)
    }

    /// This set with the `::core::ops::Mul<Output = Self>` (Number `*`) bound.
    #[must_use]
    pub const fn with_mul(self) -> Self {
        Self(self.0 | Self::MUL)
    }

    /// This set with the `PartialOrd` (Comparable ordering) bound.
    #[must_use]
    pub const fn with_ord(self) -> Self {
        Self(self.0 | Self::ORD)
    }

    /// This set with the `PartialEq` (Equatable equality) bound.
    #[must_use]
    pub const fn with_eq(self) -> Self {
        Self(self.0 | Self::EQ)
    }

    /// This set with the `SkyStringify` (Sky `toString` / `Log.*With`) bound.
    #[must_use]
    pub const fn with_show(self) -> Self {
        Self(self.0 | Self::SHOW)
    }

    /// This set with the `Copy` (bit-copyable reuse) bound.
    #[must_use]
    pub const fn with_copy(self) -> Self {
        Self(self.0 | Self::COPY)
    }

    /// This set with the `Clone` (non-`Copy` reuse) bound.
    #[must_use]
    pub const fn with_clone(self) -> Self {
        Self(self.0 | Self::CLONE)
    }

    /// This set with the SQL-bind-parameter (`Into<SqlParam>`) bound — see
    /// [`Self::SQL_PARAM`].
    #[must_use]
    pub const fn with_sql_param(self) -> Self {
        Self(self.0 | Self::SQL_PARAM)
    }

    /// This set with the `SkyRow` (Db field-accessor row) bound — see
    /// [`Self::SKY_ROW`].
    #[must_use]
    pub const fn with_sky_row(self) -> Self {
        Self(self.0 | Self::SKY_ROW)
    }

    /// This set with the `std::fmt::Display` (`Basics.toString`) bound — see
    /// [`Self::DISPLAY`].
    #[must_use]
    pub const fn with_display(self) -> Self {
        Self(self.0 | Self::DISPLAY)
    }

    /// This set with the `'static` (boxed-callback trait-object) lifetime bound —
    /// see [`Self::STATIC`].
    #[must_use]
    pub const fn with_static(self) -> Self {
        Self(self.0 | Self::STATIC)
    }

    /// Whether the `Add` bound is set.
    #[must_use]
    pub const fn has_add(self) -> bool {
        self.0 & Self::ADD != 0
    }

    /// Whether the `Sub` bound is set.
    #[must_use]
    pub const fn has_sub(self) -> bool {
        self.0 & Self::SUB != 0
    }

    /// Whether the `Mul` bound is set.
    #[must_use]
    pub const fn has_mul(self) -> bool {
        self.0 & Self::MUL != 0
    }

    /// Whether the `PartialOrd` bound is set.
    #[must_use]
    pub const fn has_ord(self) -> bool {
        self.0 & Self::ORD != 0
    }

    /// Whether the `PartialEq` bound is set.
    #[must_use]
    pub const fn has_eq(self) -> bool {
        self.0 & Self::EQ != 0
    }

    /// Whether the `SkyStringify` bound is set.
    #[must_use]
    pub const fn has_show(self) -> bool {
        self.0 & Self::SHOW != 0
    }

    /// Whether the `Copy` bound is set.
    #[must_use]
    pub const fn has_copy(self) -> bool {
        self.0 & Self::COPY != 0
    }

    /// Whether the `Clone` bound is set.
    #[must_use]
    pub const fn has_clone(self) -> bool {
        self.0 & Self::CLONE != 0
    }

    /// Whether the SQL-bind-parameter (`Into<SqlParam>`) bound is set — see
    /// [`Self::SQL_PARAM`].
    #[must_use]
    pub const fn has_sql_param(self) -> bool {
        self.0 & Self::SQL_PARAM != 0
    }

    /// Whether the `SkyRow` (Db field-accessor row) bound is set — see
    /// [`Self::SKY_ROW`].
    #[must_use]
    pub const fn has_sky_row(self) -> bool {
        self.0 & Self::SKY_ROW != 0
    }

    /// Whether the `std::fmt::Display` (`Basics.toString`) bound is set — see
    /// [`Self::DISPLAY`].
    #[must_use]
    pub const fn has_display(self) -> bool {
        self.0 & Self::DISPLAY != 0
    }

    /// Whether the `'static` (boxed-callback trait-object) lifetime bound is set
    /// — see [`Self::STATIC`].
    #[must_use]
    pub const fn has_static(self) -> bool {
        self.0 & Self::STATIC != 0
    }
}

/// A function: the type variables it quantifies, typed parameters, a return
/// type, and a body expression.
//
// `Eq` is not derived: `body` is an [`Expr`] that may carry a float literal
// (only `PartialEq`).
#[derive(Clone, PartialEq, Debug, serde::Serialize, serde::Deserialize)]
pub struct Func {
    pub id: FuncId,
    pub name: Symbol,
    /// The defining module's path. After `link::link` merges several modules
    /// into one this field retains the original source module path, so the
    /// backend can prefix the emitted Rust function name with the correct
    /// module segment (e.g. `lib_helper` for `home = ModPath(["Lib"])`,
    /// `main_helper` for `home = ModPath(["Main"])`) instead of always using
    /// the merged entry module's name — preventing same-named functions from
    /// different source modules from colliding with Rust E0428.
    pub home: ModPath,
    /// The type variables this function quantifies, in quantification order,
    /// each paired with its [`BoundSet`] (M2a / M2d). A type variable is a Sky
    /// type-variable [`Symbol`] that appears as an [`IrType::Generic`] in the
    /// parameters / return / body; its `BoundSet` records the Rust trait bounds
    /// the body's use of the variable demands. A monomorphic function has an
    /// empty list, so existing M0 / M1 functions are unchanged. A
    /// structurally-parametric variable carries [`BoundSet::UNBOUNDED`],
    /// so its emitted generic stays a bare `T1`.
    ///
    /// The order is load-bearing: the backend derives each variable's Rust
    /// generic name (`T1`, `T2`, …) from its *position* here, so a function
    /// quantifying `[a, b]` emits `fn name<T1, T2>(..)` with `a` → `T1` and
    /// `b` → `T2` regardless of the source variable spellings. Only the
    /// [`Symbol`] participates in naming; the [`BoundSet`] adds the `: <bounds>`
    /// clause at that position.
    pub type_params: Vec<(Symbol, BoundSet)>,
    pub params: Vec<(Symbol, IrType)>,
    pub ret: IrType,
    pub body: Expr,
}

/// The M0 type lattice. Widened in later milestones.
///
/// `Hash` is derived so record-shape collection (`sky_lower`'s
/// `collect_record_types`) can dedup via a `HashSet` gate instead of an
/// O(n²) `Vec::contains` scan (efficiency-audit §3 medium). The derive is
/// inert — it emits no IR and changes no equality semantics.
#[derive(Clone, PartialEq, Eq, Hash, Debug, serde::Serialize, serde::Deserialize)]
pub enum IrType {
    Int,
    Float,
    Bool,
    Str,
    /// A Unicode scalar value `Char`. Renders as Rust's `char`.
    Char,
    Unit,
    /// A task producing a value of type `A` (`Task Error A` in Sky). Renders as
    /// the project-level alias `SkyTask<A>` (which expands to
    /// `sky_runtime::SkyTask<SkyError, A>`). Replaces the former `TaskUnit`
    /// leaf — `Task Error ()` is now `Task(Box::new(Unit))`.
    Task(Box<Self>),
    /// A user-declared enum type, applied to its type arguments.
    ///
    /// `home` is the type's DEFINING module and `name` its bare type [`Symbol`];
    /// together they are the type's nominal identity (see [`EnumDef::home`]). Two
    /// modules each declaring `type Color` share the bare `name` but differ in
    /// `home`, so the backend resolves each use site to the correct distinct Rust
    /// enum. `args` are the concrete type arguments at a use site (`Maybe Int` →
    /// `args = [Int]`, rendered `MainMaybe<i64>`). A non-generic enum (`Msg`)
    /// carries an empty `args` list, so it renders as the bare Rust type name.
    /// An `arg` may itself be an [`IrType::Generic`] when a generic enum is passed
    /// through a generic function (`Maybe a` inside a parametric signature →
    /// `MainMaybe<T1>`).
    Enum {
        home: ModPath,
        name: Symbol,
        args: Vec<Self>,
    },
    /// The built-in `Maybe a` type, carrying its element type. Renders as the
    /// runtime's `SkyMaybe<T>`. Distinct from a user [`IrType::Enum`] so the
    /// backend maps it to the shared runtime representation (and the type
    /// checker / lowerer never need a synthetic `type Maybe a = …` declaration).
    Maybe(Box<Self>),
    /// The built-in `Result e a` type, carrying its error type then its success
    /// type (Sky's `Result e a` argument order). Renders as the runtime's
    /// `SkyResult<E, A>`.
    Result(Box<Self>, Box<Self>),
    /// The built-in `List a` type, carrying its element type. Renders as the
    /// runtime's `Vec<T>` (the representation the Rust runtime's list kernels
    /// operate over).
    List(Box<Self>),
    /// An anonymous product type `(T1, T2, ...)`.
    ///
    /// Invariant: the element list has arity ≥ 2. A 0-tuple is [`IrType::Unit`]
    /// and a 1-tuple is just its element type — neither is a `Tuple`. The
    /// lowerer is the sole producer and upholds this; the backend stays total
    /// over any vector it receives (it never panics on a degenerate arity).
    Tuple(Vec<Self>),
    /// A CLOSED record type `{ x : Int, y : Bool, ... }` — an exact, known field
    /// set keyed by field name.
    ///
    /// The field map is a [`BTreeMap`], so its iteration order is fixed (by
    /// [`Symbol`]). The backend re-canonicalises by *field name* before it
    /// derives a struct name or emits the struct body, so the synthesised Rust
    /// struct is deterministic regardless of interning order.
    ///
    /// Open / row-polymorphic records (`{ r | x : Int }`) are intentionally NOT
    /// representable here — they are deferred to M2 and rejected at lowering, so
    /// every `Record` the backend sees is closed.
    Record(BTreeMap<Symbol, Self>),
    /// A function type `T0 -> T1 -> ... -> R`, carried as its parameter list and
    /// return type (`params -> ret`).
    ///
    /// This is the type of a first-class function value — a lambda, a
    /// function-typed parameter or binding, or a top-level function used as a
    /// value. The backend renders it as a boxed trait object
    /// `Box<dyn Fn(T0, ...) -> R>`.
    ///
    /// Invariant: a zero-parameter function type (`params` empty) is a genuine
    /// nullary `Fn() -> R`, distinct from `ret` alone. The lowerer is the sole
    /// producer; the backend stays total over any parameter vector it receives.
    Fun(Vec<Self>, Box<Self>),
    /// A CURRIED chain of one-shot closures `T0 -> (T1 -> ( ... -> R))`,
    /// carried as its parameter list (one per curry level, in application
    /// order) and the FINAL, non-function return type.
    ///
    /// Distinct from [`Self::Fun`] (a flattened, re-callable
    /// `Box<dyn Fn(T0, T1, ...) -> R>`): this variant renders as NESTED
    /// `Box<dyn FnOnce(Ti) -> { next level or R } + Send>` boxes, one level
    /// of currying per parameter. Introduced for #164 (`f7_succeed_curried`)
    /// to type the `next_decoder` slot of the `JsonDec.Pipeline`/`Db.Decode`
    /// curried-combinator kernels (`decode_pipeline_required` /
    /// `decode_pipeline_optional` / `decode_pipeline_required_at` /
    /// `db_decode_required` / `db_decode_optional`), whose hand-written Rust
    /// signatures deliberately require a `Box<dyn FnOnce>` chain — each
    /// level is consumed exactly once per pipeline `run` (matching the
    /// `curryN` runtime helpers' factory-produced chains) — never the
    /// re-callable `Fn` boxing the generic [`Self::Fun`] path emits. Only
    /// ever produced by `eta_expand_partial`'s special-cased handling of
    /// those five kernels; every other position keeps the ordinary,
    /// flattened [`Self::Fun`] shape.
    ///
    /// Invariant: `params` is non-empty (a chain of zero levels is just
    /// `ret` itself, never constructed as `FnOnceChain`).
    FnOnceChain(Vec<Self>, Box<Self>),
    /// A generic type parameter — a Sky type variable used STRUCTURALLY
    /// (pass-through, no operation applied to it) in a fully-parametric
    /// top-level function. The carried [`Symbol`] is the source type
    /// variable's name (e.g. interned `"a"`).
    ///
    /// The backend renders this as the function's corresponding Rust generic
    /// (`T1`, `T2`, …), resolved by the variable's position in the enclosing
    /// [`Func::type_params`] — not by the symbol's spelling — so emission is
    /// deterministic regardless of source naming.
    ///
    /// A `Generic` is only ever in scope inside a function that quantifies it;
    /// it never appears in a program-level position (enum / record-struct
    /// declaration). Constrained type variables (those needing a Rust trait
    /// bound — `Number` / `Comparable` / `Appendable`) are NOT representable
    /// here: they are rejected at lowering.
    ///
    /// The wildcard `any` is a SEPARATE case, not genuine polymorphism: the
    /// checker gives every `any` occurrence in an annotation its own fresh flex
    /// UV ("fresh flex UV per occurrence" — `sky_types::constrain`), so two
    /// `any` params in one signature can be pinned to two DIFFERENT concrete
    /// types by the body. `split_typed_sig` (AUD-01 seal fix) resolves each
    /// param-position `any` from the def's solved env type, per occurrence, to
    /// its concrete `IrType` whenever that solved type is available — a shared
    /// `Generic(any_sym)` here would collapse distinct occurrences onto ONE
    /// Rust generic (exit-0-then-cargo-fail). A `Generic` carrying the interned
    /// `"any"` symbol therefore still appears ONLY as the fallback when the
    /// solved type genuinely could not be resolved (should not occur for a
    /// well-formed `Def::Typed` post-solve) — it is not the steady-state
    /// representation the way it is for a genuine type parameter.
    Generic(Symbol),
    /// The built-in `Dict k v` associative map type, carrying its key type then
    /// its value type. Renders as the runtime's `HashMap<K, V>` (backed by
    /// `std::collections::HashMap`). Distinct from a user [`IrType::Enum`] so
    /// the backend maps it to the shared runtime representation. Key iteration
    /// is sorted for determinism on the Rust backend (Go iterates map-order).
    Dict(Box<Self>, Box<Self>),
    /// The built-in `Set a` unordered-set type, carrying its element type.
    /// Renders as the runtime's `BTreeSet<A>` (backed by
    /// `std::collections::BTreeSet`). Iteration is sorted on the Rust backend
    /// (Go uses an unordered internal map) — a conforming strengthening.
    Set(Box<Self>),
    /// The built-in `Bytes` type — an arbitrary byte buffer.
    ///
    /// Divergence from Sky: Sky defines `type alias Bytes = String` (Go's
    /// `string` is a byte sequence, making the alias cost-free). Rust's
    /// `String` is UTF-8 constrained; mapping `Bytes` to `String` would be
    /// unsound for non-UTF-8 binary payloads. Sky-Rust makes `Bytes` a
    /// distinct primitive lowering to `Vec<u8>` — lossless for arbitrary
    /// binary, with explicit UTF-8 conversion via `Bytes.fromString` /
    /// `Bytes.toString`. Rationale: Rust type-system correctness.
    Bytes,
    /// The JSON value type — an opaque, dynamically-typed JSON node.
    ///
    /// The Sky `Value` type alias (`Value = any`) creates an unresolved
    /// `Ty::Var` at use sites.  In a JSON-kernel context the concrete Rust
    /// type is always `serde_json::Value`, re-exported from the runtime as
    /// `JsonVal`.  The lowerer produces this variant when a `Ty::Var`
    /// appears in the argument or return position of a `JsonEnc.*` kernel
    /// call — the only place in the M4g subset where `any` is meaningful.
    /// The backend emits `JsonVal`.
    Json,
    /// The `Decoder a` type — an opaque decoder that reads a JSON value and
    /// produces a value of type `a`.
    ///
    /// Introduced in M4h (`Sky.Core.Json.Decode`).  Renders as
    /// `Decoder<T>` using the emitted project's preamble type alias:
    /// `pub type Decoder<T> = sky_runtime::json::Decoder<SkyError, T>`.
    Decoder(Box<Self>),
    /// The `Db` connection pool type — an opaque handle to an open database
    /// connection pool (`Std.Db`).
    ///
    /// Introduced in M5b-db.  Renders as `Db` via the runtime re-export
    /// `pub use sky_runtime::Db;` in the emitted crate preamble.  The type is
    /// zero-argument (no type parameters) and value-cloneable (the pool is
    /// reference-counted internally).
    Db,
    /// A `Cmd msg` value — an opaque command produced by the `update` function
    /// and passed back to the TEA runtime.
    ///
    /// Introduced in M5c.  Renders as `SkyCmd<T>` via the project-level alias
    /// `pub type SkyCmd<M> = sky_runtime::tea::SkyCmd<M>`.
    /// The inner type is the message type `M`.
    Cmd(Box<Self>),
    /// A `Sub msg` value — an opaque subscription descriptor returned by
    /// the `subscriptions` function.
    ///
    /// Introduced in M5c.  Renders as `SkySub<T>` via the project-level alias
    /// `pub type SkySub<M> = sky_runtime::tea::SkySub<M>`.
    /// The inner type is the message type `M`.
    Sub(Box<Self>),
    // ── Sky.Http.Server opaque types ────────────────────────────────────
    /// `Request` — opaque HTTP server request.  Renders as `ServerRequest`.
    ///
    /// Corresponds to `sky_runtime::server::ServerRequest`.  Never synthesised
    /// as a record struct; always treated as an opaque handle.
    ServerRequest,
    /// `Response` — opaque HTTP server response.  Renders as `ServerResponse`.
    ///
    /// Corresponds to `sky_runtime::server::ServerResponse`.
    ServerResponse,
    /// `Route` — opaque server route descriptor.  Renders as `ServerRoute`.
    ///
    /// Corresponds to `sky_runtime::server::ServerRoute`.
    ServerRoute,
    /// `Cookie` — opaque server cookie descriptor.  Renders as `ServerCookie`.
    ///
    /// Corresponds to `sky_runtime::server::ServerCookie`.
    ServerCookie,
    /// `StreamWriter` — opaque server-side stream writer handle.  Renders as
    /// `StreamWriter`.
    ///
    /// Corresponds to `sky_runtime::server_stream::StreamWriter`.  Used as the
    /// argument type of the `Stream.stream` callback and the target type of
    /// `Stream.emit` / `Stream.finish` / `Stream.withContentType`.  Never
    /// synthesised as a record struct; always treated as an opaque handle.
    StreamWriter,
    /// `HttpRequest` — opaque HTTP request descriptor used by `Sky.Core.Http`
    /// and `Sky.Core.Http.Stream`.  Renders as `HttpRequest`.
    ///
    /// Corresponds to `sky_runtime::http::HttpRequest`.  In Sky source, users
    /// write `HttpRequest` literals as structural records; the lowerer detects
    /// the canonical 7-field set (`body`, `followRedirects`, `headers`,
    /// `maxRedirects`, `method`, `timeout`, `url`) and folds it to this opaque
    /// variant instead of synthesising a backend record struct, so call sites
    /// that pass the value to `http_stream_open` / `http_request` kernels see
    /// the correct runtime type.  Never stored in a Sky.Live Model.
    HttpRequest,
    // ── Sky.Http.Server.WebSocket opaque type handles ──────────────────
    /// `WebSocketServer` — opaque per-peer WebSocket handle.  Renders as
    /// `WsHandle`.
    ///
    /// Passed to every `WsServerCfg` callback as the first argument; also
    /// accepted by `Ws.sendToClient` / `Ws.sendBinaryToClient` /
    /// `Ws.broadcast` / `Ws.closeClient`.  Never stored in a Sky.Live Model.
    WebSocketServer,
    /// `WebSocketServerCfg` — opaque WebSocket server configuration.  Renders
    /// as `WsServerCfg<SkyError>`.
    ///
    /// Constructed by `Ws.defaultCfg` and threaded through the `Ws.with*`
    /// builder chain; consumed by `Ws.upgrade`.  Phantom `msg` type parameter
    /// dropped (D2 — see docs/adr/0023-websocket-server-kernel-only-typed-handles.md).
    WebSocketServerCfg,
    // ── Std.Ui / Std.Html parametric types ──────────────────────────────
    /// A parametric `Std.Ui` or `Std.Html` type — one that carries a message type
    /// parameter `msg`.  The `ctor` field identifies which of the five
    /// message-parametric types this is; `msg` is the message type.
    ///
    /// | ctor                     | Rust type                                    |
    /// |--------------------------|----------------------------------------------|
    /// | `UiCtor::Html`           | `sky_runtime::html::Html<M>`                 |
    /// | `UiCtor::Element`        | `sky_runtime::ui::element::Element<M>`       |
    /// | `UiCtor::UiAttribute`    | `sky_runtime::ui::element::Attribute<M>`     |
    /// | `UiCtor::HtmlAttribute`  | `sky_runtime::html::Attribute<M>`            |
    /// | `UiCtor::HtmlEvent`      | `sky_runtime::html::Event<M>`                |
    /// | `UiCtor::Label`          | `sky_runtime::ui::input::Label<M>`           |
    /// | `UiCtor::Placeholder`    | `sky_runtime::ui::input::Placeholder<M>`     |
    /// | `UiCtor::RadioOption`    | `sky_runtime::ui::input::RadioOption<M>`     |
    Ui {
        ctor: UiCtor,
        msg: Box<Self>,
    },
    /// A nullary (non-parametric) `Std.Ui` type.  These are closed value types
    /// that carry no message type parameter.
    ///
    /// | plain             | Rust type                                         |
    /// |-------------------|---------------------------------------------------|
    /// | `Length`          | `sky_runtime::ui::element::Length`                |
    /// | `Color`           | `sky_runtime::ui::element::Color`                 |
    /// | `HAlign`          | `sky_runtime::ui::element::HAlign`                |
    /// | `VAlign`          | `sky_runtime::ui::element::VAlign`                |
    /// | `Location`        | `sky_runtime::ui::element::Location`              |
    /// | `PseudoClass`     | `sky_runtime::ui::element::PseudoClass`           |
    /// | `Description`     | `sky_runtime::ui::element::Description`           |
    /// | `LayoutContext`   | `sky_runtime::ui::element::LayoutContext`          |
    UiPlain(UiPlain),
    /// `LiveReq` — opaque request type threaded through `Live.app`'s `init`
    /// callback.  Rendered as `sky_runtime::live::LiveReq`.
    LiveReq,
    /// `LiveRoute page` — route descriptor returned by `Live.route`, carrying
    /// the page type it builds. Rendered as
    /// `sky_runtime::live::route::Route<Page>`. The runtime `Route<Page>`
    /// struct has NO default type parameter, so the page argument is
    /// load-bearing: rendering a bare `Route` (the pre-#108-round-4 shape) is
    /// an E0107 `cargo` failure whenever the type reaches a rendered position
    /// (an empty `routes = []` literal's `Vec::<…>::new()` turbofish, or a
    /// let-bound route table's fn signature).
    LiveRoute(Box<Self>),
    /// The built-in `Order` type — the result of `Basics.compare`.
    ///
    /// Renders as `sky_runtime::SkyOrder` (the `#[repr(u8)]` enum exposed from
    /// the runtime's `basics` module and re-exported via `pub use basics::*`).
    /// Constructors `LT / EQ / GT` emit as `sky_runtime::SkyOrder::LT` etc.
    /// via the `builtin_runtime_enum` path in the backend (no synthetic
    /// `EnumDef` is injected — the enum lives entirely in the runtime crate).
    ///
    /// Sanctioned divergence from Sky/Go: Go's `Basics_compareT` returns an
    /// `int` (-1/0/1).  The Rust backend uses a typed enum for sound exhaustive
    /// pattern matching without a range-check.
    Order,

    /// `Std.Decimal` — arbitrary-precision decimal arithmetic.
    ///
    /// Renders as `sky_runtime::decimal::Decimal` (newtype around
    /// `rust_decimal::Decimal`).  Carries Copy + serde semantics; used as
    /// a field type in record structs and as a direct call-argument / return
    /// value for all `Decimal.*` kernels.
    Decimal,

    /// The built-in `ErrorKind` type — `Error`'s 11-variant classification
    ///
    /// Renders as `sky_runtime::error::SkyErrorKind` (a `#[repr(u8)]` enum,
    /// same convention as [`IrType::Order`]). Constructors (`Io` / `Network` /
    /// …) emit via the `builtin_runtime_enum` path — no synthetic `EnumDef`.
    ErrorKind,

    /// The built-in `Error` type — `Error ErrorKind ErrorInfo` (backlog
    /// #85/#160).
    ///
    /// Renders as `sky_runtime::error::SkyError`. Sole constructor shares its
    /// name with the type (`enum_variants[(Prelude, error)] = [error]`, set in
    /// `sky_lower`), so it emits as the tuple variant `SkyError::Error(kind,
    /// info)` via the SAME `builtin_runtime_enum` path `Maybe`/`Result` use —
    /// no synthetic `EnumDef`, no new emitter mechanism.
    ///
    /// `ErrorInfo` carries `{ message : String, details : Maybe ErrorDetails
    /// }` — a plain closed record, not a leaf `IrType` (Sky records are
    /// structural); `details` was added by the backlog #85 follow-up.
    Error,

    /// The built-in `ErrorDetails` type — the 5-variant enrichment union
    /// carried optionally on `ErrorInfo.details` (backlog #85 follow-up).
    ///
    /// Renders as `sky_runtime::error::SkyErrorDetails`. Constructor names
    /// match Sky source verbatim (`FfiPanic` / `TypeMismatch` / `HttpStatus`
    /// / `JsonDecode` / `Custom`) and emit via the SAME `builtin_runtime_enum`
    /// path [`IrType::Error`]/[`IrType::ErrorKind`] use — no synthetic
    /// `EnumDef`.
    ErrorDetails,

    /// The built-in NOMINAL `ErrorInfo` type — `Error`'s second constructor
    /// argument, `{ message : String, details : Maybe ErrorDetails }` at the
    /// field level (SEAL fix — see `docs/architecture/
    /// docs/adr/0017-error-payload-nominal-identity.md`).
    ///
    /// Renders as `sky_runtime::error::SkyErrorInfo`. NOT a structural
    /// record: a bare record literal cannot construct it (the type checker
    /// rejects the unification), so the backend never has to reconcile a
    /// synthesized record struct with the runtime's concrete type — the
    /// exit-0-then-cargo-fail this leaf exists to prevent. Field access is
    /// resolved by `sky_types`' `ErrorRecordFields` table and emits plain
    /// `.message` / `.details` reads of the runtime struct's pub fields.
    ErrorInfo,

    /// The built-in NOMINAL `PanicInfo` type — `FfiPanic`'s payload,
    /// `{ message : String, stack : List String }` at the field level (SEAL
    /// fix; same design as [`IrType::ErrorInfo`]).
    ///
    /// Renders as `sky_runtime::error::SkyPanicInfo`.
    PanicInfo,

    /// The built-in NOMINAL `TypeInfo` type — `TypeMismatch`'s payload,
    /// `{ expected : String, actual : String }` at the field level (SEAL fix
    /// same design as [`IrType::ErrorInfo`]).
    ///
    /// Renders as `sky_runtime::error::SkyTypeInfo`.
    TypeInfo,

    /// `Std.Db.Sql`'s opaque WHERE-fragment type (backlog #61 — SQL injection
    /// closed by construction: a `SqlFragment` can only be built through the
    /// `Sql.*` combinator kernels, never from an arbitrary `String`).
    ///
    /// Renders as `sky_runtime::db::SqlFragment`. Fully `Clone + PartialEq`
    /// (derivable), but NOT serde — it is a query-building value, never
    /// persisted to a Live session store. `Debug` is hand-written on the
    /// runtime type to show SQL text + bind COUNT only, never bind values (a
    /// bind may carry a revealed secret).
    SqlFragment,

    /// `Sky.Core.Secret`'s opaque, sealed secret-string type (backlog #44 —
    /// "secrets are typed, never `fmt`-stringified": a `Secret` can only be
    /// built through `Secret.fromString`, never implicitly from a `String`).
    ///
    /// Renders as `sky_runtime::secret::Secret`. Fully `Clone + PartialEq`
    /// (derivable — `PartialEq` is hand-written and CONSTANT-TIME, the only
    /// equality impl the runtime type has), but NOT serde — a `Secret` must
    /// never round-trip through a session store or any other serialisation
    /// path (this is ALSO the WASM hydration-island containment predicate a
    /// future `HydrationState` field-type gate consults, per
    /// `docs/architecture/wasm-target.md` §Q6 — nothing to build yet, the
    /// target does not exist). `Debug` and the Sky-facing `SkyStringify` (the
    /// trait backing `toString` / interpolation / `Log.*With`) are BOTH
    /// hand-written on the runtime type to ALWAYS render a fixed
    /// `"<redacted>"` placeholder, never the wrapped value — see
    /// `sky_runtime::secret`'s module doc for the full design.
    Secret,

    /// `Std.Cache`'s configuration record `{ maxEntries : Int, ttlMs : Int,
    /// maxBytes : Int }`. Renders as `sky_runtime::cache::CacheCfg`.
    ///
    /// The lowerer folds any solved / annotated record matching that exact
    /// 3-field shape to this opaque variant (same mechanism as
    /// [`IrType::HttpRequest`]) so a `Cache.defaultCfg`-built record literal
    /// constructs the runtime struct the `cache_new_raw` kernel takes, rather
    /// than a backend-synthesised `RecMaxBytes…` struct that would mismatch it
    /// (E0308). Fully `Clone` (derivable on the runtime struct); never stored in
    /// a Sky.Live Model.
    CacheCfg,

    /// `Std.Cache.stats`'s return record `{ hits : Int, misses : Int,
    /// evictions : Int }`. Renders as `sky_runtime::cache::CacheStats`.
    ///
    /// Folded the same way as [`IrType::CacheCfg`]: the `statsRaw` kernel
    /// alias's annotated return type is this record shape, and the runtime
    /// `cache_stats` returns `CacheStats`, so folding the annotation to this
    /// nominal type keeps the wrapper's declared return type in step with the
    /// kernel (otherwise E0308). Fields are read via `.hits`/`.misses`/
    /// `.evictions` on the runtime struct's pub fields.
    CacheStats,
}

/// Tag enum for the message-parametric `Std.Ui` / `Std.Html` types.
///
/// Used inside [`IrType::Ui`] to select the correct Rust path at emission time.
/// The set is intentionally small to keep the pattern match exhaustive without a
/// catch-all.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, serde::Serialize, serde::Deserialize)]
pub enum UiCtor {
    /// `Html msg` — a rendered HTML tree (`sky_runtime::html::Html<M>`).
    Html,
    /// `Element msg` — a Std.Ui layout element (`sky_runtime::ui::element::Element<M>`).
    Element,
    /// `Attribute msg` from `Std.Ui` — a layout attribute (`sky_runtime::ui::element::Attribute<M>`).
    UiAttribute,
    /// `Attribute msg` from `Std.Html` / `Std.Html.Attributes` —
    /// an HTML attribute (`sky_runtime::html::Attribute<M>`).
    HtmlAttribute,
    /// `Event msg` from `Std.Html.Events` —
    /// an HTML event handler (`sky_runtime::html::Event<M>`).
    HtmlEvent,
    /// `Label msg` — a `Std.Ui.Input` label descriptor (`sky_runtime::ui::input::Label<M>`).
    Label,
    /// `Placeholder msg` — a `Std.Ui.Input` placeholder descriptor
    /// (`sky_runtime::ui::input::Placeholder<M>`).
    Placeholder,
    /// `RadioOption msg` — a `Std.Ui.Input` radio option descriptor
    /// (`sky_runtime::ui::input::RadioOption<M>`).
    RadioOption,
}

/// Tag enum for the nullary (non-message-parametric) `Std.Ui` types.
///
/// Used inside [`IrType::UiPlain`] to select the correct Rust path at emission
/// time.  The set is closed (eight variants).
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, serde::Serialize, serde::Deserialize)]
pub enum UiPlain {
    /// `Length` — `sky_runtime::ui::element::Length`.
    Length,
    /// `Color` — `sky_runtime::ui::element::Color`.
    Color,
    /// `HAlign` — `sky_runtime::ui::element::HAlign`.
    HAlign,
    /// `VAlign` — `sky_runtime::ui::element::VAlign`.
    VAlign,
    /// `Location` — `sky_runtime::ui::element::Location`.
    Location,
    /// `PseudoClass` — `sky_runtime::ui::element::PseudoClass`.
    PseudoClass,
    /// `Description` — `sky_runtime::ui::element::Description`.
    Description,
    /// `LayoutContext` — `sky_runtime::ui::element::LayoutContext`.
    LayoutContext,
}

/// Total predicate: does the Rust type that [`IrType`] renders to support the
/// full `#[derive(Clone, Debug, PartialEq)]` set the backend stamps on every
/// generated enum / record struct?
///
/// This is the authoritative soundness gate that keeps the *unconditional*
/// derive off any type whose rendered Rust form lacks one of those traits, so a
/// well-typed program that stores such a value in a record field or enum payload
/// can never `skyc`-succeed and then `cargo`-fail on a missing `Clone` / `Debug`
/// / `PartialEq` impl. The non-derivable leaves are:
///
/// * [`IrType::Fun`] — a first-class function, rendered `Box<dyn Fn(..) -> R>`
///   (no `Clone`/`Debug`/`PartialEq`).
/// * the opaque effect / handle wrappers [`IrType::Task`], [`IrType::Cmd`],
///   [`IrType::Sub`], [`IrType::Decoder`], [`IrType::Db`] — each wraps a boxed
///   closure or future (no `Clone`/`Debug`/`PartialEq`).
/// * the opaque server / live handles [`IrType::ServerRequest`] /
///   [`IrType::ServerResponse`] / [`IrType::ServerRoute`] /
///   [`IrType::ServerCookie`] / [`IrType::StreamWriter`] /
///   [`IrType::HttpRequest`] / [`IrType::WebSocketServer`] /
///   [`IrType::WebSocketServerCfg`] / [`IrType::LiveReq`] / [`IrType::LiveRoute`]
///   (each lacks at least `PartialEq`).
/// * the two `Clone`-only `Std.Html` carriers [`UiCtor::HtmlAttribute`] /
///   [`UiCtor::HtmlEvent`] (they hold `Arc<dyn Fn>` event handlers).
///
/// A non-derivable leaf poisons every *transparent carrier* that reaches it
/// (list / set / tuple / dict / result / maybe / closed record) and every user
/// enum whose payload reaches it. `enum_derivable` answers the same question for
/// a referenced user [`IrType::Enum`]; the caller resolves the enum-to-enum
/// fixpoint before consulting it (see the backend `EmitCtx`).
///
/// Leaves that DO render to a fully-derivable Rust type return `true`:
/// primitives, `Bytes` (`Vec<u8>`), `Json` (`serde_json::Value`), every
/// [`UiPlain`] value type, the three fully-derivable [`IrType::Ui`] carriers
/// (`Html` / `Element` / ui-`Attribute`), and a [`IrType::Generic`] parameter
/// (the `derive` macro adds the per-parameter trait bound, so the generic frame
/// stays derivable).
///
/// The match is deliberately exhaustive with no wildcard arm: a new [`IrType`]
/// variant must make an explicit derivability decision here rather than default
/// silently into the derivable branch (walker-arm rule).
#[must_use]
pub fn ir_type_is_derivable(
    ty: &IrType,
    enum_derivable: &impl Fn(&ModPath, Symbol) -> bool,
) -> bool {
    match ty {
        IrType::Int
        | IrType::Float
        | IrType::Bool
        | IrType::Str
        | IrType::Char
        | IrType::Unit
        | IrType::Bytes
        | IrType::Json
        // `SkyOrder` derives Clone + Copy + PartialEq + Eq + Debug — fully derivable.
        // `Decimal` derives Clone + Copy + PartialEq + Eq + Debug — fully derivable.
        // `SkyErrorKind` derives Clone + Copy + PartialEq + Eq + Debug.
        // `SkyError` derives Clone + PartialEq + Debug (not Copy — carries a
        // heap-allocated `String` message; not `Eq` — its `SkyErrorInfo`
        // field carries a `SkyMaybe`, which is `PartialEq`-only).
        // `SkyErrorDetails` derives Clone + PartialEq + Eq + Debug (backlog
        // follow-up).
        // `SkyErrorInfo` derives Clone + PartialEq + Debug (not Eq — carries
        // a `SkyMaybe`); `SkyPanicInfo`/`SkyTypeInfo` derive Clone +
        // PartialEq + Eq + Debug (SEAL fix).
        // `SqlFragment` derives Clone + PartialEq (hand-written Debug; see
        // its own doc) — fully derivable, not serde (see `ir_type_is_serde`).
        // `Secret` derives Clone; `PartialEq`/`Debug` are hand-written
        // (constant-time equality, always-redacting Debug — see its own doc)
        // — fully derivable, not serde (see `ir_type_is_serde`).
        | IrType::Order
        | IrType::Decimal
        | IrType::ErrorKind
        | IrType::Error
        | IrType::ErrorDetails
        | IrType::ErrorInfo
        | IrType::PanicInfo
        | IrType::TypeInfo
        | IrType::SqlFragment
        | IrType::Secret
        // Cache config / stats runtime structs derive Clone+Debug+PartialEq.
        | IrType::CacheCfg
        | IrType::CacheStats
        | IrType::Generic(_)
        | IrType::UiPlain(_) => true,
        // The fully-derivable Std.Ui / Std.Html carriers vs the two Clone-only
        // ones (`html::Attribute` / `html::Event`, which hold `Arc<dyn Fn>`).
        IrType::Ui { ctor, msg } => {
            matches!(
                ctor,
                UiCtor::Html
                    | UiCtor::Element
                    | UiCtor::UiAttribute
                    | UiCtor::Label
                    | UiCtor::Placeholder
                    | UiCtor::RadioOption
            ) && ir_type_is_derivable(msg, enum_derivable)
        }
        // Non-derivable opaque leaves (each lacks ≥1 of Clone / Debug / PartialEq).
        IrType::Task(_)
        | IrType::Cmd(_)
        | IrType::Sub(_)
        | IrType::Decoder(_)
        | IrType::Db
        | IrType::Fun(_, _)
        // A curried `Box<dyn FnOnce>` chain is exactly as non-derivable as
        // `Fun` (it renders to the same family of boxed trait objects).
        | IrType::FnOnceChain(_, _)
        | IrType::ServerRequest
        | IrType::ServerResponse
        | IrType::ServerRoute
        | IrType::ServerCookie
        // `StreamWriter` derives Clone+Copy+Debug but not PartialEq — not fully derivable.
        | IrType::StreamWriter
        // `HttpRequest` is an opaque handle — not fully derivable.
        | IrType::HttpRequest
        // `WsHandle` / `WsServerCfg` are opaque handles — not fully derivable.
        | IrType::WebSocketServer
        | IrType::WebSocketServerCfg
        | IrType::LiveReq
        // `Route<Page>` holds an `Arc<dyn Fn>` builder — never derivable/serde
        // regardless of its page argument.
        | IrType::LiveRoute(_) => false,
        // Transparent carriers: derivable iff every carried element is.
        IrType::Maybe(e) | IrType::List(e) | IrType::Set(e) => {
            ir_type_is_derivable(e, enum_derivable)
        }
        IrType::Result(a, b) | IrType::Dict(a, b) => {
            ir_type_is_derivable(a, enum_derivable) && ir_type_is_derivable(b, enum_derivable)
        }
        IrType::Tuple(es) => es.iter().all(|e| ir_type_is_derivable(e, enum_derivable)),
        IrType::Record(fields) => fields
            .values()
            .all(|f| ir_type_is_derivable(f, enum_derivable)),
        IrType::Enum { home, name, args } => {
            enum_derivable(home, *name)
                && args.iter().all(|a| ir_type_is_derivable(a, enum_derivable))
        }
    }
}

/// Total predicate: does the Rust type that [`IrType`] renders to derive
/// `serde::Serialize` **and** `serde::de::DeserializeOwned`?
///
/// This is the authoritative admissibility gate for a `Std.Live` / `Sky.Live`
/// **Model**: the live runtime persists the Model to the session store, so
/// `live_app` bounds it "Serialize + `DeserializeOwned` + Clone + `PartialEq`".
/// Without this gate a well-typed program that stores a non-serialisable value
/// in its Model `skyc`-succeeds and then `cargo`-fails on the missing `serde`
/// bound (the #91 seal hole).
///
/// The serde-OK leaf set is a **strict subset** of the derivable leaf set
/// ([`ir_type_is_derivable`]): every serde-OK leaf is also derivable, so
/// `ir_type_is_serde(t) ⇒ ir_type_is_derivable(t)` structurally (the two arms
/// that differ both demote to `false` here). Consequently a serde-admissible
/// Model automatically satisfies the `Clone + PartialEq` half of the bound too —
/// the Live gate needs only this one predicate.
///
/// serde-OK leaves (render to a `serde`-deriving Rust type):
/// * primitives `Int` / `Float` / `Bool` / `Str` / `Char` / `Unit`,
/// * `Bytes` (`Vec<u8>`), `Json` (`serde_json::Value` — itself `serde`),
/// * `Generic(_)` — the derive macro adds a per-parameter `T: Serialize` /
///   `T: DeserializeOwned` bound, so the frame stays admissible (a concrete
///   Model never carries a free generic anyway).
///
/// NON-serde leaves — the full non-derivable set (functions, the opaque
/// effect/handle wrappers, the two `Clone`-only `Std.Html` carriers) PLUS the
/// derivable-but-not-`serde` UI value types:
/// * every [`IrType::Ui`] carrier (`Html` / `Element` / ui-`Attribute` and the
///   two `html` carriers) — verified: `runtime/src/sky_runtime/{html,ui}` derive
///   only `Clone, Debug, PartialEq`, never `Serialize` / `Deserialize`,
/// * every [`IrType::UiPlain`] value type (`Length` / `Color` / `HAlign` / … →
///   `sky_runtime::ui::element::*`) — verified: no `serde` derive in
///   `runtime/src/sky_runtime/ui`.
///
/// A non-serde leaf poisons every transparent carrier that reaches it and every
/// user enum whose payload reaches it. `enum_serde` answers the same question
/// for a referenced user [`IrType::Enum`]; the caller resolves the enum-to-enum
/// fixpoint before consulting it (see the backend `EmitCtx`, which computes an
/// `enum_serde` fixpoint parallel to `enum_derivable`).
///
/// The match is deliberately exhaustive with no wildcard arm: a new [`IrType`]
/// variant must make an explicit serde decision here (walker-arm rule).
#[must_use]
pub fn ir_type_is_serde(ty: &IrType, enum_serde: &impl Fn(&ModPath, Symbol) -> bool) -> bool {
    match ty {
        IrType::Int
        | IrType::Float
        | IrType::Bool
        | IrType::Str
        | IrType::Char
        | IrType::Unit
        | IrType::Bytes
        | IrType::Json
        | IrType::Generic(_)
        // `Order` (LT/EQ/GT) is a plain no-payload enum; SkyOrder derives serde.
        // `Decimal` is a Copy newtype; rust_decimal supports serde via feature.
        // `SkyErrorKind`/`SkyError`/`SkyErrorDetails` derive serde — `Error`
        // must serialize to round-trip through a Live session store (e.g. a
        // Model's `historyError : Maybe Error` field). The nominal payload
        // types `SkyErrorInfo`/`SkyPanicInfo`/`SkyTypeInfo` derive serde for
        // the same reason (they ride inside `Error`; SEAL fix).
        | IrType::Order
        | IrType::Decimal
        | IrType::ErrorKind
        | IrType::Error
        | IrType::ErrorDetails
        | IrType::ErrorInfo
        | IrType::PanicInfo
        | IrType::TypeInfo => true,
        // All non-serde leaves collapse to `false`:
        //   * `UiPlain` value types (`Length`/`Color`/… → `ui::element::*`) and
        //     every `Ui` carrier (`Html`/`Element`/`Attribute`) derive only
        //     Clone/Debug/PartialEq, never serde — the two arms where serde is
        //     STRICTER than `ir_type_is_derivable` (there they are `true`);
        //   * the opaque effect/handle wrappers + first-class functions (also
        //     non-derivable) — a `Box<dyn Fn>` / future / handle is not serde.
        IrType::UiPlain(_)
        | IrType::Ui { .. }
        | IrType::Task(_)
        | IrType::Cmd(_)
        | IrType::Sub(_)
        | IrType::Decoder(_)
        | IrType::Db
        | IrType::Fun(_, _)
        // Same family as `Fun` — a boxed `FnOnce` chain is never serde.
        | IrType::FnOnceChain(_, _)
        | IrType::ServerRequest
        | IrType::ServerResponse
        | IrType::ServerRoute
        | IrType::ServerCookie
        // `StreamWriter` is an opaque handle; not serde.
        | IrType::StreamWriter
        // `HttpRequest` is an opaque handle; not serde.
        | IrType::HttpRequest
        // `SqlFragment` is a query-building value, never persisted to a Live
        // session store — derivable (see `ir_type_is_derivable`) but not serde.
        | IrType::SqlFragment
        // `Secret` must NEVER round-trip through serde (session store, JSON
        // encode, anything) — derivable (see `ir_type_is_derivable`) but not
        // serde. This is the load-bearing gate that makes a `Std.Live` Model
        // field of type `Secret` a compile-time SKY-L0120, not a session-store
        // leak.
        | IrType::Secret
        // Cache config / stats are kernel-boundary data records — derivable
        // (see `ir_type_is_derivable`) but never persisted to a session store, so
        // not serde (the runtime structs carry no serde derive).
        | IrType::CacheCfg
        | IrType::CacheStats
        // `WsHandle` / `WsServerCfg` are opaque handles; not serde.
        | IrType::WebSocketServer
        | IrType::WebSocketServerCfg
        | IrType::LiveReq
        // `Route<Page>` holds an `Arc<dyn Fn>` builder — never derivable/serde
        // regardless of its page argument.
        | IrType::LiveRoute(_) => false,
        // Transparent carriers: serde-OK iff every carried element is.
        IrType::Maybe(e) | IrType::List(e) | IrType::Set(e) => ir_type_is_serde(e, enum_serde),
        IrType::Result(a, b) | IrType::Dict(a, b) => {
            ir_type_is_serde(a, enum_serde) && ir_type_is_serde(b, enum_serde)
        }
        IrType::Tuple(es) => es.iter().all(|e| ir_type_is_serde(e, enum_serde)),
        IrType::Record(fields) => fields.values().all(|f| ir_type_is_serde(f, enum_serde)),
        IrType::Enum { home, name, args } => {
            enum_serde(home, *name) && args.iter().all(|a| ir_type_is_serde(a, enum_serde))
        }
    }
}

/// An expression in the typed IR.
///
/// Note: the [`Match`] variant wraps the opaque [`Match`] type rather than
/// inlining `scrutinee` / `arms` fields. That is deliberate — it keeps the
/// exhaustiveness invariant unbreakable, because the only constructor for a
/// [`Match`] is [`Match::new`], which validates the arm set. An inline
/// struct-variant with public fields could be built directly, bypassing the
/// check, and would make illegal IR representable.
// `Eq` is not derived: [`Expr::Float`] carries an `f64`, which is only
// `PartialEq` (IEEE-754). No consumer keys a map / set on an [`Expr`].
#[derive(Clone, PartialEq, Debug, serde::Serialize, serde::Deserialize)]
pub enum Expr {
    Int(i64),
    /// A boolean literal `True` / `False` used as a VALUE. Sky's `Bool` is the
    /// closed two-constructor type whose constructors are Prelude-exposed; the
    /// backend renders this as the Rust `true` / `false` keyword constant. (A
    /// `Bool` PATTERN is the separate [`Pat::Bool`] leaf.)
    Bool(bool),
    /// A floating-point literal — the carried [`f64`] is the parsed value. The
    /// backend renders it as an f64-typed Rust literal (a whole-number value
    /// keeps its decimal point, `3.0`, so it never types as an integer).
    Float(f64),
    /// A string literal — the carried [`String`] is the already-unescaped value.
    /// The backend renders it as an owned `String` (`"…".to_string()`).
    Str(String),
    /// A character literal — the carried [`String`] is the single unescaped
    /// character's text. The backend renders it as a Rust `char` literal.
    Char(String),
    /// The unit value `()` — the sole inhabitant of [`IrType::Unit`].
    ///
    /// Sky's `()` literal lowers here; the backend emits the Rust unit
    /// expression `()`. Distinct from a zero-element [`Expr::Tuple`], which the
    /// tuple invariant forbids (arity ≥ 2): the empty product is this `Unit`.
    Unit,
    Var(Symbol),
    /// A read of a CAPTURED binder inside a closure body that must not consume
    /// the capture: renders as `{ident}.clone()`. Produced only by the lowerer's
    /// capture-clone rewrite (`lower_lambda` / `eta_expand_partial` / Fix-C thunk);
    /// never for `Copy`-classed or function-typed captures.
    CloneVar(Symbol),
    /// A constructor application `Variant arg0 arg1 …` (a nullary constructor
    /// `Variant` has an empty `args`).
    ///
    /// `home` + `ty` are the constructor's enum-type nominal identity (its
    /// defining module and bare type [`Symbol`]; see [`EnumDef::home`]); `variant`
    /// is the constructor name. `args` are the payload expressions, one per
    /// declared field, in source order. The backend resolves the variant's
    /// declared field types from the enum declaration (keyed by `(home, ty)`) to
    /// wrap any direct-self-recursive field in `Box::new` at construction
    /// (matching the boxed enum field).
    Ctor {
        home: ModPath,
        ty: Symbol,
        variant: Symbol,
        args: Vec<Self>,
    },
    BinOp {
        op: BinOp,
        lhs: Box<Self>,
        rhs: Box<Self>,
    },
    /// A non-recursive single-binding `let name = value in body`. Multi-binding
    /// `let` lowers to nested `Let`s; `name` is bound only within `body`, not in
    /// `value`.
    Let {
        name: Symbol,
        value: Box<Self>,
        body: Box<Self>,
    },
    /// An irrefutable destructuring binding `let <binder> = value in body`.
    ///
    /// The sibling of [`Self::Let`] for the pattern-binder case: where `Let`
    /// binds a single [`Symbol`] (the audited common fast path), `Destructure`
    /// binds an IRREFUTABLE [`Pat`] — a [`Pat::Tuple`] of variables / wildcards
    /// (recursively), or a bare [`Pat::Var`] / [`Pat::Wildcard`]. It is the IR
    /// shape M3b-1 lowers a tuple-destructuring `case` arm and a tuple function
    /// parameter to (`fst (a, b) = a` → a synthetic param plus
    /// `Destructure { (a, b) = arg } a`). The binder must be irrefutable — the
    /// lowerer is the sole producer and rejects a refutable element
    /// (a constructor / literal) fail-closed (SKY-L0115) — so the backend's
    /// `let <binder> = <value>;` is a sound, exhaustive Rust binding. `binder`
    /// is bound only within `body`, not in `value`.
    Destructure {
        binder: Pat,
        value: Box<Self>,
        body: Box<Self>,
    },
    /// A conditional `if cond then then_ else else_`. The `else` arm is
    /// mandatory — every Sky `if` is an expression with both branches.
    If {
        cond: Box<Self>,
        then_: Box<Self>,
        else_: Box<Self>,
    },
    Match(Match),
    Call {
        callee: Callee,
        args: Vec<Self>,
        /// A turbofish pin for a polymorphic kernel whose free result type
        /// parameter the HM solver left GENUINELY UNCONSTRAINED at this call
        /// site (a discarded / empty / phantom position). Without it the
        /// emitted Rust hits `E0282`/`E0283` "type annotations needed" — the
        /// SEAL-violating exit-0-then-cargo-fail class.
        ///
        /// The lowerer sets a non-[`CallPin::None`] value ONLY when the
        /// relevant parameter's solved type is a free type variable that is
        /// NOT bound by an enclosing generic function's signature (which would
        /// already pin it, making an added turbofish a conflict). When the
        /// type is concrete, rustc infers it from the argument / context, so
        /// the pin stays [`CallPin::None`] and emission is byte-identical to
        /// the pre-#181 output. Every IR→IR rewrite that reconstructs a `Call`
        /// MUST preserve this field.
        pin: CallPin,
    },
    /// A tuple constructor `(e1, e2, ...)`.
    ///
    /// Invariant: the element list has arity ≥ 2 — a 0-tuple is the unit value
    /// and a 1-tuple is just its element, so neither is a `Tuple`. The lowerer
    /// upholds this; the backend remains total over any vector (it never panics
    /// on a degenerate arity).
    Tuple(Vec<Self>),
    /// A list literal `[]` / `[e1, e2, …]`. `elem` is the element [`IrType`]
    /// (recorded so the empty list renders with a concrete `Vec::<T>::new()`);
    /// `items` are the element expressions in source order. Renders as a Rust
    /// `vec![…]` (or a typed `Vec::new()` when empty).
    List {
        elem: IrType,
        items: Vec<Self>,
    },
    /// A cons `head :: tail` — prepend one element to a list. Renders through the
    /// runtime's `sky_list_cons(head, tail)`, the move-only list prepend.
    Cons {
        head: Box<Self>,
        tail: Box<Self>,
    },
    /// Clone the element at a CONSTANT index of a list value: `<list>[<index>].clone()`.
    ///
    /// Introduced for Class 4 item C2. A cons / list sub-pattern nested in
    /// a constructor payload lowers to a fresh `Vec` binder plus an arm-level
    /// length GUARD ([`Arm::guard`]); the named head elements are then recovered
    /// in the arm-body prelude via this node, one per prefix position. It is ONLY
    /// ever emitted where the arm's guard has already proven `list.len() > index`,
    /// so the Rust index is in bounds by construction (never a panic on
    /// well-typed source — the guard falls through to the next arm otherwise).
    /// The `.clone()` keeps the original list intact for the sibling tail binder
    /// (`List.drop`), mirroring the `rebind_clone` the top-level slice path uses.
    ListIndexClone {
        list: Box<Self>,
        index: usize,
    },
    /// A borrowing list-length CHECK for a Class 4 item C2 arm guard:
    /// `<list>.len() >= <len>` (`exact == false`, an OPEN cons chain
    /// `a :: b :: rest`) or `<list>.len() == <len>` (`exact == true`, a CLOSED
    /// list literal `[a, b]`). `.len()` borrows the bound `Vec` — a match guard
    /// may not MOVE out of a binding, so this is deliberately NOT the consuming
    /// `List.length` kernel. Only ever emitted in [`Arm::guard`] position.
    ListLenCheck {
        list: Box<Self>,
        len: usize,
        exact: bool,
    },
    /// A record literal `{ x = e1, y = e2, ... }`.
    ///
    /// The fields are carried as `(field name, value)` pairs sorted by field
    /// name, so the construction is deterministic. The backend resolves the
    /// literal's synthesised Rust struct from its field-name set; Rust names its
    /// struct-literal fields, so the emitted construction is order-independent.
    Record(Vec<(Symbol, Self)>),
    /// A record field access `record.field`. `field_ty` is the field's own
    /// solved type — carried so the Rust backend can decide, WITHOUT any
    /// textual heuristic, whether the read needs a `.clone()` (a heap-backed
    /// field) or can skip it (a Rust-`Copy` scalar) — see #142/AUD-09's
    /// type-directed Copy-elision fix,
    /// `docs/adr/0011-emitter-clone-borrow-discipline.md` §3.
    /// When the lowerer cannot resolve a concrete field type (a still-generic
    /// field inside a polymorphic body), it falls back to
    /// [`IrType::Generic`], which the backend classifies as non-`Copy` and
    /// therefore conservatively KEEPS the `.clone()`.
    Access {
        record: Box<Self>,
        field: Symbol,
        field_ty: IrType,
    },
    /// A record update `{ record | x = e1, ... }`: a copy of `record` with the
    /// listed fields replaced. `fields` lists only the changed fields, as
    /// `(field name, new value)` pairs.
    Update {
        record: Box<Self>,
        fields: Vec<(Symbol, Self)>,
    },
    /// An anonymous function `\p0 p1 ... -> body`: typed parameters, a return
    /// type, and a body expression.
    ///
    /// Distinct from [`Func`] (a named top-level declaration): a `Lambda` is an
    /// expression value. The backend emits it as a boxed closure
    /// `Box::new(move |p0: T0, ...| -> R { body })`, move-capturing any free
    /// locals. A zero-parameter lambda is a genuine nullary closure.
    Lambda {
        params: Vec<(Symbol, IrType)>,
        ret: IrType,
        body: Box<Self>,
    },
    /// A LET-BOUND closure literal that must be reference-counted (`Arc`)
    /// rather than uniquely owned (`Box`), because the lowerer's capture
    /// analysis proved the bound symbol is captured-by-move into more than
    /// one closure environment along some nesting chain (see
    /// `sky_lower::needs_shared_capture`).
    ///
    /// SEAL fix (#164 E0507 pair): `examples/18-job-queue`'s
    /// `withErrorReporting` (and its `saveSnapshot`/`loadHistory` siblings)
    /// let-bind a local closure (`logAndFail`, `insertRow`, `selectRecent`)
    /// that is referenced from INSIDE another, more-deeply-nested closure --
    /// e.g. `report e = Crypto.randomToken 4 |> Task.andThen (\errId ->
    /// logAndFail e errId)`. Each `move` closure independently move-captures
    /// its free locals: the outer `report` closure must own `logAndFail` to
    /// hand it to the inner `\errId -> ...` closure, which ALSO move-captures
    /// it to call it. A `Box<dyn Fn>` is not `Clone`, so the inner capture's
    /// move is illegal against the outer closure's `&self`-borrowed field --
    /// `cannot move out of ... a captured variable in an Fn closure` (E0507).
    /// The lowerer's PER-LAMBDA capture classification (`lower_lambda`) has
    /// no visibility into ANCESTOR closures' captures, so each closure's own
    /// depth-0 bare-callee exemption fires independently and unsoundly.
    ///
    /// Rendering: the backend emits `Arc::new(move |p0: T0, ...| -> R { body
    /// })` pinned to `Arc<dyn Fn(T0, ...) -> R + Send + Sync + 'static>`
    /// (`emit_shared_lambda`, mirroring [`Self::Lambda`]'s `emit_lambda` but
    /// with the `+ Sync` bound Arc needs to itself be `Send + Sync`). Every
    /// read of the bound symbol at lambda-nesting depth >= 1 relative to its
    /// binding is rewritten to [`Self::CloneVar`] (`Arc::clone`, cheap
    /// pointer bump) by `sky_lower::force_shared_capture_clones`; a depth-0
    /// (non-nested) read stays a bare [`Self::Var`] -- calling through an
    /// `Arc<dyn Fn>` auto-derefs exactly like `Box<dyn Fn>`, no clone needed.
    ///
    /// Produced ONLY by `sky_lower::lower_let`'s `PVar` arm, for a
    /// function-typed (`IrType::Fun`) let-binding whose capture pre-pass
    /// fires. Never appears anywhere else (never a `Match` scrutinee, never
    /// a def-head body, never a bare call-arg lambda) -- the type system
    /// still models it as an ordinary [`IrType::Fun`]; only its OWN Rust
    /// pointer representation differs.
    SharedLambda {
        params: Vec<(Symbol, IrType)>,
        ret: IrType,
        body: Box<Self>,
    },
    /// Application of an arbitrary expression value to arguments, `func(args)`.
    ///
    /// Distinct from [`Expr::Call`], which targets a known [`Callee`] (a direct
    /// top-level function or a kernel) and keeps the efficient direct-call path.
    /// `Apply` calls a first-class function *value* — a lambda, a
    /// function-typed parameter/binding, or a top-level function passed as a
    /// value — and renders as `(func)(args)` (a boxed `dyn Fn` auto-derefs).
    Apply {
        func: Box<Self>,
        args: Vec<Self>,
    },
    /// A top-level function or kernel named as a first-class *value* — passed as
    /// an argument, returned, or let-bound — rather than directly called.
    ///
    /// Distinct from [`Expr::Call`] (which applies a known [`Callee`] to
    /// arguments on the spot): `FuncValue` reifies the callee into a boxed
    /// closure value so it fills a `Box<dyn Fn(..) -> R>` slot uniformly. The
    /// backend emits `{ let f: <ty> = Box::new(<callee>); f }`, the explicit
    /// binding type pinning the unsized coercion of the top-level `fn` item (a
    /// zero-sized `Fn` implementor) to the boxed trait object. `ty` is the
    /// value's flattened [`IrType::Fun`], recorded by the lowerer from the
    /// reference's solved region type. A direct call keeps the efficient
    /// [`Expr::Call`] path; only a bare value reference becomes a `FuncValue`.
    FuncValue {
        callee: Callee,
        ty: IrType,
    },
    /// Force-and-sequence a Task effect, discarding its result, then continue
    /// with `rest`. Produced by `lower_let` when a `let _ = <task>` binding
    /// discards a Task-typed value; the backend emits
    /// `task_and_then(Box::new(move |_: ()| -> SkyTask<()> { <rest> }), <effect>)`.
    /// This is the auto-force fix (F1): without `TaskSeq`, the future would be
    /// silently dropped unawaited.
    TaskSeq {
        effect: Box<Self>,
        rest: Box<Self>,
    },
    /// Sync variant of [`Self::TaskSeq`]: blocks on `effect` synchronously
    /// (discarding its result), then continues with `rest`. Produced by
    /// `lower_let` when a `let _ = <task>` binding discards a Task-typed value
    /// inside a **non-Task** (sync) function. The backend emits
    /// `{ let _ = task_run(effect); rest }`. This avoids the E0308 type-mismatch
    /// that arises when `task_and_then(...)` (returning `SkyTask<E,B>`) is placed
    /// in a context whose expected type is not a Task.
    TaskSeqSync {
        effect: Box<Self>,
        rest: Box<Self>,
    },
    /// A tail-recursive function body wrapped for loop emission. Produced ONLY by
    /// the lowerer's TCO rewrite (`sky_lower::rewrite_tail_calls`); `params` are
    /// the enclosing [`Func`]'s parameters (name + type) so emission can shadow
    /// them `let mut`. Invariant: `body` contains ≥ 1 [`Self::TailRecur`] in tail
    /// position and no self-[`Self::Call`] to the enclosing [`FuncId`] remains.
    /// Consumed by the Rust backend's `emit_func` / `emit_expr_tail`; reaching one
    /// on the ordinary value-emit path is a compiler bug, surfaced fail-closed
    /// (never a panic).
    TailLoop {
        params: Vec<(Symbol, IrType)>,
        body: Box<Self>,
    },
    /// A tail self-call rewritten to a loop jump. `args` are the next-iteration
    /// argument expressions, one per enclosing [`Self::TailLoop`] parameter, in the
    /// same order. Invariant: appears ONLY in tail position inside a `TailLoop`,
    /// and `args.len() == TailLoop.params.len()`.
    TailRecur {
        args: Vec<Self>,
    },
}

/// The target of a [`Expr::Call`].
#[derive(Clone, PartialEq, Eq, Debug, serde::Serialize, serde::Deserialize)]
pub enum Callee {
    Func(FuncId),
    Kernel(KernelFn),
}

/// A per-call-site turbofish pin for a polymorphic kernel.
///
/// Set when the HM solver left the kernel's free result type parameter
/// genuinely unconstrained. Each variant names the SEMANTIC default the emitter
/// renders; the mapping to a concrete `::<…>` suffix lives in the backend
/// (`emit_expr`), so this enum stays a small, typed decision — never a raw
/// string in the IR.
///
/// The lowerer only ever emits a non-[`Self::None`] variant when the free
/// parameter is a bare type variable NOT bound by an enclosing generic
/// signature; a concrete parameter needs no pin (rustc infers it) and stays
/// [`Self::None`], keeping emission byte-identical to the pre-#181 output.
#[derive(Clone, Copy, PartialEq, Eq, Debug, serde::Serialize, serde::Deserialize, Default)]
pub enum CallPin {
    /// No turbofish — the common case (rustc infers every type parameter).
    #[default]
    None,
    /// A single free element/value type defaulted to `i64` — `::<i64>`.
    /// Used by `list_head` / `list_tail` / `set_empty` and the `task_fail`
    /// main-crate wrapper (`fn task_fail<A>(…) -> SkyTask<A>`, error already
    /// pinned to `SkyError`).
    DefaultI64,
    /// A free key AND value defaulted to a String-keyed i64-valued map —
    /// `::<String, i64>`. Used by `dict_empty` (`fn dict_empty<K, V>()`).
    DefaultDict,
    /// Two inferred leading parameters and a trailing free type defaulted to
    /// `i64` — `::<_, _, i64>`. Used by `sky_result_map_error<E, F, A>` where
    /// the `Ok` type `A` is discarded (the value comes only from an `Err`).
    DefaultResultMapErr,
    /// A single free error/phantom parameter pinned to the project's canonical
    /// error type — `::<SkyError>`. Used by `decimal_from_string<E: From<String>>`
    /// when the `Err` channel is discarded.
    ErrSkyError,
}

impl CallPin {
    /// The turbofish suffix this pin renders immediately after a kernel's name
    /// (before its `(` argument list): `dict_empty::<String, i64>(…)`, etc.
    /// [`Self::None`] renders the empty string, so an unpinned call is emitted
    /// byte-identically to the pre-#181 output.
    ///
    /// The concrete default types (`i64` / `String` / `SkyError`) mirror the
    /// Go/Haskell reference's polymorphic-kernel defaults: a genuinely
    /// unconstrained parameter has no observable effect on behaviour (the value
    /// is discarded / the collection is empty / the task never yields), so any
    /// inhabited default is sound — `i64` and `String` are the reference's
    /// canonical choices, `SkyError` the project's canonical error type.
    #[must_use]
    pub const fn turbofish(self) -> &'static str {
        match self {
            Self::None => "",
            Self::DefaultI64 => "::<i64>",
            Self::DefaultDict => "::<String, i64>",
            Self::DefaultResultMapErr => "::<_, _, i64>",
            Self::ErrSkyError => "::<SkyError>",
        }
    }
}

/// Every stdlib kernel function known to the Sky compiler.
///
/// **Phase A type alias.** The alias keeps every downstream call-site
/// (`Callee::Kernel(KernelFn)`, `k.is_db()`, `KernelFn::*` variant patterns)
/// compiling unchanged.  `sky_kernels::StdlibKernel` is the single source of
/// truth; `sky_ir` re-exports it under the `KernelFn` alias.
///
/// See [`sky_kernels::StdlibKernel`] for the full variant list and
/// [`sky_kernels::StdlibKernel::decl`] for per-variant metadata.
pub type KernelFn = sky_kernels::StdlibKernel;

/// Re-export of the `Std.Html.Events` payload-shape ADT, so backend
/// crates that already depend on `sky_ir` (for `KernelFn`) can match on it
/// without taking a direct `sky_kernels` dependency.
pub use sky_kernels::HtmlEventShape;

/// Binary operators.
///
/// M0 shipped `Add`/`Sub`; M1 core widens the set with the remaining
/// arithmetic, comparison, and boolean operators. `Append` (`++`) carries
/// string concatenation; list `++` and `::` are deferred until the list type
/// lands.
#[derive(Clone, Copy, PartialEq, Eq, Debug, serde::Serialize, serde::Deserialize)]
pub enum BinOp {
    Add,
    Sub,
    Mul,
    Div,
    Eq,
    Neq,
    Lt,
    Gt,
    Le,
    Ge,
    And,
    Or,
    /// Integer division `//`. Has no sound Rust infix form: raw `/` on `i64`
    /// panics on `b == 0` (divide by zero) **and** on `i64::MIN / -1`
    /// (signed overflow); `//` is a Rust line comment, so it cannot be
    /// emitted literally. The backend routes this variant through the total
    /// helper `sky_runtime::math::sky_int_div(l, r)`, never via `op_str`.
    IntDiv,
    /// String append `++`. Unlike the infix arithmetic/comparison operators,
    /// this has no single Rust infix form for two `String`s, so the backend
    /// emits it as a `format!` concatenation rather than via `op_str`.
    Append,
}

/// One arm of a [`Match`]: a constructor pattern and the body it guards.
//
// `Eq` is not derived: `body` is an [`Expr`], only `PartialEq` (float literals).
#[derive(Clone, PartialEq, Debug, serde::Serialize, serde::Deserialize)]
pub struct Arm {
    pub pat: Pat,
    pub body: Expr,
    /// An optional boolean guard evaluated after `pat` matches; `false` falls
    /// through to the next arm (native Rust `match` guard semantics). `None`
    /// for every pre-existing arm shape (byte-identical emission — the backend
    /// renders `{pat} => …` unchanged, only adding `if {guard}` when `Some`).
    ///
    /// Introduced for Class 4 item C2 — a cons / list sub-pattern nested
    /// in a constructor payload lowers to a plain [`Pat::Var`] binder for that
    /// position PLUS a guard checking the bound `Vec`'s length, with the named
    /// sub-bindings (`h`, `t`) recovered via indexing / slicing in the arm
    /// body's prelude rather than embedded in the pattern itself (Rust cannot
    /// slice-pattern a `Vec<T>` ENUM FIELD inline — only an actual slice/array,
    /// which needs a scrutinee-level `.as_slice()` coercion this nested position
    /// does not have).
    pub guard: Option<Expr>,
}

impl Arm {
    /// A guard-free arm — the default shape for every pattern that discriminates
    /// entirely in its `Pat`. Keeps the ~40 existing `Arm { pat, body }` call
    /// sites concise while the `guard` field defaults to `None`.
    #[must_use]
    pub const fn new(pat: Pat, body: Expr) -> Self {
        Self {
            pat,
            body,
            guard: None,
        }
    }
}

/// A pattern.
///
/// M3a supports a constructor pattern whose payload sub-patterns bind to a
/// variable ([`Pat::Var`]) or are ignored ([`Pat::Wildcard`]). Nullary
/// constructor patterns are [`Pat::Ctor`] with an empty `args`. M3b-1 adds
/// the tuple pattern [`Pat::Tuple`]. M3b-2 adds the record pattern
/// [`Pat::Record`] and makes every sub-position fully recursive: ANY [`Pat`] may
/// appear as a constructor payload, a tuple element, or a record-field
/// sub-pattern (`Just (a, b)`, `Node (Node …) x r`, `{ point = (a, b) }`).
///
/// [`Pat::Var`] / [`Pat::Wildcard`] and the literal leaves [`Pat::Int`] /
/// [`Pat::Bool`] / [`Pat::Char`] / [`Pat::Str`] are leaves; [`Pat::Ctor`] /
/// [`Pat::Tuple`] / [`Pat::Record`] / [`Pat::Alias`] are nesting nodes whose
/// sub-patterns reuse the same enum, recursively. The var / wildcard /
/// alias-of-irrefutable shapes also serve as an irrefutable destructuring binder
/// (a single irrefutable case arm, a function parameter, or a `let`-destructure)
/// when every leaf is a var / wildcard.
///
/// M3b-3 adds the refutable literal leaves ([`Pat::Int`], [`Pat::Bool`],
/// [`Pat::Char`], [`Pat::Str`]) and the alias / `as` binder ([`Pat::Alias`]).
/// Cons / list patterns remain M4 and are rejected upstream at lowering.
#[derive(Clone, PartialEq, Eq, Debug, serde::Serialize, serde::Deserialize)]
pub enum Pat {
    /// A variable binder — binds the matched value (a constructor payload field)
    /// to a name.
    Var(Symbol),
    /// A wildcard `_` — matches any value and binds nothing.
    Wildcard,
    /// An integer literal pattern `0`, `42`, `-1`. Refutable. Renders as the Rust
    /// integer literal of the same value.
    Int(i64),
    /// A boolean literal pattern `True` / `False`. Refutable in isolation but a
    /// `True` + `False` pair is an exhaustive cover of `Bool`. Renders as the Rust
    /// `true` / `false` literal.
    Bool(bool),
    /// A character literal pattern `'a'`. The carried [`String`] is the source
    /// character text (a single grapheme in well-formed IR); the backend renders
    /// it as a Rust `char` literal. Refutable.
    Char(String),
    /// A string literal pattern `"hello"`. The carried [`String`] is the literal's
    /// value; the backend renders it as a Rust string literal with deterministic
    /// escaping. Refutable.
    Str(String),
    /// An alias / `as` pattern `inner as name` — matches `inner` and additionally
    /// binds the whole matched value to `name`. Renders as the Rust binding-with-
    /// subpattern form `name @ <inner>`. The inner sub-pattern is an arbitrary
    /// [`Pat`] and recurses.
    Alias(Box<Self>, Symbol),
    /// A constructor pattern `Variant sub0 sub1 …` (a nullary pattern `Variant`
    /// has an empty `args`). Each `args` element is an arbitrary [`Pat`] (M3b-2:
    /// nested ctor / tuple / record sub-patterns are all permitted). `home` + `ty`
    /// are the scrutinee enum's nominal identity (see [`EnumDef::home`]).
    Ctor {
        home: ModPath,
        ty: Symbol,
        variant: Symbol,
        args: Vec<Self>,
    },
    /// A tuple pattern `(p0, p1, …)`, destructuring an [`IrType::Tuple`] value
    /// element-by-element.
    ///
    /// The element sub-patterns are arbitrary [`Pat`]s. The tuple-value invariant
    /// (arity ≥ 2) applies to well-formed IR — the lowerer is the sole producer
    /// and upholds it — but the backend stays total over any element vector it
    /// receives and never panics on a degenerate arity.
    Tuple(Vec<Self>),
    /// A record pattern `{ field0 = p0, field1 = p1, … }`, destructuring an
    /// [`IrType::Record`] value field-by-field.
    ///
    /// Each entry pairs a field name ([`Symbol`]) with its sub-pattern (an
    /// arbitrary [`Pat`]). The lowerer is contracted to surface the COMPLETE
    /// field set of the record type — every field the type declares appears here,
    /// a field the source omits binding to a [`Pat::Wildcard`] — so the field-name
    /// set resolves the synthesised struct unambiguously, exactly as a record
    /// literal does. The backend stays total over any entry vector it receives.
    Record(Vec<(Symbol, Self)>),
    /// A list / cons pattern, flattened to a Rust slice-pattern shape: a
    /// `prefix` of fixed leading element sub-patterns plus an optional `rest`
    /// tail binder.
    ///
    /// * `rest = None` is a CLOSED, exact-length list pattern — `[]`
    ///   (`prefix` empty) or `[a, b]` (`prefix` = `[a, b]`). It matches only a
    ///   list of exactly `prefix.len()` elements.
    /// * `rest = Some(p)` is an OPEN cons tail — `x :: xs` (`prefix` = `[x]`,
    ///   `rest` = `xs`) or `a :: b :: rest` (`prefix` = `[a, b]`, `rest` =
    ///   `rest`). It matches any list with AT LEAST `prefix.len()` elements; `p`
    ///   binds the remaining list (a variable / wildcard / alias).
    ///
    /// The element sub-patterns (`prefix`) and the tail binder (`rest`) are
    /// arbitrary [`Pat`]s and recurse. The List type is the closed two-constructor
    /// type `Nil | Cons`, so a `[]` arm plus an `_ :: _`-shaped arm is an
    /// exhaustive cover; coverage over the flattened shape is the type phase's
    /// usefulness check (SKY-T0010), proven before lowering. The backend renders
    /// this directly as a Rust slice pattern (`[p0, p1]` / `[p0, p1, rest @ ..]`).
    Slice {
        prefix: Vec<Self>,
        rest: Option<Box<Self>>,
    },
}

/// Whether a pattern matches EVERY value of its scrutinee type.
///
/// True for a wildcard, a variable binder, or an alias whose inner pattern is
/// itself irrefutable. Used to prove a flat `match`'s trailing arm is a genuine
/// catch-all.
#[must_use]
pub fn is_irrefutable(pat: &Pat) -> bool {
    match pat {
        Pat::Wildcard | Pat::Var(_) => true,
        Pat::Alias(inner, _) => is_irrefutable(inner),
        Pat::Int(_)
        | Pat::Bool(_)
        | Pat::Char(_)
        | Pat::Str(_)
        | Pat::Ctor { .. }
        | Pat::Tuple(_)
        | Pat::Record(_)
        // A slice / cons pattern is refutable: `[]` matches only the empty list,
        // and `[x, rest @ ..]` matches only a non-empty one. (The lowerer never
        // produces an empty-`prefix` open `[rest @ ..]`, which would be the lone
        // irrefutable slice shape — a whole-list binder stays a [`Pat::Var`].)
        | Pat::Slice { .. } => false,
    }
}

/// Whether this pattern needs NO Rust-level runtime dispatch (discriminant
/// check) anywhere in its shape — no [`Pat::Ctor`], literal leaf, or
/// [`Pat::Slice`] at any depth.
///
/// A `Var` / `Wildcard` leaf, or any nesting of `Tuple` / `Record` /
/// `Alias` over such leaves, is dispatch-free: Rust's tuple/struct/binding
/// patterns always succeed structurally, so matching them costs no
/// discriminant check and (unlike [`is_irrefutable`], which answers a
/// different question about catch-all arms and treats `Tuple`/`Record` as
/// unconditionally refutable) is safe to evaluate at ANY nesting depth,
/// including inside another constructor's payload.
///
/// Used by the Rust backend (`render_arm_pat_alias_safe`) to decide whether
/// an `Alias` node's inner shape can be safely rebuilt from a CLONE of the
/// alias binder (the #96/#99 by-value alias-split fix) — safe exactly when
/// reconstructing every leaf `inner` binds is possible without having
/// discarded any data, which holds only when `inner` never needed a runtime
/// check to get there. The `sky_lower` lowerer calls this too, to reject an
/// alias over a dispatch-needing inner pattern in a REFUTABLE match-arm
/// position (SKY-L0128) rather than let it reach the backend, where
/// honoring it soundly would require matching the scrutinee by reference
/// throughout — a materially larger redesign. See
/// `docs/adr/0011-emitter-clone-borrow-discipline.md` §1.
#[must_use]
pub fn is_dispatch_free(pat: &Pat) -> bool {
    match pat {
        Pat::Wildcard | Pat::Var(_) => true,
        Pat::Alias(inner, _) => is_dispatch_free(inner),
        Pat::Tuple(elems) => elems.iter().all(is_dispatch_free),
        Pat::Record(fields) => fields.iter().all(|(_, p)| is_dispatch_free(p)),
        Pat::Int(_)
        | Pat::Bool(_)
        | Pat::Char(_)
        | Pat::Str(_)
        | Pat::Ctor { .. }
        | Pat::Slice { .. } => false,
    }
}

/// Whether a pattern is LIST-SHAPED — a slice / cons pattern ([`Pat::Slice`]), an
/// irrefutable whole-list binder (a variable / wildcard catch-all over a list
/// scrutinee), or an alias whose inner pattern is itself list-shaped. Used by
/// [`Match::new_flat`] to recognise a list `case` (whose `Nil | Cons` coverage
/// the upstream Maranget check already proved) as a structurally-exhaustive arm
/// set, distinct from a constructor / literal cover.
#[must_use]
pub fn is_list_shaped(pat: &Pat) -> bool {
    match pat {
        Pat::Slice { .. } | Pat::Wildcard | Pat::Var(_) => true,
        Pat::Alias(inner, _) => is_list_shaped(inner),
        Pat::Int(_)
        | Pat::Bool(_)
        | Pat::Char(_)
        | Pat::Str(_)
        | Pat::Ctor { .. }
        | Pat::Tuple(_)
        | Pat::Record(_) => false,
    }
}

/// Whether a pattern's HEAD is a constructor — a [`Pat::Ctor`] directly, or an
/// alias (`name @ <inner>`) whose inner pattern is itself constructor-headed.
/// Used by [`Match::new_flat`] to recognise a constructor-discrimination arm set
/// (where coverage is proven by the upstream enum exhaustiveness check) as a
/// distinct case from an open-literal cover (which needs a trailing catch-all).
#[must_use]
pub fn is_ctor_headed(pat: &Pat) -> bool {
    match pat {
        Pat::Ctor { .. } => true,
        Pat::Alias(inner, _) => is_ctor_headed(inner),
        Pat::Wildcard
        | Pat::Var(_)
        | Pat::Int(_)
        | Pat::Bool(_)
        | Pat::Char(_)
        | Pat::Str(_)
        | Pat::Tuple(_)
        | Pat::Record(_)
        | Pat::Slice { .. } => false,
    }
}

/// Whether a pattern is PRODUCT-SHAPED — a tuple pattern ([`Pat::Tuple`]), an
/// irrefutable whole-tuple binder (a variable / wildcard catch-all over a tuple
/// scrutinee), or an alias whose inner pattern is itself product-shaped. Used by
/// [`Match::new_flat`] to recognise a multi-arm tuple `case` (whose product
/// coverage the upstream Maranget check already proved) as a structurally-
/// exhaustive arm set, distinct from a constructor / literal / list cover.
#[must_use]
pub fn is_product_shaped(pat: &Pat) -> bool {
    match pat {
        Pat::Tuple(_) | Pat::Wildcard | Pat::Var(_) => true,
        Pat::Alias(inner, _) => is_product_shaped(inner),
        Pat::Int(_)
        | Pat::Bool(_)
        | Pat::Char(_)
        | Pat::Str(_)
        | Pat::Ctor { .. }
        | Pat::Record(_)
        | Pat::Slice { .. } => false,
    }
}

/// An exhaustive case analysis over an enum scrutinee.
///
/// Fields are private: the sole way to obtain a `Match` is [`Match::new`],
/// which proves exhaustiveness at construction time. This makes a
/// non-exhaustive `Match` unrepresentable.
//
// `Eq` is not derived: the scrutinee / arm bodies are [`Expr`]s that may carry
// a float literal (only `PartialEq`).
#[derive(Clone, PartialEq, Debug)]
pub struct Match {
    scrutinee: Box<Expr>,
    arms: Vec<Arm>,
}

/// Serialises as the plain `{ scrutinee, arms }` shape via the public
/// accessors — no invariant to preserve on the way OUT (any value that
/// exists has already been validated by [`Match::new`]/[`Match::new_flat`]
/// at construction time).
impl serde::Serialize for Match {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        #[derive(serde::Serialize)]
        struct MatchRef<'a> {
            scrutinee: &'a Expr,
            arms: &'a [Arm],
        }
        MatchRef {
            scrutinee: self.scrutinee(),
            arms: self.arms(),
        }
        .serialize(serializer)
    }
}

/// **Deliberately hand-written, never `#[derive(Deserialize)]`** — `Match`'s
/// fields are private specifically because the sole way to obtain one is
/// [`Match::new`]/[`Match::new_flat`], which prove the arm set is
/// structurally exhaustive at construction time (see the type's own doc). A
/// derived impl would reconstruct `Match { scrutinee, arms }` directly from
/// untrusted bytes, bypassing that proof entirely — the exact same "parse,
/// don't validate" gap `sky_backend::RelPath`'s hand-written `Deserialize`
/// closes for path-traversal.
///
/// This impl re-validates through [`Match::new_flat`] rather than
/// re-deriving [`Match::new`]'s stricter ctor-exhaustive-cover check,
/// because `new_flat`'s NECESSARY-condition backstop (trailing catch-all /
/// complete `Bool` cover / all-constructor-headed / all-list-shaped /
/// all-product-shaped) is a **provable superset** of what `new` guarantees:
/// every arm `Match::new` accepts has `Pat::Ctor` as its literal arm head
/// (`new`'s own hard requirement — a non-`Ctor` head is rejected before the
/// exhaustiveness check even runs), so `is_ctor_headed` holds for every arm
/// and `new_flat`'s `all_ctor_headed` branch always accepts it. A `Match`
/// built via `new_flat` trivially re-validates through the same function
/// (pure, deterministic over the same arms). So EVERY legitimately
/// constructed `Match` in the whole compiler round-trips through this
/// impl unchanged, while an EMPTY arm list or an open-literal cover with no
/// trailing catch-all is rejected exactly as it would be at original
/// construction time.
///
/// **Honestly scoped gap.** `new_flat`'s `all_ctor_headed` branch (unlike
/// `Match::new` itself) does not re-verify that the ctor-headed arms cover
/// EVERY variant of the scrutinee's enum — `Match` carries no external
/// "complete variant set" of its own to check against (that list lives on
/// the `EnumDef` elsewhere in the `Program`, not on `Match`), and `new_flat`
/// deliberately trusts the upstream Maranget check for that shape (see its
/// own doc). So a tampered entry that DROPS one arm from an otherwise-
/// exhaustive ctor cover (while keeping every remaining arm ctor-headed)
/// is NOT caught here. This is not a silent-corruption or RCE risk: the
/// missing-variant gap surfaces the moment the relocated `Program` reaches
/// `RustBackend::emit` as a plain Rust `match` with a missing arm, which
/// `cargo build` rejects with E0004 — a loud, safe failure, never wrong
/// output. Closing it fully would require deserializing the WHOLE
/// `Program` first and cross-checking every `Match` against its scrutinee
/// enum's `EnumDef` in a second pass — recorded here as a possible future
/// hardening, not attempted because the current gap already fails safe.
impl<'de> serde::Deserialize<'de> for Match {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        #[derive(serde::Deserialize)]
        struct RawMatch {
            scrutinee: Expr,
            arms: Vec<Arm>,
        }
        let raw = RawMatch::deserialize(deserializer)?;
        Self::new_flat(raw.scrutinee, raw.arms)
            .map_err(|diag| serde::de::Error::custom(format!("{diag:?}")))
    }
}

impl Match {
    /// Build a constructor-headed `Match` from an ORDERED arm list.
    ///
    /// `variants` is the complete set of constructors of the scrutinee's enum.
    /// Every arm head is a constructor pattern, and the same top-level
    /// constructor MAY appear in more than one arm — those arms discriminate on
    /// their nested sub-patterns (`Som (Som x)`, `Som Non`, `Non`) and Rust's
    /// `match` resolves the overlap and ordering natively. Arms are kept in
    /// source order; the renderer emits them one-to-one.
    ///
    /// Exhaustiveness over the nested shape is proven UPSTREAM by the type
    /// phase's usefulness/Maranget analysis (SKY-T0010), which runs before
    /// lowering, so a non-exhaustive `case` never reaches this constructor. The
    /// check here is a cheap NECESSARY-condition backstop only: every variant of
    /// the enum must appear as some arm's top constructor, and no arm may name a
    /// constructor outside the enum. A variant wholly absent from the top
    /// constructors guarantees non-exhaustiveness, so it is a genuine internal
    /// invariant violation; duplicate top constructors are the normal nested-
    /// discrimination shape and are accepted.
    ///
    /// # Errors
    ///
    /// Returns [`Diagnostic::CompilerBug`] when an arm head is not a constructor,
    /// an arm names a constructor not in `variants`, or some variant is missing
    /// from the top constructors — each an internal invariant violation the
    /// lowerer must never produce.
    pub fn new(scrutinee: Expr, arms: Vec<Arm>, variants: &[Symbol]) -> DResult<Self> {
        let expected: BTreeSet<Symbol> = variants.iter().copied().collect();

        let mut covered: BTreeSet<Symbol> = BTreeSet::new();
        for arm in &arms {
            // The case-arm head is always a constructor pattern (payload binders
            // are sub-patterns). A bare variable / wildcard whole-scrutinee arm
            // routes through `new_flat`, so a non-ctor arm head here is an
            // internal invariant violation, surfaced as a `CompilerBug` rather
            // than silently skewing the coverage set.
            let Pat::Ctor { variant, .. } = &arm.pat else {
                return Err(Diagnostic::CompilerBug {
                    where_: "sky_ir::Match::new",
                    detail: "match arm head is not a constructor pattern".to_owned(),
                });
            };
            let variant = *variant;
            if !expected.contains(&variant) {
                return Err(Diagnostic::CompilerBug {
                    where_: "sky_ir::Match::new",
                    detail: format!(
                        "match arm covers variant {} not in the scrutinee's enum",
                        variant.as_raw()
                    ),
                });
            }
            // A repeated top constructor is the nested-discrimination shape
            // (`Som (Som x)` then `Som Non`); the set insert ignores the repeat.
            covered.insert(variant);
        }

        if covered != expected {
            return Err(Diagnostic::CompilerBug {
                where_: "sky_ir::Match::new",
                detail: format!(
                    "non-exhaustive match: top constructors cover {} of {} variants",
                    covered.len(),
                    expected.len()
                ),
            });
        }

        Ok(Self {
            scrutinee: Box::new(scrutinee),
            arms,
        })
    }

    /// Build a FLAT refutable `match` from an ORDERED arm list whose heads are
    /// literals (`0` / `'a'` / `"hi"` / `True` / `False`), wildcards / variables,
    /// alias binders, or constructors, in any mix. Arms are kept in source order;
    /// the renderer emits them one-to-one, so several arms may discriminate on the
    /// same top-level constructor via their nested sub-patterns.
    ///
    /// Unlike [`Match::new`] (the all-constructor path), this path admits open
    /// literal types (`Int` / `Char` / `String`) whose coverage cannot be proven
    /// from a finite variant set. Exhaustiveness is therefore proven UPSTREAM by
    /// the type phase's usefulness/Maranget analysis (SKY-T0010), which runs
    /// before lowering, so a non-exhaustive `case` never reaches this constructor.
    ///
    /// The backstop here is a cheap NECESSARY-condition check — the arm set is
    /// accepted when it is structurally guaranteed to cover its scrutinee:
    ///
    /// * a trailing IRREFUTABLE arm (`_`, a variable, or an alias whose inner
    ///   pattern is irrefutable) matches every remaining value, OR
    /// * the arms are a complete `Bool` cover (`True` and `False` both present), OR
    /// * every arm head is constructor-shaped (a constructor, or an alias over
    ///   one): the scrutinee is then a finite enum whose coverage the upstream
    ///   Maranget check already proved, so no open-literal gap can hide here.
    ///
    /// An arm set matching none of these (an open-literal cover with no trailing
    /// catch-all) would be genuinely non-exhaustive, so it is a `CompilerBug`
    /// rather than a Rust `match` rustc would reject with E0004.
    ///
    /// # Errors
    /// [`Diagnostic::CompilerBug`] when `arms` is empty or none of the backstop
    /// conditions hold.
    pub fn new_flat(scrutinee: Expr, arms: Vec<Arm>) -> DResult<Self> {
        let Some(last) = arms.last() else {
            return Err(Diagnostic::CompilerBug {
                where_: "sky_ir::Match::new_flat",
                detail: "flat match has no arms".to_owned(),
            });
        };
        let trailing_catch_all = is_irrefutable(&last.pat);
        let bool_complete = arms.iter().all(|a| matches!(a.pat, Pat::Bool(_)))
            && arms.iter().any(|a| matches!(a.pat, Pat::Bool(true)))
            && arms.iter().any(|a| matches!(a.pat, Pat::Bool(false)));
        // Constructor-shaped heads (a constructor, or an alias whose inner is one)
        // mean the scrutinee is a finite enum; the upstream Maranget check proved
        // its coverage, so an alias-over-constructor discrimination set with no
        // trailing catch-all (`Som (Som x) as w` then `Som Non` then `Non`) is
        // still sound here.
        let all_ctor_headed = arms.iter().all(|a| is_ctor_headed(&a.pat));
        // A list `case`: every arm is list-shaped (a slice / cons pattern, an
        // irrefutable whole-list binder, or an alias over those) and at least one
        // is a genuine slice pattern. The List type is the closed `Nil | Cons`
        // type, so a `[]` arm plus an `_ :: _`-shaped arm covers it; that coverage
        // was proven UPSTREAM by the Maranget usefulness check (SKY-T0010), so an
        // arm set in this shape with no trailing catch-all (`x :: rest` then `[]`)
        // is still sound here.
        let all_list_shaped = arms.iter().all(|a| is_list_shaped(&a.pat))
            && arms.iter().any(|a| matches!(a.pat, Pat::Slice { .. }));
        // A multi-arm tuple `case`: every arm is product-shaped (a tuple pattern,
        // an irrefutable whole-tuple binder, or an alias over those) and at least
        // one is a genuine tuple pattern. A product type is inhabited only by its
        // element tuples, so the upstream Maranget usefulness check (SKY-T0010)
        // already proved coverage before lowering — an arm set in this shape with
        // no trailing catch-all (`(True, _)` then `(False, _)`) is still sound.
        let all_product_shaped = arms.iter().all(|a| is_product_shaped(&a.pat))
            && arms.iter().any(|a| matches!(a.pat, Pat::Tuple(_)));
        if !trailing_catch_all
            && !bool_complete
            && !all_ctor_headed
            && !all_list_shaped
            && !all_product_shaped
        {
            return Err(Diagnostic::CompilerBug {
                where_: "sky_ir::Match::new_flat",
                detail: "flat match is not structurally exhaustive (no trailing \
                         catch-all, not a complete Bool cover, not a \
                         constructor-headed cover, not a list cover, and not a \
                         tuple cover)"
                    .to_owned(),
            });
        }
        Ok(Self {
            scrutinee: Box::new(scrutinee),
            arms,
        })
    }

    #[must_use]
    pub fn scrutinee(&self) -> &Expr {
        &self.scrutinee
    }

    #[must_use]
    pub fn arms(&self) -> &[Arm] {
        &self.arms
    }

    /// Rebuild this `Match` by transforming its scrutinee and every arm's
    /// body/guard, leaving every arm's PATTERN — and the arm count and order —
    /// completely untouched.
    ///
    /// [`Self::new`]/[`Self::new_flat`]'s exhaustiveness invariant is a
    /// property of the pattern SHAPES alone (see their doc comments), so a
    /// transformation that only ever touches `scrutinee`, [`Arm::body`], and
    /// [`Arm::guard`] (an expression evaluated in the arm's scope, not a
    /// pattern shape) can never invalidate it. This is the sound, sealed
    /// replacement for the former `pub fn from_parts_unchecked` escape hatch
    /// (AUD-09), which took a raw `Vec<Arm>` and could rebuild a `Match` with
    /// an empty arm list (`match x {}` — rustc E0004, no Sky diagnostic) or a
    /// reordered/dropped-arm list. `pub(crate)`-sealing was not viable
    /// instead, because every caller of the old function lived in a different
    /// crate (`sky_lower`, `sky_backend_rust`).
    ///
    /// `arm_map` receives `(pattern, body, guard)` per arm and returns the
    /// new `(body, guard)`; the pattern is read-only by construction.
    ///
    /// When a pass threads one `&mut` accumulator through BOTH the scrutinee
    /// and the arm bodies (two closures cannot share a unique borrow —
    /// E0524), rewrite the scrutinee first via [`Self::map_scrutinee`] and
    /// then map the bodies with an identity `scrutinee_map`.
    #[must_use]
    pub fn map_bodies(
        self,
        scrutinee_map: impl FnOnce(Expr) -> Expr,
        mut arm_map: impl FnMut(&Pat, Expr, Option<Expr>) -> (Expr, Option<Expr>),
    ) -> Self {
        let new_scrutinee = Box::new(scrutinee_map(*self.scrutinee));
        let new_arms = self
            .arms
            .into_iter()
            .map(|arm| {
                let (body, guard) = arm_map(&arm.pat, arm.body, arm.guard);
                Arm {
                    pat: arm.pat,
                    body,
                    guard,
                }
            })
            .collect();
        Self {
            scrutinee: new_scrutinee,
            arms: new_arms,
        }
    }

    /// Rebuild this `Match` with a transformed scrutinee, leaving every arm
    /// completely untouched. Shape-preserving by construction — the arm
    /// vector is never even iterated. Companion to [`Self::map_bodies`] for
    /// passes that thread a single `&mut` accumulator through scrutinee and
    /// bodies sequentially (see `map_bodies`'s doc).
    #[must_use]
    pub fn map_scrutinee(self, scrutinee_map: impl FnOnce(Expr) -> Expr) -> Self {
        Self {
            scrutinee: Box::new(scrutinee_map(*self.scrutinee)),
            arms: self.arms,
        }
    }

    /// Fallible sibling of [`Self::map_bodies`] for passes that can fail
    /// (e.g. depth-limited clone-capture rewriting). Same shape invariant:
    /// only `scrutinee`, [`Arm::body`], and [`Arm::guard`] are transformed;
    /// each arm's `pat` is carried through untouched, and a failure
    /// short-circuits before any arm is lost or reordered.
    ///
    /// # Errors
    ///
    /// Propagates the first error `scrutinee_map` or `arm_map` returns.
    pub fn try_map_bodies<E>(
        self,
        scrutinee_map: impl FnOnce(Expr) -> Result<Expr, E>,
        mut arm_map: impl FnMut(&Pat, Expr, Option<Expr>) -> Result<(Expr, Option<Expr>), E>,
    ) -> Result<Self, E> {
        let new_scrutinee = Box::new(scrutinee_map(*self.scrutinee)?);
        let new_arms = self
            .arms
            .into_iter()
            .map(|arm| {
                let (body, guard) = arm_map(&arm.pat, arm.body, arm.guard)?;
                Ok(Arm {
                    pat: arm.pat,
                    body,
                    guard,
                })
            })
            .collect::<Result<Vec<_>, E>>()?;
        Ok(Self {
            scrutinee: new_scrutinee,
            arms: new_arms,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use sky_diagnostics::DResult;
    use sky_intern::Interner;

    fn msg_enum(i: &mut Interner) -> DResult<(Symbol, Symbol, Symbol)> {
        let ty = i.intern("Msg")?;
        let inc = i.intern("Increment")?;
        let dec = i.intern("Decrement")?;
        Ok((ty, inc, dec))
    }

    #[test]
    fn match_new_accepts_exhaustive_and_round_trips_debug() -> DResult<()> {
        let mut i = Interner::new();
        let (ty, inc, dec) = msg_enum(&mut i)?;
        let count = i.intern("count")?;

        // case msg of Increment -> count + 1 ; Decrement -> count - 1
        let arms = vec![
            Arm {
                pat: Pat::Ctor {
                    home: ModPath(vec![]),
                    ty,
                    variant: inc,
                    args: vec![],
                },
                body: Expr::BinOp {
                    op: BinOp::Add,
                    lhs: Box::new(Expr::Var(count)),
                    rhs: Box::new(Expr::Int(1)),
                },
                guard: None,
            },
            Arm {
                pat: Pat::Ctor {
                    home: ModPath(vec![]),
                    ty,
                    variant: dec,
                    args: vec![],
                },
                body: Expr::BinOp {
                    op: BinOp::Sub,
                    lhs: Box::new(Expr::Var(count)),
                    rhs: Box::new(Expr::Int(1)),
                },
                guard: None,
            },
        ];
        let res = Match::new(Expr::Var(i.intern("msg")?), arms, &[inc, dec]);

        assert_eq!(res.as_ref().map(|m| m.arms().len()), Ok(2));
        assert!(matches!(
            res.as_ref().map(Match::scrutinee),
            Ok(Expr::Var(_))
        ));
        // Debug round-trips (no panic, stable shape).
        let rendered = format!("{res:?}");
        assert!(rendered.contains("Match"));
        Ok(())
    }

    /// Build the canonical 2-arm exhaustive `Match` the AUD-09 combinator
    /// tests transform (`case msg of Increment -> count ; Decrement -> count`).
    fn two_arm_match(i: &mut Interner) -> DResult<Match> {
        let (ty, inc, dec) = msg_enum(i)?;
        let count = i.intern("count")?;
        let arms = vec![
            Arm {
                pat: Pat::Ctor {
                    home: ModPath(vec![]),
                    ty,
                    variant: inc,
                    args: vec![],
                },
                body: Expr::Var(count),
                guard: None,
            },
            Arm {
                pat: Pat::Ctor {
                    home: ModPath(vec![]),
                    ty,
                    variant: dec,
                    args: vec![],
                },
                body: Expr::Var(count),
                guard: None,
            },
        ];
        Match::new(Expr::Var(i.intern("msg")?), arms, &[inc, dec])
    }

    /// AUD-09: `map_bodies` must carry every arm's pattern (and the arm
    /// count/order) through untouched — an identity transform round-trips, and
    /// a body-rewriting transform changes ONLY the bodies.
    #[test]
    fn map_bodies_preserves_arm_patterns_and_count() -> DResult<()> {
        let mut i = Interner::new();
        let m = two_arm_match(&mut i)?;
        let before_pats: Vec<Pat> = m.arms().iter().map(|a| a.pat.clone()).collect();

        // Identity transform: everything structurally equal.
        let id = m.clone().map_bodies(|s| s, |_, b, g| (b, g));
        assert_eq!(id.arms().len(), 2);
        let id_pats: Vec<Pat> = id.arms().iter().map(|a| a.pat.clone()).collect();
        assert_eq!(id_pats, before_pats);
        assert_eq!(id, m);

        // Real body rewrite: bodies change, patterns/count/order do not.
        let rewritten = m.map_bodies(|s| s, |_, _, g| (Expr::Int(9), g));
        assert_eq!(rewritten.arms().len(), 2);
        let new_pats: Vec<Pat> = rewritten.arms().iter().map(|a| a.pat.clone()).collect();
        assert_eq!(new_pats, before_pats);
        assert!(rewritten.arms().iter().all(|a| a.body == Expr::Int(9)));
        Ok(())
    }

    /// AUD-09: `try_map_bodies` short-circuits on the first arm error — the
    /// whole call is `Err`, no partial/corrupted `Match` is observable.
    #[test]
    fn try_map_bodies_short_circuits_on_err() -> DResult<()> {
        let mut i = Interner::new();
        let m = two_arm_match(&mut i)?;
        let mut seen = 0_u32;
        let res: Result<Match, &str> = m.try_map_bodies(Ok, |_, b, g| {
            seen += 1;
            if seen == 2 {
                Err("second arm fails")
            } else {
                Ok((b, g))
            }
        });
        assert_eq!(res, Err("second arm fails"));
        Ok(())
    }

    /// AUD-09: a scrutinee error short-circuits BEFORE any arm transform runs.
    #[test]
    fn try_map_bodies_scrutinee_error_short_circuits_before_any_arm_runs() -> DResult<()> {
        let mut i = Interner::new();
        let m = two_arm_match(&mut i)?;
        let mut arm_ran = false;
        let res: Result<Match, &str> = m.try_map_bodies(
            |_| Err("scrutinee fails"),
            |_, b, g| {
                arm_ran = true;
                Ok((b, g))
            },
        );
        assert_eq!(res, Err("scrutinee fails"));
        assert!(!arm_ran, "arm_map must never run after a scrutinee error");
        Ok(())
    }

    /// AUD-09 seal: `from_parts_unchecked` must never be reintroduced. Any
    /// caller that needs to rebuild a `Match` after a body-only rewrite must
    /// use `map_bodies`/`try_map_bodies`, which cannot change arm patterns or
    /// arm count/order. (The test's own name spells the sealed identifier
    /// differently so the scan below does not trip on itself.)
    #[test]
    fn no_raw_parts_match_escape_hatch_reintroduced() {
        let src = include_str!("ir.rs");
        for (idx, line) in src.lines().enumerate() {
            let code = line.split("//").next().unwrap_or(line);
            assert!(
                !code.contains(concat!("from_parts_", "unchecked")),
                "AUD-09 seal reintroduced at ir.rs:{} — rebuild via \
                 Match::map_bodies/try_map_bodies instead: {line:?}",
                idx + 1,
            );
        }
    }

    #[test]
    fn match_new_rejects_non_exhaustive() -> DResult<()> {
        let mut i = Interner::new();
        let (ty, inc, dec) = msg_enum(&mut i)?;
        let count = i.intern("count")?;

        // Only the Increment arm — Decrement uncovered.
        let arms = vec![Arm {
            pat: Pat::Ctor {
                home: ModPath(vec![]),
                ty,
                variant: inc,
                args: vec![],
            },
            body: Expr::Var(count),
            guard: None,
        }];
        let r = Match::new(Expr::Var(i.intern("msg")?), arms, &[inc, dec]);
        assert!(matches!(r, Err(Diagnostic::CompilerBug { .. })));
        Ok(())
    }

    #[test]
    fn match_new_accepts_duplicate_top_ctor_with_full_cover() -> DResult<()> {
        let mut i = Interner::new();
        let (ty, inc, dec) = msg_enum(&mut i)?;

        // Two arms head-matching the same top constructor (`Increment`) is the
        // nested-discrimination shape; combined with the `Decrement` arm the top
        // constructors cover the whole enum, so the ordered arm list is accepted.
        let arms = vec![
            Arm {
                pat: Pat::Ctor {
                    home: ModPath(vec![]),
                    ty,
                    variant: inc,
                    args: vec![],
                },
                body: Expr::Int(0),
                guard: None,
            },
            Arm {
                pat: Pat::Ctor {
                    home: ModPath(vec![]),
                    ty,
                    variant: inc,
                    args: vec![],
                },
                body: Expr::Int(1),
                guard: None,
            },
            Arm {
                pat: Pat::Ctor {
                    home: ModPath(vec![]),
                    ty,
                    variant: dec,
                    args: vec![],
                },
                body: Expr::Int(2),
                guard: None,
            },
        ];
        let r = Match::new(Expr::Var(i.intern("msg")?), arms, &[inc, dec])?;
        assert_eq!(r.arms().len(), 3);
        Ok(())
    }

    #[test]
    fn match_new_rejects_missing_top_ctor_despite_duplicate() -> DResult<()> {
        let mut i = Interner::new();
        let (ty, inc, dec) = msg_enum(&mut i)?;

        // `Increment` twice but `Decrement` never: a variant wholly absent from
        // the top constructors guarantees non-exhaustiveness, so the cheap
        // necessary-condition backstop still fails closed.
        let arms = vec![
            Arm {
                pat: Pat::Ctor {
                    home: ModPath(vec![]),
                    ty,
                    variant: inc,
                    args: vec![],
                },
                body: Expr::Int(0),
                guard: None,
            },
            Arm {
                pat: Pat::Ctor {
                    home: ModPath(vec![]),
                    ty,
                    variant: inc,
                    args: vec![],
                },
                body: Expr::Int(1),
                guard: None,
            },
        ];
        let r = Match::new(Expr::Var(i.intern("msg")?), arms, &[inc, dec]);
        assert!(matches!(r, Err(Diagnostic::CompilerBug { .. })));
        Ok(())
    }

    #[test]
    fn match_new_rejects_unknown_variant() -> DResult<()> {
        let mut i = Interner::new();
        let (ty, inc, dec) = msg_enum(&mut i)?;
        let bogus = i.intern("Reset")?;

        let arms = vec![
            Arm {
                pat: Pat::Ctor {
                    home: ModPath(vec![]),
                    ty,
                    variant: inc,
                    args: vec![],
                },
                body: Expr::Int(0),
                guard: None,
            },
            Arm {
                pat: Pat::Ctor {
                    home: ModPath(vec![]),
                    ty,
                    variant: bogus,
                    args: vec![],
                },
                body: Expr::Int(1),
                guard: None,
            },
        ];
        let r = Match::new(Expr::Var(i.intern("msg")?), arms, &[inc, dec]);
        assert!(matches!(r, Err(Diagnostic::CompilerBug { .. })));
        Ok(())
    }

    // ── M3b-3 flat refutable match (`Match::new_flat`) ─────────────────────

    #[test]
    fn new_flat_accepts_literal_arms_with_trailing_wildcard() -> DResult<()> {
        let mut i = Interner::new();
        let n = i.intern("n")?;
        // case n of 0 -> 0 ; 1 -> 1 ; _ -> 9
        let arms = vec![
            Arm {
                pat: Pat::Int(0),
                body: Expr::Int(0),
                guard: None,
            },
            Arm {
                pat: Pat::Int(1),
                body: Expr::Int(1),
                guard: None,
            },
            Arm {
                pat: Pat::Wildcard,
                body: Expr::Int(9),
                guard: None,
            },
        ];
        let r = Match::new_flat(Expr::Var(n), arms);
        assert_eq!(r.as_ref().map(|m| m.arms().len()), Ok(3));
        Ok(())
    }

    #[test]
    fn new_flat_accepts_trailing_variable_and_alias_catch_all() -> DResult<()> {
        let mut i = Interner::new();
        let n = i.intern("n")?;
        let m = i.intern("m")?;
        let k = i.intern("k")?;
        // case n of 0 -> 0 ; (m as k) -> k  — alias-of-var is irrefutable.
        let arms = vec![
            Arm {
                pat: Pat::Int(0),
                body: Expr::Int(0),
                guard: None,
            },
            Arm {
                pat: Pat::Alias(Box::new(Pat::Var(m)), k),
                body: Expr::Var(k),
                guard: None,
            },
        ];
        assert!(Match::new_flat(Expr::Var(n), arms).is_ok());
        Ok(())
    }

    #[test]
    fn new_flat_accepts_complete_bool_cover_without_wildcard() -> DResult<()> {
        let mut i = Interner::new();
        let b = i.intern("b")?;
        // case b of True -> 1 ; False -> 0 — closed cover, no catch-all needed.
        let arms = vec![
            Arm {
                pat: Pat::Bool(true),
                body: Expr::Int(1),
                guard: None,
            },
            Arm {
                pat: Pat::Bool(false),
                body: Expr::Int(0),
                guard: None,
            },
        ];
        assert!(Match::new_flat(Expr::Var(b), arms).is_ok());
        Ok(())
    }

    #[test]
    fn new_flat_accepts_alias_over_ctor_discrimination_without_catch_all() -> DResult<()> {
        let mut i = Interner::new();
        let opt = i.intern("Opt")?;
        let som = i.intern("Som")?;
        let non = i.intern("Non")?;
        let o = i.intern("o")?;
        let w = i.intern("w")?;
        let x = i.intern("x")?;
        // case o of (Som (Som x)) as w -> … ; Som Non -> … ; Non -> …
        // Every head is constructor-shaped (the first under an alias), so the
        // scrutinee is a finite enum whose coverage the upstream Maranget check
        // proved — no trailing catch-all needed.
        let arms = vec![
            Arm {
                pat: Pat::Alias(
                    Box::new(Pat::Ctor {
                        home: ModPath(vec![]),
                        ty: opt,
                        variant: som,
                        args: vec![Pat::Ctor {
                            home: ModPath(vec![]),
                            ty: opt,
                            variant: som,
                            args: vec![Pat::Var(x)],
                        }],
                    }),
                    w,
                ),
                body: Expr::Var(x),
                guard: None,
            },
            Arm {
                pat: Pat::Ctor {
                    home: ModPath(vec![]),
                    ty: opt,
                    variant: som,
                    args: vec![Pat::Ctor {
                        home: ModPath(vec![]),
                        ty: opt,
                        variant: non,
                        args: vec![],
                    }],
                },
                body: Expr::Int(0),
                guard: None,
            },
            Arm {
                pat: Pat::Ctor {
                    home: ModPath(vec![]),
                    ty: opt,
                    variant: non,
                    args: vec![],
                },
                body: Expr::Int(1),
                guard: None,
            },
        ];
        assert!(Match::new_flat(Expr::Var(o), arms).is_ok());
        Ok(())
    }

    #[test]
    fn new_flat_rejects_open_literals_without_catch_all() -> DResult<()> {
        let mut i = Interner::new();
        let n = i.intern("n")?;
        // case n of 0 -> 0 ; 1 -> 1 — Int is OPEN; no catch-all → not structurally
        // exhaustive. The soundness floor: a CompilerBug here, never an emitted
        // `match` that rustc would reject with E0004.
        let arms = vec![
            Arm {
                pat: Pat::Int(0),
                body: Expr::Int(0),
                guard: None,
            },
            Arm {
                pat: Pat::Int(1),
                body: Expr::Int(1),
                guard: None,
            },
        ];
        assert!(matches!(
            Match::new_flat(Expr::Var(n), arms),
            Err(Diagnostic::CompilerBug { .. })
        ));
        Ok(())
    }

    #[test]
    fn new_flat_rejects_incomplete_bool_cover() -> DResult<()> {
        let mut i = Interner::new();
        let b = i.intern("b")?;
        // Only `True` — `False` uncovered and no wildcard.
        let arms = vec![Arm {
            pat: Pat::Bool(true),
            body: Expr::Int(1),
            guard: None,
        }];
        assert!(matches!(
            Match::new_flat(Expr::Var(b), arms),
            Err(Diagnostic::CompilerBug { .. })
        ));
        Ok(())
    }

    #[test]
    fn is_irrefutable_classifies_binders_only() -> DResult<()> {
        let mut i = Interner::new();
        let x = i.intern("x")?;
        assert!(is_irrefutable(&Pat::Wildcard));
        assert!(is_irrefutable(&Pat::Var(x)));
        assert!(is_irrefutable(&Pat::Alias(Box::new(Pat::Var(x)), x)));
        assert!(!is_irrefutable(&Pat::Int(0)));
        assert!(!is_irrefutable(&Pat::Bool(true)));
        assert!(!is_irrefutable(&Pat::Str("hi".to_owned())));
        assert!(!is_irrefutable(&Pat::Alias(Box::new(Pat::Int(0)), x)));
        Ok(())
    }

    #[test]
    fn dispatch_free_over_tuple_of_vars_and_wildcards() -> DResult<()> {
        let mut i = Interner::new();
        let x = i.intern("x")?;
        assert!(is_dispatch_free(&Pat::Tuple(vec![
            Pat::Var(x),
            Pat::Wildcard
        ])));
        // An alias over a dispatch-free tuple stays dispatch-free.
        assert!(is_dispatch_free(&Pat::Alias(
            Box::new(Pat::Tuple(vec![Pat::Var(x), Pat::Var(x)])),
            x,
        )));
        // A record of binder leaves is dispatch-free too.
        assert!(is_dispatch_free(&Pat::Record(vec![
            (x, Pat::Var(x)),
            (x, Pat::Wildcard)
        ])));
        Ok(())
    }

    #[test]
    fn dispatch_free_false_over_nested_ctor_or_literal() -> DResult<()> {
        let mut i = Interner::new();
        let x = i.intern("x")?;
        assert!(!is_dispatch_free(&Pat::Tuple(vec![
            Pat::Int(0),
            Pat::Var(x)
        ])));
        // A Ctor nested inside a Tuple inside an Alias must still fail.
        assert!(!is_dispatch_free(&Pat::Alias(
            Box::new(Pat::Tuple(vec![Pat::Ctor {
                home: ModPath(vec![]),
                ty: x,
                variant: x,
                args: vec![],
            }])),
            x,
        )));
        // Slice / literal leaves need dispatch wherever they sit.
        assert!(!is_dispatch_free(&Pat::Slice {
            prefix: vec![Pat::Var(x)],
            rest: None,
        }));
        assert!(!is_dispatch_free(&Pat::Str("s".to_owned())));
        Ok(())
    }

    #[test]
    fn tuple_expr_and_type_construct_and_round_trip() -> DResult<()> {
        let mut i = Interner::new();
        let x = i.intern("x")?;

        // ( x + 1, 2, "three"-as-Var ) — a 3-tuple expression.
        let expr = Expr::Tuple(vec![
            Expr::BinOp {
                op: BinOp::Add,
                lhs: Box::new(Expr::Var(x)),
                rhs: Box::new(Expr::Int(1)),
            },
            Expr::Int(2),
            Expr::Var(x),
        ]);
        assert_eq!(expr, expr.clone());
        let rendered = format!("{expr:?}");
        assert!(rendered.contains("Tuple"));

        // (Int, Bool) — a 2-tuple type.
        let ty = IrType::Tuple(vec![IrType::Int, IrType::Bool]);
        assert_eq!(ty, ty.clone());
        assert!(format!("{ty:?}").contains("Tuple"));

        // Nested tuple type: (Int, (Bool, String)).
        let nested = IrType::Tuple(vec![
            IrType::Int,
            IrType::Tuple(vec![IrType::Bool, IrType::Str]),
        ]);
        assert_eq!(nested, nested.clone());
        Ok(())
    }

    #[test]
    fn record_expr_access_update_and_type_construct_and_round_trip() -> DResult<()> {
        let mut i = Interner::new();
        let x = i.intern("x")?;
        let y = i.intern("y")?;
        let p = i.intern("p")?;

        // { x = 1, y = 2 } — fields sorted by name (x before y).
        let lit = Expr::Record(vec![(x, Expr::Int(1)), (y, Expr::Int(2))]);
        assert_eq!(lit, lit.clone());
        assert!(format!("{lit:?}").contains("Record"));

        // p.x — a field access.
        let access = Expr::Access {
            record: Box::new(Expr::Var(p)),
            field: x,
            field_ty: IrType::Int,
        };
        assert_eq!(access, access.clone());
        assert!(format!("{access:?}").contains("Access"));

        // { p | x = 5 } — a single-field update.
        let update = Expr::Update {
            record: Box::new(Expr::Var(p)),
            fields: vec![(x, Expr::Int(5))],
        };
        assert_eq!(update, update.clone());
        assert!(format!("{update:?}").contains("Update"));

        // { x : Int, y : Bool } — a closed record TYPE.
        let mut fields = BTreeMap::new();
        fields.insert(x, IrType::Int);
        fields.insert(y, IrType::Bool);
        let ty = IrType::Record(fields);
        assert_eq!(ty, ty.clone());
        assert!(format!("{ty:?}").contains("Record"));

        // Nested record type: { x : Int, y : { x : Int, y : Bool } }.
        let mut outer = BTreeMap::new();
        outer.insert(x, IrType::Int);
        outer.insert(y, ty);
        let nested = IrType::Record(outer);
        assert_eq!(nested, nested.clone());
        Ok(())
    }

    #[test]
    fn lambda_apply_expr_and_fun_type_construct_and_round_trip() -> DResult<()> {
        let mut i = Interner::new();
        let x = i.intern("x")?;
        let f = i.intern("f")?;

        // \x -> x + 1 — a single-param lambda returning Int.
        let lambda = Expr::Lambda {
            params: vec![(x, IrType::Int)],
            ret: IrType::Int,
            body: Box::new(Expr::BinOp {
                op: BinOp::Add,
                lhs: Box::new(Expr::Var(x)),
                rhs: Box::new(Expr::Int(1)),
            }),
        };
        assert_eq!(lambda, lambda.clone());
        assert!(format!("{lambda:?}").contains("Lambda"));

        // f 2 — apply the function-typed local `f` to one argument.
        let apply = Expr::Apply {
            func: Box::new(Expr::Var(f)),
            args: vec![Expr::Int(2)],
        };
        assert_eq!(apply, apply.clone());
        assert!(format!("{apply:?}").contains("Apply"));

        // Int -> Int — a one-param function type.
        let fun_ty = IrType::Fun(vec![IrType::Int], Box::new(IrType::Int));
        assert_eq!(fun_ty, fun_ty.clone());
        assert!(format!("{fun_ty:?}").contains("Fun"));

        // () -> Bool — a nullary function type (distinct from Bool alone).
        let nullary = IrType::Fun(vec![], Box::new(IrType::Bool));
        assert_eq!(nullary, nullary.clone());
        assert_ne!(nullary, IrType::Bool);

        // (Int, Bool) -> Int — a multi-param function type, nested under Fun.
        let multi = IrType::Fun(
            vec![IrType::Int, IrType::Bool],
            Box::new(IrType::Fun(vec![IrType::Str], Box::new(IrType::Unit))),
        );
        assert_eq!(multi, multi.clone());

        // A top-level function named as a first-class value: callee `fn#0`,
        // reified at its boxed `Int -> Int` value type.
        let func_value = Expr::FuncValue {
            callee: Callee::Func(FuncId::from_raw(0)),
            ty: IrType::Fun(vec![IrType::Int], Box::new(IrType::Int)),
        };
        assert_eq!(func_value, func_value.clone());
        assert!(format!("{func_value:?}").contains("FuncValue"));
        Ok(())
    }

    #[test]
    fn generic_type_and_quantified_func_construct_and_round_trip() -> DResult<()> {
        let mut i = Interner::new();
        let a = i.intern("a")?;
        let b = i.intern("b")?;
        let x = i.intern("x")?;
        let id = i.intern("id")?;

        // A fully-parametric `id : a -> a` quantifying [a].
        let generic_a = IrType::Generic(a);
        assert_eq!(generic_a, generic_a.clone());
        assert!(format!("{generic_a:?}").contains("Generic"));

        let func = Func {
            id: FuncId::from_raw(0),
            name: id,
            home: ModPath(vec![]),
            type_params: vec![(a, BoundSet::UNBOUNDED)],
            params: vec![(x, IrType::Generic(a))],
            ret: IrType::Generic(a),
            body: Expr::Var(x),
        };
        assert_eq!(func, func.clone());
        assert_eq!(func.type_params, vec![(a, BoundSet::UNBOUNDED)]);

        // Distinct generic vars compare unequal; quantification order is carried
        // verbatim (no dedup / sort), so [a, b] stays [a, b].
        assert_ne!(IrType::Generic(a), IrType::Generic(b));
        let two = Func {
            id: FuncId::from_raw(1),
            name: id,
            home: ModPath(vec![]),
            type_params: vec![(a, BoundSet::UNBOUNDED), (b, BoundSet::UNBOUNDED)],
            params: vec![(x, IrType::Generic(a))],
            ret: IrType::Generic(b),
            body: Expr::Var(x),
        };
        assert_eq!(
            two.type_params,
            vec![(a, BoundSet::UNBOUNDED), (b, BoundSet::UNBOUNDED)]
        );

        // A constrained variable carries its bounds; an unbounded one does not.
        assert!(BoundSet::default().is_unbounded());
        let bounds = BoundSet::UNBOUNDED.with_add().with_copy();
        assert!(!bounds.is_unbounded());
        assert!(bounds.has_add() && bounds.has_copy());
        assert!(!bounds.has_sub() && !bounds.has_ord());
        let double = Func {
            id: FuncId::from_raw(2),
            name: id,
            home: ModPath(vec![]),
            type_params: vec![(a, bounds)],
            params: vec![(x, IrType::Generic(a))],
            ret: IrType::Generic(a),
            body: Expr::Var(x),
        };
        assert_eq!(double.type_params, vec![(a, bounds)]);
        Ok(())
    }

    #[test]
    fn program_round_trips_debug() -> DResult<()> {
        let mut i = Interner::new();
        let (ty, inc, dec) = msg_enum(&mut i)?;
        let main_sym = i.intern("main")?;
        let main_mod = i.intern("Main")?;

        let func = Func {
            id: FuncId::from_raw(0),
            name: main_sym,
            home: ModPath(vec![]),
            type_params: vec![],
            params: vec![],
            ret: IrType::Task(Box::new(IrType::Unit)),
            body: Expr::Call {
                callee: Callee::Kernel(KernelFn::LogPrintln),
                args: vec![Expr::Call {
                    callee: Callee::Kernel(KernelFn::StringFromInt),
                    args: vec![Expr::Int(1)],
                    pin: CallPin::None,
                }],
                pin: CallPin::None,
            },
        };
        let program = Program {
            modules: vec![Module {
                name: ModPath(vec![main_mod]),
                types: vec![TypeDef::Enum(EnumDef {
                    home: ModPath(vec![]),
                    name: ty,
                    type_params: vec![],
                    variants: vec![
                        Variant {
                            name: inc,
                            fields: vec![],
                        },
                        Variant {
                            name: dec,
                            fields: vec![],
                        },
                    ],
                })],
                funcs: vec![func],
                entry: Some(FuncId::from_raw(0)),
                records: vec![],
                uses_tea: false,
                uses_server: false,
                uses_ui: false,
                uses_live: false,
                uses_tui: false,
                uses_webview: false,
                uses_css: false,
                uses_auth: false,
            }],
        };
        let clone = program.clone();
        assert_eq!(program, clone);
        assert!(format!("{program:?}").contains("Program"));
        Ok(())
    }

    #[test]
    fn let_if_and_extended_binops_construct_and_round_trip() -> DResult<()> {
        let mut i = Interner::new();
        let x = i.intern("x")?;

        // let x = 6 / 2 in if (x == 3) && (x > 0) then x * 10 else x - 1
        let expr = Expr::Let {
            name: x,
            value: Box::new(Expr::BinOp {
                op: BinOp::Div,
                lhs: Box::new(Expr::Int(6)),
                rhs: Box::new(Expr::Int(2)),
            }),
            body: Box::new(Expr::If {
                cond: Box::new(Expr::BinOp {
                    op: BinOp::And,
                    lhs: Box::new(Expr::BinOp {
                        op: BinOp::Eq,
                        lhs: Box::new(Expr::Var(x)),
                        rhs: Box::new(Expr::Int(3)),
                    }),
                    rhs: Box::new(Expr::BinOp {
                        op: BinOp::Gt,
                        lhs: Box::new(Expr::Var(x)),
                        rhs: Box::new(Expr::Int(0)),
                    }),
                }),
                then_: Box::new(Expr::BinOp {
                    op: BinOp::Mul,
                    lhs: Box::new(Expr::Var(x)),
                    rhs: Box::new(Expr::Int(10)),
                }),
                else_: Box::new(Expr::BinOp {
                    op: BinOp::Sub,
                    lhs: Box::new(Expr::Var(x)),
                    rhs: Box::new(Expr::Int(1)),
                }),
            }),
        };

        // Clone + structural equality + Debug all hold for the new variants.
        assert_eq!(expr, expr.clone());
        let rendered = format!("{expr:?}");
        assert!(rendered.contains("Let"));
        assert!(rendered.contains("If"));

        // Every extended BinOp is a distinct, Copy, comparable value: the full
        // set has no duplicates and the Copy bound holds (the array is consumed
        // by value below without moving out of `all`).
        let all = [
            BinOp::Add,
            BinOp::Sub,
            BinOp::Mul,
            BinOp::Div,
            BinOp::IntDiv,
            BinOp::Eq,
            BinOp::Neq,
            BinOp::Lt,
            BinOp::Gt,
            BinOp::Le,
            BinOp::Ge,
            BinOp::And,
            BinOp::Or,
            BinOp::Append,
        ];
        let distinct: BTreeSet<_> = all.iter().map(|op| format!("{op:?}")).collect();
        assert_eq!(distinct.len(), all.len());
        let copied = all;
        assert_eq!(copied.len(), all.len());
        Ok(())
    }

    #[test]
    fn payload_and_generic_enum_construct_and_round_trip() -> DResult<()> {
        let mut i = Interner::new();
        let a = i.intern("a")?;
        let maybe = i.intern("Maybe")?;
        let just = i.intern("Just")?;
        let nothing = i.intern("Nothing")?;

        // type Maybe a = Just a | Nothing — one generic param, one payload
        // variant (carrying the type variable), one nullary variant.
        let def = EnumDef {
            home: ModPath(vec![]),
            name: maybe,
            type_params: vec![a],
            variants: vec![
                Variant {
                    name: just,
                    fields: vec![IrType::Generic(a)],
                },
                Variant {
                    name: nothing,
                    fields: vec![],
                },
            ],
        };
        assert_eq!(def, def.clone());
        assert_eq!(def.type_params, vec![a]);
        assert_eq!(def.variants.len(), 2);
        assert!(def.variants.first().is_some_and(|v| !v.fields.is_empty()));
        assert!(def.variants.get(1).is_some_and(|v| v.fields.is_empty()));

        // A use-site type `Maybe Int` carries its concrete type argument.
        let use_ty = IrType::Enum {
            home: ModPath(vec![]),
            name: maybe,
            args: vec![IrType::Int],
        };
        assert_eq!(use_ty, use_ty.clone());
        // A non-generic enum use carries no args and is distinct from the applied
        // form.
        let bare = IrType::Enum {
            home: ModPath(vec![]),
            name: maybe,
            args: vec![],
        };
        assert_ne!(use_ty, bare);

        // Construction `Just 5` carries its payload argument.
        let ctor = Expr::Ctor {
            home: ModPath(vec![]),
            ty: maybe,
            variant: just,
            args: vec![Expr::Int(5)],
        };
        assert_eq!(ctor, ctor.clone());
        assert!(format!("{ctor:?}").contains("Ctor"));
        Ok(())
    }

    #[test]
    fn ctor_pattern_with_var_and_wildcard_payloads_round_trip() -> DResult<()> {
        let mut i = Interner::new();
        let maybe = i.intern("Maybe")?;
        let just = i.intern("Just")?;
        let nothing = i.intern("Nothing")?;
        let x = i.intern("x")?;
        let m = i.intern("m")?;

        // case m of Just x -> x ; Nothing -> 0  — a var-binding payload pattern
        // and a nullary pattern. Match::new accepts it (coverage over the variant
        // NAME set; payload binding does not affect coverage).
        let arms = vec![
            Arm {
                pat: Pat::Ctor {
                    home: ModPath(vec![]),
                    ty: maybe,
                    variant: just,
                    args: vec![Pat::Var(x)],
                },
                body: Expr::Var(x),
                guard: None,
            },
            Arm {
                pat: Pat::Ctor {
                    home: ModPath(vec![]),
                    ty: maybe,
                    variant: nothing,
                    args: vec![],
                },
                body: Expr::Int(0),
                guard: None,
            },
        ];
        let m1 = Match::new(Expr::Var(m), arms, &[just, nothing])?;
        assert_eq!(m1.arms().len(), 2);

        // The wildcard payload sub-pattern is also representable.
        let wild = Pat::Ctor {
            home: ModPath(vec![]),
            ty: maybe,
            variant: just,
            args: vec![Pat::Wildcard],
        };
        assert_eq!(wild, wild.clone());
        assert!(format!("{wild:?}").contains("Wildcard"));
        Ok(())
    }

    #[test]
    fn match_new_rejects_non_ctor_arm_head() -> DResult<()> {
        let mut i = Interner::new();
        let (_ty, inc, dec) = msg_enum(&mut i)?;

        // A bare variable whole-scrutinee arm is not an M3a shape — the arm head
        // must be a constructor pattern, so Match::new fails closed.
        let arms = vec![Arm {
            pat: Pat::Var(i.intern("anything")?),
            body: Expr::Int(0),
            guard: None,
        }];
        let r = Match::new(Expr::Var(i.intern("msg")?), arms, &[inc, dec]);
        assert!(matches!(r, Err(Diagnostic::CompilerBug { .. })));
        Ok(())
    }

    #[test]
    fn recursive_enum_def_round_trips() -> DResult<()> {
        let mut i = Interner::new();
        let tree = i.intern("Tree")?;
        let leaf = i.intern("Leaf")?;
        let node = i.intern("Node")?;

        // type Tree = Leaf | Node Tree Int Tree — the Node payload carries two
        // direct self-edges (the enum's own type) around an Int.
        let self_ty = IrType::Enum {
            home: ModPath(vec![]),
            name: tree,
            args: vec![],
        };
        let def = EnumDef {
            home: ModPath(vec![]),
            name: tree,
            type_params: vec![],
            variants: vec![
                Variant {
                    name: leaf,
                    fields: vec![],
                },
                Variant {
                    name: node,
                    fields: vec![self_ty.clone(), IrType::Int, self_ty],
                },
            ],
        };
        assert_eq!(def, def.clone());
        assert!(def.variants.get(1).is_some_and(|v| v.fields.len() == 3));
        Ok(())
    }

    // ── seal: ir_type_is_serde ─────────────────────────────────────────

    /// `true` for every referenced enum — the leaf-level predicate under test.
    fn all_serde(_: &ModPath, _: Symbol) -> bool {
        true
    }

    #[test]
    fn serde_primitives_and_plain_carriers_are_ok() {
        let ok = [
            IrType::Int,
            IrType::Float,
            IrType::Bool,
            IrType::Str,
            IrType::Char,
            IrType::Unit,
            IrType::Bytes,
            IrType::Json,
            IrType::Generic(Symbol::from_raw(0)),
            IrType::List(Box::new(IrType::Int)),
            IrType::Maybe(Box::new(IrType::Str)),
            IrType::Result(Box::new(IrType::Str), Box::new(IrType::Int)),
            IrType::Tuple(vec![IrType::Int, IrType::Bool]),
        ];
        for t in ok {
            assert!(ir_type_is_serde(&t, &all_serde), "{t:?} should be serde");
        }
    }

    #[test]
    fn serde_rejects_effects_handles_and_functions() {
        let bad = [
            IrType::Cmd(Box::new(IrType::Unit)),
            IrType::Sub(Box::new(IrType::Unit)),
            IrType::Task(Box::new(IrType::Unit)),
            IrType::Decoder(Box::new(IrType::Int)),
            IrType::Db,
            IrType::Fun(vec![IrType::Int], Box::new(IrType::Int)),
            IrType::ServerRequest,
            IrType::LiveReq,
        ];
        for t in bad {
            assert!(!ir_type_is_serde(&t, &all_serde), "{t:?} must NOT be serde");
        }
    }

    /// The two arms where serde is STRICTER than derivable: UI carriers and UI
    /// plain values are `Clone`/`Debug`/`PartialEq` but not `serde`.
    #[test]
    fn serde_rejects_ui_but_derivable_accepts_it() {
        let html = IrType::Ui {
            ctor: UiCtor::Html,
            msg: Box::new(IrType::Unit),
        };
        let color = IrType::UiPlain(UiPlain::Color);
        for t in [&html, &color] {
            assert!(
                ir_type_is_derivable(t, &all_serde),
                "{t:?} IS derivable (CDPeq)"
            );
            assert!(
                !ir_type_is_serde(t, &all_serde),
                "{t:?} must NOT be serde (the #91 CDPeq-but-not-serde gap)"
            );
        }
    }

    /// `SqlFragment`: fully derivable (Clone + `PartialEq`) but
    /// deliberately NOT serde — it is a query-building value, never persisted
    /// to a Live session store.
    #[test]
    fn sqlfragment_derivable_but_not_serde() {
        let t = IrType::SqlFragment;
        assert!(
            ir_type_is_derivable(&t, &all_serde),
            "SqlFragment IS derivable (Clone + PartialEq)"
        );
        assert!(
            !ir_type_is_serde(&t, &all_serde),
            "SqlFragment must NOT be serde"
        );
    }

    /// `Secret`: fully derivable (Clone + `PartialEq`) but
    /// deliberately NOT serde — a `Secret` must never round-trip through a
    /// Live session store or any other serialisation path. This is also the
    /// #45/#70 derive-blast-radius regression: a record containing a `Secret`
    /// field must still get `Clone`/`Debug`/`==` (proved by
    /// `serde_poisons_carriers_transitively`-style coverage below).
    #[test]
    fn secret_derivable_but_not_serde() {
        let t = IrType::Secret;
        assert!(
            ir_type_is_derivable(&t, &all_serde),
            "Secret IS derivable (Clone + PartialEq)"
        );
        assert!(
            !ir_type_is_serde(&t, &all_serde),
            "Secret must NOT be serde"
        );
    }

    /// derive-blast-radius regression: a record `{ apiKey : Secret, label :
    /// String }` must still be derivable (Clone/Debug/PartialEq) even though
    /// its `Secret` field is not serde — marking a leaf non-derivable (rather
    /// than merely non-serde) would have made the WHOLE record lose ALL
    /// derives (the exact #45/#70 exit-0-then-cargo-fail class the fix spec's
    /// §1 mechanism exists to avoid).
    #[test]
    fn record_containing_secret_stays_derivable() {
        let mut fields = std::collections::BTreeMap::new();
        fields.insert(Symbol::from_raw(0), IrType::Secret);
        fields.insert(Symbol::from_raw(1), IrType::Str);
        let t = IrType::Record(fields);
        assert!(
            ir_type_is_derivable(&t, &all_serde),
            "a record containing a Secret field must stay derivable"
        );
        assert!(
            !ir_type_is_serde(&t, &all_serde),
            "a record containing a Secret field must NOT be serde"
        );
    }

    /// A carrier is serde iff every child is; one bad field poisons it.
    #[test]
    fn serde_poisons_carriers_transitively() {
        let bad_list = IrType::List(Box::new(IrType::Cmd(Box::new(IrType::Unit))));
        assert!(!ir_type_is_serde(&bad_list, &all_serde));
        let good_list = IrType::List(Box::new(IrType::Int));
        assert!(ir_type_is_serde(&good_list, &all_serde));
        // A record with one Cmd field is not serde; all-Int is.
        let mut bad = std::collections::BTreeMap::new();
        bad.insert(Symbol::from_raw(0), IrType::Int);
        bad.insert(Symbol::from_raw(1), IrType::Cmd(Box::new(IrType::Unit)));
        assert!(!ir_type_is_serde(&IrType::Record(bad), &all_serde));
    }

    /// A referenced enum's serde verdict flows through the `enum_serde` lookup.
    #[test]
    fn serde_consults_enum_lookup() {
        let e = IrType::Enum {
            home: ModPath(vec![]),
            name: Symbol::from_raw(7),
            args: vec![],
        };
        assert!(ir_type_is_serde(&e, &|_, _| true), "serde enum passes");
        assert!(
            !ir_type_is_serde(&e, &|_, _| false),
            "non-serde enum poisons"
        );
    }
}

/// Phase 6.5 (symbol-relocation persistence) — `sky_ir`-level round-trip and
/// cross-process id-drift proof for whole-`Program` `serde` persistence.
///
/// Complements `sky_intern`'s `Symbol`-level proof
/// (`serialize_then_deserialize_survives_cross_process_id_drift`) with the
/// SAME property one layer up, over a realistic `Program` value that
/// exercises every `Symbol`-carrying shape: `ModPath`, `EnumDef`/`Variant`
/// names, `Func` `name`/`params`/`type_params`, a `Match` (the one hand-written
/// `serde` impl in this crate), `Pat::Ctor`/`Pat::Var`, and a record literal
/// (`Vec<(Symbol, Expr)>`).
#[cfg(test)]
mod serde_persistence_tests {
    use std::sync::{Arc, Mutex};

    use sky_diagnostics::DResult;
    use sky_intern::{Interner, SerdeInternerGuard};

    use super::{
        Arm, CallPin, Callee, EnumDef, Expr, Func, FuncId, IrType, KernelFn, Match, ModPath,
        Module, Pat, Program, TypeDef, Variant,
    };
    use crate::pretty::pretty;

    /// Build a small but representative `Program`: one enum (`Msg`), one
    /// entry function whose body pattern-matches the enum via a genuine
    /// [`Match`] node and constructs a record literal — every
    /// `Symbol`-carrying IR shape this module's doc names, in one value.
    fn sample_program(i: &mut Interner) -> DResult<Program> {
        let msg_ty = i.intern("Msg")?;
        let inc = i.intern("Increment")?;
        let dec = i.intern("Decrement")?;
        let main_sym = i.intern("main")?;
        let main_mod = i.intern("Main")?;
        let msg_param = i.intern("msg")?;
        let count_field = i.intern("count")?;

        let scrutinee = Expr::Var(msg_param);
        let arms = vec![
            Arm::new(
                Pat::Ctor {
                    home: ModPath(vec![]),
                    ty: msg_ty,
                    variant: inc,
                    args: vec![],
                },
                // A record literal `{ count = 1 }` — exercises
                // `Vec<(Symbol, Expr)>`.
                Expr::Record(vec![(count_field, Expr::Int(1))]),
            ),
            Arm::new(
                Pat::Ctor {
                    home: ModPath(vec![]),
                    ty: msg_ty,
                    variant: dec,
                    args: vec![],
                },
                Expr::Record(vec![(count_field, Expr::Int(0))]),
            ),
        ];
        let body = Expr::Match(Match::new(scrutinee, arms, &[inc, dec])?);

        let func = Func {
            id: FuncId::from_raw(0),
            name: main_sym,
            home: ModPath(vec![]),
            type_params: vec![],
            params: vec![(msg_param, IrType::Generic(msg_param))],
            ret: IrType::Unit,
            body,
        };
        Ok(Program {
            modules: vec![Module {
                name: ModPath(vec![main_mod]),
                types: vec![TypeDef::Enum(EnumDef {
                    home: ModPath(vec![]),
                    name: msg_ty,
                    type_params: vec![],
                    variants: vec![
                        Variant {
                            name: inc,
                            fields: vec![],
                        },
                        Variant {
                            name: dec,
                            fields: vec![],
                        },
                    ],
                })],
                funcs: vec![func],
                entry: Some(FuncId::from_raw(0)),
                records: vec![],
                uses_tea: false,
                uses_server: false,
                uses_ui: false,
                uses_live: false,
                uses_tui: false,
                uses_webview: false,
                uses_css: false,
                uses_auth: false,
            }],
        })
    }

    #[test]
    fn round_trips_within_one_interner() -> DResult<()> {
        let mut plain = Interner::new();
        let program = sample_program(&mut plain)?;
        let interner = Arc::new(Mutex::new(plain));

        let json = {
            let _guard = SerdeInternerGuard::install(Arc::clone(&interner));
            serde_json::to_string(&program).expect("serialize must succeed")
        };
        let round_tripped: Program = {
            let _guard = SerdeInternerGuard::install(Arc::clone(&interner));
            serde_json::from_str(&json).expect("deserialize must succeed")
        };
        assert_eq!(
            program, round_tripped,
            "same interner: ids AND strings agree"
        );
        Ok(())
    }

    /// A tampered [`Callee::Kernel`] entry must not silently coerce — the
    /// derived `Deserialize` on the closed `StdlibKernel` enum rejects any
    /// tag it does not recognise (proof that the whole-`Program` `serde`
    /// surface fails closed on structurally-invalid input, not just on a
    /// poisoned `Symbol`).
    #[test]
    fn deserialize_rejects_unknown_kernel_tag() -> DResult<()> {
        let mut plain = Interner::new();
        let f = plain.intern("f")?;
        let interner = Arc::new(Mutex::new(plain));
        let program = Program {
            modules: vec![Module {
                name: ModPath(vec![f]),
                types: vec![],
                funcs: vec![Func {
                    id: FuncId::from_raw(0),
                    name: f,
                    home: ModPath(vec![]),
                    type_params: vec![],
                    params: vec![],
                    ret: IrType::Unit,
                    body: Expr::Call {
                        callee: Callee::Kernel(KernelFn::LogPrintln),
                        args: vec![],
                        pin: CallPin::None,
                    },
                }],
                entry: None,
                records: vec![],
                uses_tea: false,
                uses_server: false,
                uses_ui: false,
                uses_live: false,
                uses_tui: false,
                uses_webview: false,
                uses_css: false,
                uses_auth: false,
            }],
        };
        let json = {
            let _guard = SerdeInternerGuard::install(Arc::clone(&interner));
            serde_json::to_string(&program).expect("serialize must succeed")
        };
        let tampered = json.replace("\"LogPrintln\"", "\"NotARealKernelVariant\"");
        let _guard = SerdeInternerGuard::install(Arc::clone(&interner));
        let result: Result<Program, _> = serde_json::from_str(&tampered);
        assert!(
            result.is_err(),
            "an unknown kernel tag must be rejected, never silently coerced"
        );
        Ok(())
    }

    /// A tampered `Match` with an EMPTIED arm list must be rejected at
    /// deserialize time via [`Match::new_flat`]'s structural backstop —
    /// proof that `Match`'s hand-written `Deserialize` actually revalidates
    /// rather than trusting the disk bytes verbatim.
    #[test]
    fn deserialize_rejects_emptied_tampered_match() -> DResult<()> {
        let mut plain = Interner::new();
        let program = sample_program(&mut plain)?;
        let interner = Arc::new(Mutex::new(plain));
        let json = {
            let _guard = SerdeInternerGuard::install(Arc::clone(&interner));
            serde_json::to_string(&program).expect("serialize must succeed")
        };

        // Parse back into a generic JSON value and empty the arm list
        // entirely — `Match::new_flat` rejects an arm-less match outright
        // ("flat match has no arms").
        let mut value: serde_json::Value =
            serde_json::from_str(&json).expect("must parse as generic JSON");
        let arms = value
            .pointer_mut("/modules/0/funcs/0/body/Match/arms")
            .and_then(serde_json::Value::as_array_mut)
            .expect("Match.arms must be a JSON array at the expected path");
        assert_eq!(
            arms.len(),
            2,
            "sample_program must have built a 2-arm Match"
        );
        arms.clear();
        let tampered = serde_json::to_string(&value).expect("re-serialize must succeed");

        let _guard = SerdeInternerGuard::install(Arc::clone(&interner));
        let result: Result<Program, _> = serde_json::from_str(&tampered);
        assert!(
            result.is_err(),
            "an emptied tampered Match arm list must be rejected, \
             never silently accepted as a valid Program"
        );
        Ok(())
    }

    /// **Honestly scoped gap, verified rather than merely documented.**
    /// Dropping ONE arm from an otherwise-exhaustive ctor-headed cover is
    /// NOT caught by [`Match`]'s `Deserialize` (see that impl's own doc for
    /// why: `new_flat`'s `all_ctor_headed` branch trusts the upstream
    /// Maranget check and does not re-verify full variant coverage, and
    /// `Match` carries no external "complete variant set" to check
    /// against). This test pins that boundary explicitly so a future
    /// change to `new_flat`'s semantics is forced to reconsider it, rather
    /// than silently regressing this deserializer's actual coverage
    /// without anyone noticing. The gap is safe: the resulting `Program`
    /// still cannot reach a `cargo build` success — `RustBackend::emit`
    /// renders the missing arm as a genuine Rust exhaustiveness gap, which
    /// `cargo build` rejects with E0004 (a loud failure, never silently
    /// wrong output).
    #[test]
    fn deserialize_accepts_single_arm_ctor_headed_match_new_flat_does_not_reverify_full_coverage()
    -> DResult<()> {
        let mut plain = Interner::new();
        let program = sample_program(&mut plain)?;
        let interner = Arc::new(Mutex::new(plain));
        let json = {
            let _guard = SerdeInternerGuard::install(Arc::clone(&interner));
            serde_json::to_string(&program).expect("serialize must succeed")
        };

        let mut value: serde_json::Value =
            serde_json::from_str(&json).expect("must parse as generic JSON");
        let arms = value
            .pointer_mut("/modules/0/funcs/0/body/Match/arms")
            .and_then(serde_json::Value::as_array_mut)
            .expect("Match.arms must be a JSON array at the expected path");
        arms.truncate(1);
        let tampered = serde_json::to_string(&value).expect("re-serialize must succeed");

        let _guard = SerdeInternerGuard::install(Arc::clone(&interner));
        let result: Result<Program, _> = serde_json::from_str(&tampered);
        assert!(
            result.is_ok(),
            "documents the known `new_flat` scope boundary: a single \
             ctor-headed arm still passes `all_ctor_headed`, since \
             `new_flat` does not itself re-check variant coverage"
        );
        Ok(())
    }

    /// **The mission proof.** Proves a `Program` deserialized through the
    /// relocation pass, into a reader interner whose id-assignment history
    /// has NOTHING in common with the writer's, is structurally/
    /// name-identical to a Program built by a totally independent, never-
    /// serialized construction — i.e. behaves exactly as if it had been
    /// freshly lowered in THAT reader process, which is the property this
    /// whole persistence design exists to guarantee.
    ///
    /// A same-process round trip (the test above) cannot distinguish "the
    /// relocation pass correctly re-interns by string" from "the id
    /// happened to survive by coincidence" — a fresh interner given the
    /// exact same sequence of `intern` calls in the exact same order would
    /// trivially reproduce the same ids either way. This test deliberately
    /// diverges every interner's history (different noise, different
    /// counts, different orders) so a raw-id relocation bug WOULD manifest
    /// as a wrong resolved name somewhere in the structural dump — then
    /// asserts the dumps are identical anyway, comparing by RESOLVED NAME
    /// (via `sky_ir::pretty::pretty`, not raw `Symbol` equality, since two
    /// independently-built-then-relocated `Program`s are not expected to
    /// share numeric ids — only meaning).
    #[test]
    fn program_survives_cross_process_symbol_id_drift() -> DResult<()> {
        // "Process A" (the writer): pollute with noise unrelated to the
        // program, then build + serialize.
        let mut interner_a = Interner::new();
        for noise in ["alpha", "beta", "gamma"] {
            interner_a.intern(noise)?;
        }
        let program_a = sample_program(&mut interner_a)?;
        let interner_a = Arc::new(Mutex::new(interner_a));
        let json = {
            let _guard = SerdeInternerGuard::install(Arc::clone(&interner_a));
            serde_json::to_string(&program_a).expect("serialize must succeed")
        };

        // "Process B" (the reader): pollute with a DIFFERENT set of noise,
        // in a different order and count, before deserializing — forces
        // genuine id drift relative to process A for every relocated
        // symbol.
        let mut interner_b = Interner::new();
        for noise in ["zzz_one", "zzz_two", "zzz_three", "zzz_four", "zzz_five"] {
            interner_b.intern(noise)?;
        }
        let interner_b = Arc::new(Mutex::new(interner_b));
        let program_b: Program = {
            let _guard = SerdeInternerGuard::install(Arc::clone(&interner_b));
            serde_json::from_str(&json).expect("deserialize must succeed")
        };

        // "Process C" (ground truth): a COMPLETELY independent construction
        // that never touches serialization at all — the value a genuine
        // fresh `lower_program()` call would produce in yet another
        // process. Its interner has its OWN unrelated history too.
        let mut interner_c = Interner::new();
        for noise in ["unrelated_1", "unrelated_2"] {
            interner_c.intern(noise)?;
        }
        let program_c = sample_program(&mut interner_c)?;

        // The raw entry-function-name ids MUST differ across all three —
        // proves the drift is real, not an accident of matching histories.
        let entry_name_raw = |p: &Program| -> u32 {
            p.modules
                .first()
                .and_then(|m| m.funcs.first())
                .expect("sample_program always builds one module with one func")
                .name
                .as_raw()
        };
        let raw_a = entry_name_raw(&program_a);
        let raw_b = entry_name_raw(&program_b);
        let raw_c = entry_name_raw(&program_c);
        assert!(
            raw_a != raw_b && raw_b != raw_c && raw_a != raw_c,
            "the three interners' differing histories must produce three \
             different raw ids for this test to actually probe drift \
             (got a={raw_a}, b={raw_b}, c={raw_c})"
        );

        // Yet the STRUCTURAL, NAME-RESOLVED content is identical: the
        // relocated `program_b` (interner_b) reads exactly like the
        // never-serialized `program_c` (interner_c).
        let dump_b = {
            let guard = interner_b
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            pretty(&program_b, &guard)
        };
        let dump_c = pretty(&program_c, &interner_c);
        assert_eq!(
            dump_b, dump_c,
            "a Program relocated across a simulated process boundary must be \
             structurally/name-identical to a fresh, never-serialized \
             construction in an unrelated interner"
        );
        // Sanity: the dump actually contains the meaningful names (not an
        // accidental empty-string comparison).
        assert!(dump_b.contains("Increment") && dump_b.contains("Decrement"));
        assert!(dump_b.contains("main"));
        Ok(())
    }
}
