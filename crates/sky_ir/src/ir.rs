//! The typed IR node definitions (M0 subset). Widened in later milestones; for
//! M0 the surface is deliberately narrow so that every constructible value is a
//! well-formed program fragment.

use std::collections::{BTreeMap, BTreeSet};

use sky_diagnostics::{DResult, Diagnostic};
use sky_intern::Symbol;

/// A dotted module path, e.g. `Main` or `Sky.Core.Io`, as interned segments in
/// source order.
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub struct ModPath(pub Vec<Symbol>);

/// A function identifier, unique within a [`Program`].
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
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
#[derive(Clone, PartialEq, Debug)]
pub struct Program {
    pub modules: Vec<Module>,
}

/// A single module: its declared types and functions, plus an optional entry
/// point (the `main` function, when this module carries it).
//
// `Eq` is not derived: `funcs` hold [`Expr`] bodies that may carry a float
// literal (only `PartialEq`).
#[derive(Clone, PartialEq, Debug)]
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
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum TypeDef {
    Enum(EnumDef),
}

/// An enum (algebraic data type) declaration.
///
/// A variant may carry payload fields (M3a) and the type may be generic over a
/// list of type parameters (`type Maybe a = Just a | Nothing`). A nullary-only,
/// non-generic enum (`type Msg = Increment | Decrement`) has every variant's
/// `fields` empty and an empty `type_params` — that path stays byte-identical to
/// the M0 backend output.
#[derive(Clone, PartialEq, Eq, Debug)]
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
#[derive(Clone, PartialEq, Eq, Debug)]
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
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default, Hash)]
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
}

/// A function: the type variables it quantifies, typed parameters, a return
/// type, and a body expression.
//
// `Eq` is not derived: `body` is an [`Expr`] that may carry a float literal
// (only `PartialEq`).
#[derive(Clone, PartialEq, Debug)]
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
    /// structurally-parametric variable (M2a) carries [`BoundSet::UNBOUNDED`],
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
#[derive(Clone, PartialEq, Eq, Debug)]
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
    /// A generic type parameter — a Sky type variable used STRUCTURALLY
    /// (pass-through, no operation applied to it) in a fully-parametric
    /// top-level function (M2a). The carried [`Symbol`] is the source type
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
    /// bound — `Number` / `Comparable` / `Appendable`) and the wildcard `any`
    /// are NOT representable here: they are rejected at lowering (M2c) so every
    /// `Generic` the backend sees is a true parametric pass-through.
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
    // ── M6: Sky.Http.Server opaque types ────────────────────────────────────
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
    // ── #127: Sky.Http.Server.WebSocket opaque type handles ──────────────────
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
    /// dropped (D2 — see docs/architecture/websocket-server-design.md).
    WebSocketServerCfg,
    // ── M7: Std.Ui / Std.Html parametric types ──────────────────────────────
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
}

/// Tag enum for the message-parametric `Std.Ui` / `Std.Html` types.
///
/// Used inside [`IrType::Ui`] to select the correct Rust path at emission time.
/// The set is intentionally small to keep the pattern match exhaustive without a
/// catch-all.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
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
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
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
/// This is the authoritative soundness gate (#87) that keeps the *unconditional*
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
        | IrType::Order
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
        | IrType::ServerRequest
        | IrType::ServerResponse
        | IrType::ServerRoute
        | IrType::ServerCookie
        // `StreamWriter` derives Clone+Copy+Debug but not PartialEq — not fully derivable.
        | IrType::StreamWriter
        // `HttpRequest` is an opaque handle — not fully derivable.
        | IrType::HttpRequest
        // #127: `WsHandle` / `WsServerCfg` are opaque handles — not fully derivable.
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
        | IrType::Order => true,
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
        | IrType::ServerRequest
        | IrType::ServerResponse
        | IrType::ServerRoute
        | IrType::ServerCookie
        // `StreamWriter` is an opaque handle; not serde.
        | IrType::StreamWriter
        // `HttpRequest` is an opaque handle; not serde.
        | IrType::HttpRequest
        // #127: `WsHandle` / `WsServerCfg` are opaque handles; not serde.
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
#[derive(Clone, PartialEq, Debug)]
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
    /// A record literal `{ x = e1, y = e2, ... }`.
    ///
    /// The fields are carried as `(field name, value)` pairs sorted by field
    /// name, so the construction is deterministic. The backend resolves the
    /// literal's synthesised Rust struct from its field-name set; Rust names its
    /// struct-literal fields, so the emitted construction is order-independent.
    Record(Vec<(Symbol, Self)>),
    /// A record field access `record.field`.
    Access {
        record: Box<Self>,
        field: Symbol,
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
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Callee {
    Func(FuncId),
    Kernel(KernelFn),
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

/// Re-export of the `Std.Html.Events` payload-shape ADT (#107), so backend
/// crates that already depend on `sky_ir` (for `KernelFn`) can match on it
/// without taking a direct `sky_kernels` dependency.
pub use sky_kernels::HtmlEventShape;

/// Binary operators.
///
/// M0 shipped `Add`/`Sub`; M1 core widens the set with the remaining
/// arithmetic, comparison, and boolean operators. `Append` (`++`) carries
/// string concatenation; list `++` and `::` are deferred until the list type
/// lands.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
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
#[derive(Clone, PartialEq, Debug)]
pub struct Arm {
    pub pat: Pat,
    pub body: Expr,
}

/// A pattern.
///
/// M3a supports a constructor pattern whose payload sub-patterns bind to a
/// variable ([`Pat::Var`]) or are ignored ([`Pat::Wildcard`]). Nullary
/// constructor patterns (M0) are [`Pat::Ctor`] with an empty `args`. M3b-1 adds
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
#[derive(Clone, PartialEq, Eq, Debug)]
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
    /// A list / cons pattern, flattened to a Rust slice-pattern shape (M4a): a
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

/// Whether a pattern matches EVERY value of its scrutinee type — a wildcard, a
/// variable binder, or an alias whose inner pattern is itself irrefutable. Used
/// to prove a flat `match`'s trailing arm is a genuine catch-all.
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
        if !trailing_catch_all && !bool_complete && !all_ctor_headed && !all_list_shaped {
            return Err(Diagnostic::CompilerBug {
                where_: "sky_ir::Match::new_flat",
                detail: "flat match is not structurally exhaustive (no trailing \
                         catch-all, not a complete Bool cover, not a \
                         constructor-headed cover, and not a list cover)"
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

    /// Decompose the `Match` into its raw parts for structural transformation.
    ///
    /// Used by `sky_lower`'s `rewrite_var_to_apply` to rewrite arm bodies
    /// in-place without re-running the structural validation in
    /// [`Self::new`] / [`Self::new_flat`] (the pattern shapes are unchanged,
    /// so all invariants those constructors verify still hold).
    #[must_use]
    pub fn into_parts(self) -> (Box<Expr>, Vec<Arm>) {
        (self.scrutinee, self.arms)
    }

    /// Rebuild a `Match` from raw parts without re-running structural
    /// validation. Only safe when the patterns are unmodified — the structural
    /// invariants checked by [`Self::new`] / [`Self::new_flat`] are over the
    /// arm pattern shapes, not over the arm bodies. Body rewrites (e.g.
    /// variable-to-apply substitution) never change pattern shapes, so this
    /// stays sound.
    #[must_use]
    pub const fn from_parts_unchecked(scrutinee: Box<Expr>, arms: Vec<Arm>) -> Self {
        Self { scrutinee, arms }
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
            },
            Arm {
                pat: Pat::Ctor {
                    home: ModPath(vec![]),
                    ty,
                    variant: inc,
                    args: vec![],
                },
                body: Expr::Int(1),
            },
            Arm {
                pat: Pat::Ctor {
                    home: ModPath(vec![]),
                    ty,
                    variant: dec,
                    args: vec![],
                },
                body: Expr::Int(2),
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
            },
            Arm {
                pat: Pat::Ctor {
                    home: ModPath(vec![]),
                    ty,
                    variant: inc,
                    args: vec![],
                },
                body: Expr::Int(1),
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
            },
            Arm {
                pat: Pat::Ctor {
                    home: ModPath(vec![]),
                    ty,
                    variant: bogus,
                    args: vec![],
                },
                body: Expr::Int(1),
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
            },
            Arm {
                pat: Pat::Int(1),
                body: Expr::Int(1),
            },
            Arm {
                pat: Pat::Wildcard,
                body: Expr::Int(9),
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
            },
            Arm {
                pat: Pat::Alias(Box::new(Pat::Var(m)), k),
                body: Expr::Var(k),
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
            },
            Arm {
                pat: Pat::Bool(false),
                body: Expr::Int(0),
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
            },
            Arm {
                pat: Pat::Ctor {
                    home: ModPath(vec![]),
                    ty: opt,
                    variant: non,
                    args: vec![],
                },
                body: Expr::Int(1),
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
            },
            Arm {
                pat: Pat::Int(1),
                body: Expr::Int(1),
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
                }],
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
            },
            Arm {
                pat: Pat::Ctor {
                    home: ModPath(vec![]),
                    ty: maybe,
                    variant: nothing,
                    args: vec![],
                },
                body: Expr::Int(0),
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

    // ── #91 seal: ir_type_is_serde ─────────────────────────────────────────

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
            assert!(
                !ir_type_is_serde(&t, &all_serde),
                "{t:?} must NOT be serde"
            );
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
        assert!(!ir_type_is_serde(&e, &|_, _| false), "non-serde enum poisons");
    }
}
