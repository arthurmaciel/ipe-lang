//! The typed IR node definitions (M0 subset). Widened in later milestones; for
//! M0 the surface is deliberately narrow so that every constructible value is a
//! well-formed program fragment.

use std::collections::{BTreeMap, BTreeSet};

use sky_diagnostics::{DResult, Diagnostic};
use sky_intern::Symbol;

/// A dotted module path, e.g. `Main` or `Sky.Core.Io`, as interned segments in
/// source order.
#[derive(Clone, PartialEq, Eq, Debug)]
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
    /// `name` is the enum's bare type [`Symbol`]; `args` are the concrete type
    /// arguments at a use site (`Maybe Int` → `args = [Int]`, rendered
    /// `MainMaybe<i64>`). A non-generic enum (`Msg`) carries an empty `args`
    /// list, so it renders as the bare Rust type name — byte-identical to the M0
    /// backend. An `arg` may itself be an [`IrType::Generic`] when a generic
    /// enum is passed through a generic function (`Maybe a` inside a parametric
    /// signature → `MainMaybe<T1>`).
    Enum {
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
    /// A constructor application `Variant arg0 arg1 …` (a nullary constructor
    /// `Variant` has an empty `args`).
    ///
    /// `ty` is the constructor's enum type [`Symbol`]; `variant` the constructor
    /// name. `args` are the payload expressions, one per declared field, in
    /// source order. The backend resolves the variant's declared field types
    /// from the enum declaration to wrap any direct-self-recursive field in
    /// `Box::new` at construction (matching the boxed enum field).
    Ctor {
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
}

/// The target of a [`Expr::Call`].
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Callee {
    Func(FuncId),
    Kernel(KernelFn),
}

/// Built-in kernel functions — the standard-library surface that lowers to a
/// runtime call rather than to user Sky code.
///
/// The `String` / `Log` trio is the M0 set. M4a adds the `Sky.Core.List`,
/// `Sky.Core.Maybe`, and `Sky.Core.Result` combinators that stay kernel-anchored
/// (the higher-order ones — `map` / `filter` / `foldl` / `foldr` — exactly as the
/// reference compiler keeps them, because a cross-module polymorphic HOF needs
/// monomorphisation the front end does not yet perform; routing them to the
/// generic runtime functions sidesteps that). Each variant names one runtime
/// function (see the backend's `kernel_name`); the argument order at the call
/// site is the Sky order, which the backend re-points to the runtime's order
/// where the two differ.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum KernelFn {
    StringFromInt,
    /// `String.fromFloat : Float -> String` — the float counterpart of
    /// [`KernelFn::StringFromInt`]; renders an `f64` to its decimal text.
    StringFromFloat,
    // ── String kernels — arity 1 ─────────────────────────────────────────────
    /// `String.length : String -> Int` — Unicode rune count.
    StringLength,
    /// `String.isEmpty : String -> Bool`.
    StringIsEmpty,
    /// `String.reverse : String -> String`.
    StringReverse,
    /// `String.toUpper : String -> String`.
    StringToUpper,
    /// `String.toLower : String -> String`.
    StringToLower,
    /// `String.casefold : String -> String` — Unicode case-fold for locale-neutral comparison.
    StringCasefold,
    /// `String.trim : String -> String` — remove leading and trailing whitespace.
    StringTrim,
    /// `String.trimStart : String -> String` — remove leading whitespace.
    StringTrimStart,
    /// `String.trimEnd : String -> String` — remove trailing whitespace.
    StringTrimEnd,
    /// `String.toInt : String -> Maybe Int`.
    StringToInt,
    /// `String.toFloat : String -> Maybe Float`.
    StringToFloat,
    /// `String.fromChar : Char -> String`.
    StringFromChar,
    /// `String.fromList : List Char -> String`.
    StringFromList,
    /// `String.concat : List String -> String`.
    StringConcat,
    /// `String.words : String -> List String`.
    StringWords,
    /// `String.lines : String -> List String`.
    StringLines,
    /// `String.toList : String -> List Char`.
    StringToList,
    /// `String.isEmail : String -> Bool`.
    StringIsEmail,
    /// `String.isUrl : String -> Bool`.
    StringIsUrl,
    // ── String kernels — arity 2 ─────────────────────────────────────────────
    /// `String.append : String -> String -> String`.
    StringAppend,
    /// `String.contains : String -> String -> Bool` — `contains sub haystack`.
    StringContains,
    /// `String.startsWith : String -> String -> Bool` — `startsWith prefix s`.
    StringStartsWith,
    /// `String.endsWith : String -> String -> Bool` — `endsWith suffix s`.
    StringEndsWith,
    /// `String.equalFold : String -> String -> Bool` — case-insensitive equality.
    StringEqualFold,
    /// `String.join : String -> List String -> String`.
    StringJoin,
    /// `String.split : String -> String -> List String` — `split sep s`.
    StringSplit,
    /// `String.repeat : Int -> String -> String`.
    StringRepeat,
    /// `String.dropLeft : Int -> String -> String` — rune-based.
    StringDropLeft,
    /// `String.dropRight : Int -> String -> String` — rune-based.
    StringDropRight,
    // ── String kernels — arity 3 ─────────────────────────────────────────────
    /// `String.replace : String -> String -> String -> String` — `replace old new s`.
    StringReplace,
    /// `String.slice : Int -> Int -> String -> String` — rune-indexed.
    StringSlice,
    /// `String.padLeft : Int -> Char -> String -> String`.
    StringPadLeft,
    /// `String.padRight : Int -> Char -> String -> String`.
    StringPadRight,
    // ── Char kernels — arity 1 ──────────────────────────────────────────────
    /// `Char.isAlpha : Char -> Bool`.
    CharIsAlpha,
    /// `Char.isDigit : Char -> Bool`.
    CharIsDigit,
    /// `Char.isLower : Char -> Bool`.
    CharIsLower,
    /// `Char.isUpper : Char -> Bool`.
    CharIsUpper,
    /// `Char.toLower : Char -> String`.
    CharToLower,
    /// `Char.toUpper : Char -> String`.
    CharToUpper,
    /// `Char.toCode : Char -> Int`.
    CharToCode,
    /// `Char.fromCode : Int -> Char`.
    CharFromCode,
    LogPrintln,
    /// `List.map : (a -> b) -> List a -> List b`.
    ListMap,
    /// `List.filter : (a -> Bool) -> List a -> List a`.
    ListFilter,
    /// `List.foldl : (a -> b -> b) -> b -> List a -> b` — left fold.
    ListFoldl,
    /// `List.foldr : (a -> b -> b) -> b -> List a -> b` — right fold.
    ListFoldr,
    /// `List.length : List a -> Int`.
    ListLength,
    /// `List.head : List a -> Maybe a`.
    ListHead,
    /// `List.tail : List a -> Maybe (List a)`.
    ListTail,
    /// `List.member : a -> List a -> Bool`.
    ListMember,
    /// `List.range : Int -> Int -> List Int` — inclusive on both ends.
    ListRange,
    /// `List.reverse : List a -> List a`.
    ListReverse,
    /// `Maybe.withDefault : a -> Maybe a -> a`.
    MaybeWithDefault,
    /// `Maybe.map : (a -> b) -> Maybe a -> Maybe b`.
    MaybeMap,
    /// `Maybe.andThen : (a -> Maybe b) -> Maybe a -> Maybe b`.
    MaybeAndThen,
    /// `Result.withDefault : a -> Result e a -> a`.
    ResultWithDefault,
    /// `Result.map : (a -> b) -> Result e a -> Result e b`.
    ResultMap,
    // ── Math kernels ────────────────────────────────────────────────────────
    /// `Math.min : a -> a -> a` — the lesser of two values under the polymorphic
    /// `comparable` ordering (Elm `Basics.min` semantics: `if a <= b then a else
    /// b`). Lowered to the runtime's generic `math_min<T: PartialOrd>`, which
    /// compares the arguments AT THEIR ACTUAL TYPE — `f64`/`i64`/`String`/`char`
    /// — and returns the original value unchanged. It deliberately does NOT route
    /// through any `Int` coercion: current Go (`Math_min`) compares via `AsInt`,
    /// truncating floats and rendering strings meaningless (anzellai/sky PR #136,
    /// open). We implement the correct polymorphic compare and record the
    /// divergence; parity auto-restores when #136 merges.
    MathMin,
    /// `Math.max : a -> a -> a` — the greater of two values; the `max` companion
    /// of [`KernelFn::MathMin`] (Elm `Basics.max`: `if a >= b then a else b`),
    /// with the same no-truncation, polymorphic-compare contract.
    MathMax,
    // ── Math kernels — arity 0 (constants) ──────────────────────────────────
    /// `Math.pi : Float` — π ≈ 3.141592653589793.
    MathPi,
    /// `Math.e : Float` — Euler's number ≈ 2.718281828459045.
    MathE,
    /// `Math.phi : Float` — the golden ratio ≈ 1.618033988749895.
    MathPhi,
    /// `Math.sqrt2 : Float` — √2 ≈ 1.4142135623730951.
    MathSqrt2,
    /// `Math.inf : Float` — positive infinity (`+Inf`).
    MathInf,
    /// `Math.nan : Float` — not-a-number (`NaN`; `nan == nan` is `False`).
    MathNan,
    // ── Math kernels — arity 1 (Int → Int) ──────────────────────────────────
    /// `Math.abs : Int -> Int` — absolute value; saturates at `i64::MAX` on overflow.
    MathAbs,
    // ── Math kernels — arity 1 (Float → Float) ──────────────────────────────
    /// `Math.sqrt : Float -> Float` — square root. `sqrt(-1.0)` → `NaN`.
    MathSqrt,
    /// `Math.cbrt : Float -> Float` — cube root.
    MathCbrt,
    /// `Math.exp : Float -> Float` — eˣ.
    MathExp,
    /// `Math.exp2 : Float -> Float` — 2ˣ.
    MathExp2,
    /// `Math.log : Float -> Float` — natural logarithm. `log(0.0)` → `-Inf`.
    MathLog,
    /// `Math.log2 : Float -> Float` — base-2 logarithm.
    MathLog2,
    /// `Math.log10 : Float -> Float` — base-10 logarithm.
    MathLog10,
    /// `Math.sin : Float -> Float` — sine (radians).
    MathSin,
    /// `Math.cos : Float -> Float` — cosine (radians).
    MathCos,
    /// `Math.tan : Float -> Float` — tangent (radians).
    MathTan,
    /// `Math.asin : Float -> Float` — arcsin. Out-of-domain → `NaN`.
    MathAsin,
    /// `Math.acos : Float -> Float` — arccos. Out-of-domain → `NaN`.
    MathAcos,
    /// `Math.atan : Float -> Float` — arctan (one-argument).
    MathAtan,
    /// `Math.sinh : Float -> Float` — hyperbolic sine.
    MathSinh,
    /// `Math.cosh : Float -> Float` — hyperbolic cosine.
    MathCosh,
    /// `Math.tanh : Float -> Float` — hyperbolic tangent.
    MathTanh,
    /// `Math.asinh : Float -> Float` — inverse hyperbolic sine.
    MathAsinh,
    /// `Math.acosh : Float -> Float` — inverse hyperbolic cosine.
    MathAcosh,
    /// `Math.atanh : Float -> Float` — inverse hyperbolic tangent.
    MathAtanh,
    // ── Math kernels — arity 1 (Float → Int) ────────────────────────────────
    /// `Math.floor : Float -> Int` — round toward −∞.
    MathFloor,
    /// `Math.ceil : Float -> Int` — round toward +∞.
    MathCeil,
    /// `Math.round : Float -> Int` — half-away-from-zero (Go `math.Round`).
    MathRound,
    /// `Math.trunc : Float -> Int` — truncate toward zero.
    MathTrunc,
    // ── Math kernels — arity 2 (Float → Float → Float) ──────────────────────
    /// `Math.pow : Float -> Float -> Float` — exponentiation `base^exp`.
    MathPow,
    /// `Math.hypot : Float -> Float -> Float` — √(a²+b²), avoiding overflow.
    MathHypot,
    /// `Math.atan2 : Float -> Float -> Float` — quadrant-aware arctan(y/x).
    MathAtan2,
    /// `Math.mod : Float -> Float -> Float` — modulo, result has dividend's sign (Go `math.Mod`).
    MathMod,
    /// `Math.remainder : Float -> Float -> Float` — IEEE 754 remainder (Go `math.Remainder`).
    MathRemainder,
    /// Internal: construct `Ok x` with the project error type (`SkyError`) pinned.
    ///
    /// Not a Sky-source kernel — the lowerer emits this for an `Ok` constructor
    /// whose `Result e a` error type `e` is still unconstrained after solving, so
    /// the emitted Rust has a concrete `SkyResult<SkyError, _>` instead of an
    /// ambiguous `SkyResult<_, _>` (which rustc rejects with E0282). It maps to
    /// the runtime's `ok_res` helper.
    ResultOkDefault,
    // ── Dict kernels (M4d) ──────────────────────────────────────────────────
    /// `Dict.empty : Dict k v` — the empty dictionary (arity 0).
    DictEmpty,
    /// `Dict.isEmpty : Dict k v -> Bool`.
    DictIsEmpty,
    /// `Dict.size : Dict k v -> Int`.
    DictSize,
    /// `Dict.keys : Dict k v -> List k` — all keys (sorted on Rust backend).
    DictKeys,
    /// `Dict.values : Dict k v -> List v` — all values (key-sorted on Rust backend).
    DictValues,
    /// `Dict.toList : Dict k v -> List (k, v)` — all pairs (key-sorted on Rust backend).
    DictToList,
    /// `Dict.fromList : List (k, v) -> Dict k v`.
    DictFromList,
    /// `Dict.get : k -> Dict k v -> Maybe v`.
    DictGet,
    /// `Dict.member : k -> Dict k v -> Bool`.
    DictMember,
    /// `Dict.remove : k -> Dict k v -> Dict k v`.
    DictRemove,
    /// `Dict.union : Dict k v -> Dict k v -> Dict k v` — left-biased merge.
    DictUnion,
    /// `Dict.map : (k -> v -> w) -> Dict k v -> Dict k w`.
    DictMap,
    /// `Dict.insert : k -> v -> Dict k v -> Dict k v`.
    DictInsert,
    /// `Dict.foldl : (k -> v -> a -> a) -> a -> Dict k v -> a`.
    DictFoldl,
    // ── Set kernels (M4d) ───────────────────────────────────────────────────
    /// `Set.empty : Set a` — the empty set (arity 0).
    SetEmpty,
    /// `Set.size : Set a -> Int`.
    SetSize,
    /// `Set.toList : Set a -> List a` — all elements (sorted on Rust backend).
    SetToList,
    /// `Set.fromList : List a -> Set a` — deduplicated.
    SetFromList,
    /// `Set.member : a -> Set a -> Bool`.
    SetMember,
    /// `Set.insert : a -> Set a -> Set a`.
    SetInsert,
    /// `Set.remove : a -> Set a -> Set a`.
    SetRemove,
    /// `Set.union : Set a -> Set a -> Set a`.
    SetUnion,
    /// `Set.intersect : Set a -> Set a -> Set a`.
    SetIntersect,
    /// `Set.diff : Set a -> Set a -> Set a`.
    SetDiff,
    // ── Bytes kernels (M4e) ─────────────────────────────────────────────────
    /// `Bytes.empty : Bytes` — the empty byte buffer (arity 0).
    BytesEmpty,
    /// `Bytes.length : Bytes -> Int` — byte count of the buffer.
    BytesLength,
    /// `Bytes.isEmpty : Bytes -> Bool`.
    BytesIsEmpty,
    /// `Bytes.fromString : String -> Bytes` — UTF-8 encode a Sky string into bytes.
    BytesFromString,
    /// `Bytes.toString : Bytes -> Maybe String` — UTF-8 decode bytes; `Nothing`
    /// when the buffer is not valid UTF-8.
    BytesToString,
    /// `Bytes.fromHex : String -> Maybe Bytes` — parse a lowercase/uppercase hex
    /// string into bytes; `Nothing` on any non-hex character or odd length.
    BytesFromHex,
    /// `Bytes.toHex : Bytes -> String` — hex-encode bytes (lowercase).
    BytesToHex,
    /// `Bytes.fromBase64 : String -> Maybe Bytes` — standard-base64 decode;
    /// `Nothing` on bad padding or non-base64 characters.
    BytesFromBase64,
    /// `Bytes.toBase64 : Bytes -> String` — standard-base64 encode.
    BytesToBase64,
    /// `Bytes.append : Bytes -> Bytes -> Bytes` — concatenate two byte buffers.
    BytesAppend,
    /// `Bytes.slice : Int -> Int -> Bytes -> Bytes` — byte-indexed slice with
    /// negative-index-from-end semantics (mirrors `String.slice`).
    BytesSlice,
    // ── Encoding kernels (M4f) ──────────────────────────────────────────────
    /// `Encoding.base64Encode : String -> String` — standard base64 encode
    /// (RFC 4648, with `=` padding). Mirrors Go `base64.StdEncoding.EncodeToString`.
    EncodingBase64Encode,
    /// `Encoding.base64Decode : String -> Result Error String` — standard
    /// base64 decode. Returns `Err` on invalid padding or non-base64 characters.
    EncodingBase64Decode,
    /// `Encoding.urlEncode : String -> String` — URL query-string encode.
    /// Mirrors Go `url.QueryEscape`: space → `+`, other non-alphanumerics →
    /// `%XX` percent-encoded.
    EncodingUrlEncode,
    /// `Encoding.urlDecode : String -> Result Error String` — URL
    /// query-string decode (`+` → space, `%XX` → byte). Returns `Err` on
    /// invalid percent-escape sequences.
    EncodingUrlDecode,
    /// `Encoding.hexEncode : String -> String` — lowercase hex encoding.
    /// Mirrors Go `hex.EncodeToString([]byte(s))`.
    EncodingHexEncode,
    /// `Encoding.hexDecode : String -> Result Error String` — hex decode.
    /// Returns `Err` on an odd-length string or any non-hex character.
    EncodingHexDecode,
    // ── Json.Encode kernels (M4g) ──────────────────────────────────────────────
    /// `JsonEnc.string : String -> Value` — wrap a `String` as a JSON string value.
    JsonEncString,
    /// `JsonEnc.int : Int -> Value` — wrap an `Int` as a JSON number value.
    JsonEncInt,
    /// `JsonEnc.float : Float -> Value` — wrap a `Float` as a JSON number value.
    JsonEncFloat,
    /// `JsonEnc.bool : Bool -> Value` — wrap a `Bool` as a JSON boolean value.
    JsonEncBool,
    /// `JsonEnc.null : Value` — the JSON null constant (arity 0).
    JsonEncNull,
    /// `JsonEnc.list : (a -> Value) -> List a -> Value` — encode a list with a
    /// per-element encoder (Elm-shaped: encoder first, list second).
    JsonEncList,
    /// `JsonEnc.object : List (String, Value) -> Value` — build a JSON object from
    /// key-value pairs. Key order follows Go: `json.Marshal(map[string]any{})`
    /// sorts keys alphabetically via `BTreeMap` in the Rust runtime.
    JsonEncObject,
    /// `JsonEnc.encode : Int -> Value -> String` — serialise a `Value` to JSON text.
    /// `indent=0` → compact (no whitespace); `indent=N` → N-space pretty-print
    /// matching Go's `json.MarshalIndent(val, "", strings.Repeat(" ", N))`.
    JsonEncEncode,
    // ── Json.Decode kernels (M4h) ──────────────────────────────────────────────
    /// `JsonDec.string : Decoder String` — primitive string decoder (arity 0).
    JsonDecString,
    /// `JsonDec.int : Decoder Int` — primitive integer decoder (arity 0).
    JsonDecInt,
    /// `JsonDec.float : Decoder Float` — primitive float decoder (arity 0).
    JsonDecFloat,
    /// `JsonDec.bool : Decoder Bool` — primitive boolean decoder (arity 0).
    JsonDecBool,
    /// `JsonDec.decodeString : Decoder a -> String -> Result Error a` — run a
    /// decoder against a raw JSON string.
    JsonDecDecodeString,
    /// `JsonDec.field : String -> Decoder a -> Decoder a` — decode the named
    /// object field.
    JsonDecField,
    /// `JsonDec.at : List String -> Decoder a -> Decoder a` — decode through a
    /// nested field path.
    JsonDecAt,
    /// `JsonDec.index : Int -> Decoder a -> Decoder a` — decode the n-th array
    /// element.
    JsonDecIndex,
    /// `JsonDec.list : Decoder a -> Decoder (List a)` — decode a JSON array into
    /// a `List` by applying `Decoder a` to each element.
    JsonDecList,
    /// `JsonDec.map : (a -> b) -> Decoder a -> Decoder b` — transform a decoded
    /// value.
    JsonDecMap,
    /// `JsonDec.andThen : (a -> Decoder b) -> Decoder a -> Decoder b` — chain
    /// decoders.  Sky arg order: fn first; Rust runtime arg order: decoder first.
    /// `kernel_swaps_first_two` reverses the two args at emit time.
    JsonDecAndThen,
    /// `JsonDec.succeed : a -> Decoder a` — a decoder that always succeeds with
    /// the given value.  When the argument is a function, the backend wraps it
    /// with `curry_N` so the Rust factory contract is met.
    JsonDecSucceed,
    /// `JsonDec.fail : String -> Decoder a` — a decoder that always fails with the
    /// given message.
    JsonDecFail,
    /// `JsonDec.oneOf : List (Decoder a) -> Decoder a` — try each decoder in
    /// order; succeed with the first to match.
    JsonDecOneOf,
    /// `JsonDec.map2 : (a -> b -> c) -> Decoder a -> Decoder b -> Decoder c`.
    JsonDecMap2,
    /// `JsonDec.map3 : (a -> b -> c -> d) -> Decoder a -> Decoder b -> Decoder c -> Decoder d`.
    JsonDecMap3,
    /// `JsonDec.map4 : (a -> b -> c -> d -> e) -> Decoder a -> Decoder b -> Decoder c -> Decoder d -> Decoder e`.
    JsonDecMap4,
    // ── Json.Decode.Pipeline kernels (M4h) ────────────────────────────────────
    /// `Pipeline.required : String -> Decoder a -> Decoder (a -> b) -> Decoder b`.
    JsonDecPRequired,
    /// `Pipeline.optional : String -> Decoder a -> a -> Decoder (a -> b) -> Decoder b`.
    JsonDecPOptional,
    /// `Pipeline.custom : Decoder a -> Decoder (a -> b) -> Decoder b`.
    JsonDecPCustom,
    /// `Pipeline.requiredAt : List String -> Decoder a -> Decoder (a -> b) -> Decoder b`.
    JsonDecPRequiredAt,

    // ── Crypto kernels (M5a) ─────────────────────────────────────────────────
    /// `Crypto.sha256 : String -> String`
    CryptoSha256,
    /// `Crypto.sha512 : String -> String`
    CryptoSha512,
    /// `Crypto.sha1 : String -> String`
    CryptoSha1,
    /// `Crypto.md5 : String -> String`
    CryptoMd5,
    /// `Crypto.hmacSha256 : String -> String -> String`
    CryptoHmacSha256,
    /// `Crypto.hmacSha512 : String -> String -> String`
    CryptoHmacSha512,
    /// `Crypto.rsaSha256Sign : String -> String -> Result Error String`
    CryptoRsaSha256Sign,
    /// `Crypto.rsaSha256Verify : String -> String -> String -> Bool`
    CryptoRsaSha256Verify,
    /// `Crypto.constantTimeEqual : String -> String -> Bool`
    CryptoConstantTimeEqual,
    /// `Crypto.aesGcmEncrypt : String -> String -> Result Error String`
    CryptoAesGcmEncrypt,
    /// `Crypto.aesGcmDecrypt : String -> String -> Result Error String`
    CryptoAesGcmDecrypt,
    /// `Crypto.chacha20Encrypt : String -> String -> Result Error String`
    CryptoChacha20Encrypt,
    /// `Crypto.chacha20Decrypt : String -> String -> Result Error String`
    CryptoChacha20Decrypt,
    /// `Crypto.aesKeyFromPassword : String -> String -> String`
    CryptoAesKeyFromPassword,
    /// `Crypto.chachaKeyFromPassword : String -> String -> String`
    CryptoChachaKeyFromPassword,
    /// `Crypto.randomBytes : Int -> Task Error String`
    CryptoRandomBytes,
    /// `Crypto.randomToken : Int -> Task Error String`
    CryptoRandomToken,

    // ── Uuid kernels (M5b) ─────────────────────────────────────────────────
    /// `Uuid.v4 : String` — random UUID v4 (CSPRNG). Arity 0.
    UuidV4,
    /// `Uuid.v7 : String` — time-ordered UUID v7. Arity 0.
    /// SECURITY: v7 embeds a millisecond timestamp — sortable and NOT a secret.
    UuidV7,
    /// `Uuid.parse : String -> Maybe String` — canonicalise or Nothing. Arity 1.
    UuidParse,

    // ── Jwt kernels (M5b) ──────────────────────────────────────────────────
    /// `Jwt.encodeHs256 : String -> String -> Result Error String`
    /// `encodeHs256 secret claimsJson` — HMAC-SHA256 signed JWT.
    /// Secret must be ≥32 bytes (RFC 7518 §3.2). Arity 2.
    JwtEncodeHs256,
    /// `Jwt.decodeHs256 : String -> String -> Result Error String`
    /// `decodeHs256 secret token` — verify signature + exp/nbf, return claims JSON.
    /// Secret must be ≥32 bytes (RFC 7518 §3.2). Arity 2.
    JwtDecodeHs256,
    /// `Jwt.encodeRs256 : String -> String -> Result Error String`
    /// `encodeRs256 privateKeyPem claimsJson` — RSA-SHA256 signed JWT. Arity 2.
    JwtEncodeRs256,
    /// `Jwt.decodeRs256 : String -> String -> Result Error String`
    /// `decodeRs256 publicKeyPem token` — verify RS256 signature + exp/nbf. Arity 2.
    JwtDecodeRs256,

    // ── Task combinators (M5a) ────────────────────────────────────────────────
    /// `Task.succeed : a -> Task Error a` — lift a pure value into a task. Arity 1.
    TaskSucceed,
    /// `Task.fail : Error -> Task Error a` — a task that immediately fails. Arity 1.
    TaskFail,
    /// `Task.map : (a -> b) -> Task Error a -> Task Error b` — transform the success value. Arity 2.
    TaskMap,
    /// `Task.andThen : (a -> Task Error b) -> Task Error a -> Task Error b` — sequential composition. Arity 2.
    TaskAndThen,
    /// `Task.mapError : (Error -> Error) -> Task Error a -> Task Error a`. Arity 2.
    TaskMapError,
    /// `Task.onError : (Error -> Task Error a) -> Task Error a -> Task Error a`. Arity 2.
    TaskOnError,
    /// `Task.fromResult : Result Error a -> Task Error a`. Arity 1.
    TaskFromResult,
    /// `Task.andThenResult : (a -> Result Error b) -> Task Error a -> Task Error b`. Arity 2.
    TaskAndThenResult,
    /// `Task.sequence : List (Task Error a) -> Task Error (List a)`. Arity 1.
    TaskSequence,
    /// `Task.parallel : List (Task Error a) -> Task Error (List a)`. Arity 1.
    TaskParallel,
    /// `Task.run : Task Error a -> Result Error a` — run a task synchronously. Arity 1.
    TaskRun,

    // ── Io kernels (M5a) ──────────────────────────────────────────────────────
    /// `Io.readLine : () -> Task Error String` — read one line from stdin. Arity 1.
    IoReadLine,
    /// `Io.writeStdout : String -> Task Error ()` — write to stdout. Arity 1.
    IoWriteStdout,
    /// `Io.writeStderr : String -> Task Error ()` — write to stderr. Arity 1.
    IoWriteStderr,

    // ── Time kernels (M5a) ────────────────────────────────────────────────────
    /// `Time.now : () -> Task Error Int` — current Unix milliseconds. Arity 1.
    TimeNow,
    /// `Time.sleep : Int -> Task Error ()` — sleep for N milliseconds. Arity 1.
    TimeSleep,
    /// `Time.unixMillis : () -> Task Error Int` — alias of `Time.now`. Arity 1.
    TimeUnixMillis,

    // ── System kernels (M5a) ──────────────────────────────────────────────────
    /// `System.args : () -> Task Error (List String)` — command-line arguments. Arity 1.
    SystemArgs,
    /// `System.getenv : String -> Task Error String` — read env var or fail. Arity 1.
    SystemGetenv,
    /// `System.getenvOr : String -> String -> String` — read env var with fallback (pure). Arity 2.
    SystemGetenvOr,
    /// `System.getArg : Int -> Task Error (Maybe String)` — nth command-line arg. Arity 1.
    SystemGetArg,
    /// `System.getenvInt : String -> Task Error Int` — env var parsed as Int. Arity 1.
    SystemGetenvInt,
    /// `System.getenvBool : String -> Task Error Bool` — env var parsed as Bool. Arity 1.
    SystemGetenvBool,
    /// `System.setenv : String -> String -> Task Error ()` — set an env var. Arity 2.
    SystemSetenv,
    /// `System.unsetenv : String -> Task Error ()` — unset an env var. Arity 1.
    SystemUnsetenv,
    /// `System.cwd : () -> Task Error String` — current working directory. Arity 1.
    SystemCwd,
    /// `System.loadEnv : () -> Task Error ()` — load `.env` file. Arity 1.
    SystemLoadEnv,
    /// `System.exit : Int -> a` — terminate the process (diverging). Arity 1.
    SystemExit,

    // ── Random kernels (M5a) ──────────────────────────────────────────────────
    /// `Random.int : Int -> Int -> Task Error Int` — random int in `[lo, hi]`. Arity 2.
    RandomInt,
    /// `Random.float : Float -> Float -> Task Error Float` — random float. Arity 2.
    RandomFloat,
    /// `Random.choice : List a -> Task Error a` — random element. Arity 1.
    RandomChoice,

    // ── File kernels (M5a) ────────────────────────────────────────────────────
    /// `File.readFile : String -> Task Error String`. Arity 1.
    FileReadFile,
    /// `File.writeFile : String -> String -> Task Error ()`. Arity 2.
    FileWriteFile,
    /// `File.exists : String -> Task Error Bool`. Arity 1.
    FileExists,
    /// `File.remove : String -> Task Error ()` — remove a file. Arity 1.
    FileRemove,
    /// `File.mkdirAll : String -> Task Error ()` — mkdir -p. Arity 1.
    FileMkdirAll,
    /// `File.readFileLimit : String -> Int -> Task Error String`. Arity 2.
    FileReadFileLimit,
    /// `File.readFileBytes : String -> Task Error (List Int)`. Arity 1.
    FileReadFileBytes,
    /// `File.append : String -> String -> Task Error ()`. Arity 2.
    FileAppend,
    /// `File.readDir : String -> Task Error (List String)`. Arity 1.
    FileReadDir,
    /// `File.isDir : String -> Task Error Bool`. Arity 1.
    FileIsDir,
    /// `File.tempFile : String -> Task Error String`. Arity 1.
    FileTempFile,
    /// `File.tempDir : String -> Task Error String`. Arity 1.
    FileTempDir,
    /// `File.copy : String -> String -> Task Error ()`. Arity 2.
    FileCopy,
    /// `File.rename : String -> String -> Task Error ()`. Arity 2.
    FileRename,
    /// `File.delete : String -> Task Error ()` — alias of `File.remove`. Arity 1.
    FileDelete,
    // ── Http kernels (M5b) ──────────────────────────────────────────────────
    /// `Http.get : String -> Task Error HttpResponse` — arity 1.
    ///
    /// Routes through `sky_runtime::http_client::http_get`; the SSRF guard,
    /// body cap, timeout floor, and error redaction are all inside the runtime.
    /// The returned `sky_runtime::HttpResponse` is converted to the synthesised
    /// Sky record struct `{body, headers, status}` via an inline `task_map`
    /// closure in the emitter (Design B — no nominal runtime-struct bridge).
    HttpGet,
    /// `Http.post : String -> String -> Task Error HttpResponse` — arity 2.
    ///
    /// First arg is the URL, second is the body string. Same runtime guards as
    /// `Http.get`; same `task_map` conversion on the response.
    HttpPost,
    /// `Http.request : HttpRequest -> Task Error HttpResponse` — arity 1.
    ///
    /// Accepts the full `HttpRequest` record (`{method, url, body, headers,
    /// timeout, followRedirects, maxRedirects}`), field-for-field converted to
    /// `sky_runtime::HttpRequest` before the runtime call. The emitter binds
    /// the synthesised struct to `__req` to avoid partial-move hazards.
    HttpRequest,
    /// `Http.parseQuery : String -> Dict String String` — arity 1, pure.
    ///
    /// Trims a leading `?`, splits on `&`/`=`, percent-decodes, first-key-wins.
    /// No turbofish needed: `http_parse_query` returns `HashMap<String,String>`
    /// which is `Dict String String` — the standard `Expr::Call` path is correct.
    HttpParseQuery,
    // ── Http builder kernels (M5b) ───────────────────────────────────────────
    /// `Http.defaultRequest : String -> HttpRequest` — arity 1, pure.
    ///
    /// Constructs an `HttpRequest` with sensible defaults: `method = "GET"`,
    /// `body = ""`, `headers = []`, `timeout = 30000`, `followRedirects = true`,
    /// `maxRedirects = 10`. Emitted as an inline struct literal — no runtime call.
    HttpDefaultRequest,
    /// `Http.withMethod : String -> HttpRequest -> HttpRequest` — arity 2, pure.
    ///
    /// Returns the request with the `method` field replaced. Emitted as a
    /// clone-and-reassign block matching the `emit_update` pattern.
    HttpWithMethod,
    /// `Http.withTimeout : Int -> HttpRequest -> HttpRequest` — arity 2, pure.
    ///
    /// Returns the request with the `timeout` field replaced (milliseconds).
    HttpWithTimeout,
    /// `Http.withBody : String -> HttpRequest -> HttpRequest` — arity 2, pure.
    ///
    /// Returns the request with the `body` field replaced.
    HttpWithBody,
    /// `Http.withHeader : String -> String -> HttpRequest -> HttpRequest` — arity 3, pure.
    ///
    /// PREPENDS `(key, value)` to the request's `headers` list — latest-added
    /// appears first (cons-prepend), matching the Go reference implementation.
    HttpWithHeader,

    // ── Db kernels (M5b-db) ──────────────────────────────────────────────────
    /// `Db.connect : () -> Task Error Db` — connect via `SKY_DB_URL`. Arity 1.
    DbConnect,
    /// `Db.open : String -> String -> Task Error Db` — `open driver path`. Arity 2.
    DbOpen,
    /// `Db.close : Db -> Task Error ()` — return pool to registry. Arity 1.
    DbClose,
    /// `Db.execRaw : Db -> String -> Task Error Int` — execute SQL, no params. Arity 2.
    DbExecRaw,
    /// `Db.exec : Db -> String -> List SqlValue -> Task Error Int` — parameterised exec. Arity 3.
    ///
    /// The `List SqlValue` arg is projected to `Vec<SqlParam>` via the generated
    /// `StdDbSqlValue::into_sql_param()` at the call site before reaching the
    /// runtime's `db_exec_params`.
    DbExec,
    /// `Db.query : Db -> String -> List SqlValue -> Task Error (List (Dict String String))`. Arity 3.
    ///
    /// Same `List SqlValue` → `Vec<SqlParam>` projection as [`KernelFn::DbExec`].
    DbQuery,
    /// `Db.queryDecode : Db -> String -> List SqlValue -> Decoder a -> Task Error (List a)`. Arity 4.
    ///
    /// Routes through `db_query_decode_params`.  The `List SqlValue` arg is
    /// projected to `Vec<SqlParam>`; the decoder is passed through as-is.
    DbQueryDecode,
    /// `Db.getString : String -> Dict String String -> String` — pure row accessor. Arity 2.
    DbGetString,
    /// `Db.getInt : String -> Dict String String -> Int` — pure row accessor. Arity 2.
    DbGetInt,
    /// `Db.getBool : String -> Dict String String -> Bool` — pure row accessor. Arity 2.
    DbGetBool,
    /// `Db.getField : String -> Dict String String -> String` — pure row accessor; returns `""` when absent. Arity 2.
    DbGetField,
    /// `Db.insertRow : Db -> String -> List (String, String) -> Task Error Int`. Arity 3.
    DbInsertRow,
    /// `Db.getById : Db -> String -> String -> Task Error (Maybe (Dict String String))`. Arity 3.
    DbGetById,
    /// `Db.updateById : Db -> String -> String -> List (String, String) -> Task Error Int`. Arity 4.
    DbUpdateById,
    /// `Db.deleteById : Db -> String -> String -> Task Error Int`. Arity 3.
    DbDeleteById,
    /// `Db.findOneByField : Db -> String -> String -> String -> Task Error (Maybe (Dict String String))`. Arity 4.
    DbFindOneByField,
    /// `Db.findManyByField : Db -> String -> String -> String -> Task Error (List (Dict String String))`. Arity 4.
    DbFindManyByField,
    /// `Db.findByConditions : Db -> String -> Dict String String -> Task Error (List (Dict String String))`. Arity 3.
    ///
    /// The `Dict String String` arg maps column names to equality values for AND-joined
    /// WHERE conditions. The runtime receives it as `HashMap<String, String>`.
    DbFindByConditions,
    /// `Db.unsafeFindWhere : Db -> String -> String -> List String -> Task Error (List (Dict String String))`. Arity 4.
    ///
    /// The `List String` arg provides parameterized SQL bindings (`?` placeholders)
    /// for the WHERE clause — the sole sanctioned raw-SQL path. Callers MUST pass
    /// all dynamic values through this parameter, never via string interpolation.
    DbUnsafeFindWhere,
    /// `Db.insertFields : Db -> String -> List (String, SqlField) -> Task Error Int`. Arity 3.
    ///
    /// The `List (String, SqlField)` arg is projected to `Vec<(String, Option<SqlParam>)>`
    /// via `StdDbSqlField::into_field_param()` at the call site.
    DbInsertFields,
    /// `Db.updateFields : Db -> String -> List (String, SqlValue) -> List (String, SqlField) -> Task Error Int`. Arity 4.
    DbUpdateFields,
    /// `Db.insertFieldsReturning : Db -> String -> List (String, SqlField) -> String -> Decoder a -> Task Error (List a)`. Arity 5.
    DbInsertFieldsReturning,
    /// `Db.withTransaction : Db -> (Db -> Task Error a) -> Task Error a`. Arity 2.
    DbWithTransaction,
    /// `Db.migrate : Db -> List (String, String) -> Task Error (List String)`. Arity 2.
    DbMigrate,

    // ── Db.Decode kernels (M5b-db) ───────────────────────────────────────────
    /// `Db.Decode.string : String -> Decoder String` — column string decoder. Arity 1.
    DbDecString,
    /// `Db.Decode.int : String -> Decoder Int` — column integer decoder. Arity 1.
    DbDecInt,
    /// `Db.Decode.float : String -> Decoder Float` — column float decoder. Arity 1.
    DbDecFloat,
    /// `Db.Decode.bool : String -> Decoder Bool` — column boolean decoder. Arity 1.
    DbDecBool,
    /// `Db.Decode.nullable : Decoder a -> Decoder (Maybe a)` — nullable wrapper. Arity 1.
    ///
    /// v0.16.24 breaking change: no leading column-name arg; inner decoder's
    /// column set is used for NULL detection.
    DbDecNullable,
    /// `Db.Decode.map : (a -> b) -> Decoder a -> Decoder b`. Arity 2.
    DbDecMap,
    /// `Db.Decode.andThen : (a -> Decoder b) -> Decoder a -> Decoder b`. Arity 2.
    DbDecAndThen,
    /// `Db.Decode.succeed : a -> Decoder a` — decoder that always succeeds. Arity 1.
    DbDecSucceed,
    /// `Db.Decode.fail : String -> Decoder a` — decoder that always fails. Arity 1.
    DbDecFail,
    /// `Db.Decode.map2 : (a -> b -> c) -> Decoder a -> Decoder b -> Decoder c`. Arity 3.
    DbDecMap2,
    /// `Db.Decode.map3 : (a -> b -> c -> d) -> Decoder a -> Decoder b -> Decoder c -> Decoder d`. Arity 4.
    DbDecMap3,
    /// `Db.Decode.map4 : (a -> b -> c -> d -> e) -> Decoder a -> Decoder b -> Decoder c -> Decoder d -> Decoder e`. Arity 5.
    DbDecMap4,
    /// `Db.Decode.required : String -> Decoder a -> Decoder (a -> b) -> Decoder b`. Arity 3.
    DbDecRequired,
    /// `Db.Decode.optional : String -> Decoder a -> a -> Decoder (a -> b) -> Decoder b`. Arity 4.
    DbDecOptional,

    // ── M5c: TEA Cmd / Sub / Time.every ────────────────────────────────────
    //
    // These are CONSTRUCT-ONLY in M5c — they create opaque `SkyCmd<M>` /
    // `SkySub<M>` values; the TEA dispatch loop lands in M6.
    // All live in the ungated `runtime/src/sky_runtime/tea.rs` (no `live`
    // feature required for construction).
    /// `Cmd.none : Cmd msg` — the no-op command.  Arity 0.
    CmdNone,
    /// `Cmd.batch : List (Cmd msg) -> Cmd msg` — sequence multiple commands.  Arity 1.
    CmdBatch,
    /// `Cmd.perform : Task Error a -> (Result Error a -> msg) -> Cmd msg` — lift a
    /// task into a command.  Arity 2.
    CmdPerform,
    /// `Sub.none : Sub msg` — the empty subscription.  Arity 0.
    SubNone,
    /// `Sub.batch : List (Sub msg) -> Sub msg` — combine subscriptions.  Arity 1.
    SubBatch,
    /// `Sub.every : Int -> msg -> Sub msg` — tick subscription (ms interval).  Arity 2.
    SubEvery,
    /// `Time.every : Int -> msg -> Sub msg` — alias for `Sub.every`.  Arity 2.
    TimeEvery,

    // ── M6 reserved: live-feature-gated TEA kernels ─────────────────────────
    //
    // These variants are declared here so that the exhaustiveness checker can
    // flag any `match k` that omits them, but they are NOT wired in M5c.
    // Attempting to emit them returns a hard `CompilerBug` error.
    //
    // `Cmd.publish` / `Cmd.publishNoEcho` push a topic message back into the
    // TEA broker from inside `update`; wiring requires the broker handle
    // (available in M6 via `live/pubsub.rs`).
    //
    // `Sub.subscribeTopic` / `PubSub.publish` / `PubSub.publishNoEcho`
    // similarly depend on the running broker; deferred to M6.
    /// `Cmd.publish : String -> a -> Cmd msg` — reserved for M6.  Do not emit.
    CmdPublish,
    /// `Cmd.publishNoEcho : String -> a -> Cmd msg` — reserved for M6.  Do not emit.
    CmdPublishNoEcho,
    /// `Sub.subscribeTopic : String -> (a -> msg) -> Sub msg` — reserved for M6.  Do not emit.
    SubSubscribeTopic,
    /// `PubSub.publish : String -> a -> Task Error ()` — reserved for M6.  Do not emit.
    PubSubPublish,
    /// `PubSub.publishNoEcho : String -> a -> Task Error ()` — reserved for M6.  Do not emit.
    PubSubPublishNoEcho,

    // ── M6: Sky.Http.Server kernels ─────────────────────────────────────────
    /// `Server.get : String -> (Request -> Task Error Response) -> Route`
    ServerGet,
    /// `Server.post : String -> (Request -> Task Error Response) -> Route`
    ServerPost,
    /// `Server.put : String -> (Request -> Task Error Response) -> Route`
    ServerPut,
    /// `Server.delete : String -> (Request -> Task Error Response) -> Route`
    ServerDelete,
    /// `Server.any : String -> (Request -> Task Error Response) -> Route`
    ServerAny,
    /// `Server.api : String -> (Request -> Task Error Response) -> Route`
    ServerApi,
    /// `Server.static : String -> String -> Route` (path, directory)
    ServerStatic,
    /// `Server.listen : Int -> List Route -> Task Error ()`
    ServerListen,
    /// `Server.text : String -> Response`
    ServerText,
    /// `Server.json : String -> Response`
    ServerJson,
    /// `Server.html : String -> Response`
    ServerHtml,
    /// `Server.withStatus : Int -> Response -> Response`
    ServerWithStatus,
    /// `Server.withHeader : String -> String -> Response -> Response`
    ServerWithHeader,
    /// `Server.redirect : String -> Response`
    ServerRedirect,
    /// `Server.param : String -> Request -> Maybe String`
    ServerParam,
    /// `Server.queryParam : String -> Request -> Maybe String`
    ServerQueryParam,
    /// `Server.header : String -> Request -> Maybe String`
    ServerHeader,
    /// `Server.getCookie : String -> Request -> Maybe String`
    ServerGetCookie,
    /// `Server.body : Request -> String`
    ServerBody,
    /// `Server.path : Request -> String`
    ServerPath,
    /// `Server.method : Request -> String`
    ServerMethod,
    /// `Server.cookie : String -> String -> Cookie`
    ServerCookieNew,
    /// `Server.withCookie : Cookie -> Response -> Response`
    ServerWithCookie,
    /// `Middleware.withCors : List String -> (Request -> Task Error Response) -> (Request -> Task Error Response)`
    MiddlewareWithCors,
    /// `Middleware.withLogging : (Request -> Task Error Response) -> (Request -> Task Error Response)`
    MiddlewareWithLogging,
    /// `Middleware.withBasicAuth : String -> String -> (Request -> Task Error Response) -> (Request -> Task Error Response)`
    MiddlewareWithBasicAuth,
    /// `Middleware.withRateLimit : String -> Int -> Int -> (Request -> Task Error Response) -> (Request -> Task Error Response)`
    MiddlewareWithRateLimit,
    /// `RateLimit.allow : String -> String -> Int -> Int -> Bool`
    RateLimitAllow,
}

impl KernelFn {
    /// Return `true` when this variant is one of the `Db*` kernel variants
    /// (including `DbDec*`).
    ///
    /// This is the single authoritative list — `sky_lower` and
    /// `sky_backend_rust` both import it rather than maintaining independent
    /// copies.
    ///
    /// # Exhaustiveness note
    ///
    /// `matches!` expands to a `match` with an implicit `_ => false` arm, so
    /// adding a new `KernelFn::Db*` variant does NOT automatically yield a
    /// compiler warning here.  Callers that need to detect an unlisted Db
    /// variant (e.g. `emit_db_call`'s hardening guard) MUST use this as a
    /// *guard* inside their own exhaustive `match`, so the Rust compiler's
    /// exhaustiveness checker can flag the gap at compile time.
    #[must_use]
    pub const fn is_db(self) -> bool {
        matches!(
            self,
            Self::DbConnect
                | Self::DbOpen
                | Self::DbClose
                | Self::DbExecRaw
                | Self::DbExec
                | Self::DbQuery
                | Self::DbQueryDecode
                | Self::DbGetString
                | Self::DbGetInt
                | Self::DbGetBool
                | Self::DbGetField
                | Self::DbInsertRow
                | Self::DbGetById
                | Self::DbUpdateById
                | Self::DbDeleteById
                | Self::DbFindOneByField
                | Self::DbFindManyByField
                | Self::DbFindByConditions
                | Self::DbUnsafeFindWhere
                | Self::DbInsertFields
                | Self::DbUpdateFields
                | Self::DbInsertFieldsReturning
                | Self::DbWithTransaction
                | Self::DbMigrate
                | Self::DbDecString
                | Self::DbDecInt
                | Self::DbDecFloat
                | Self::DbDecBool
                | Self::DbDecNullable
                | Self::DbDecMap
                | Self::DbDecAndThen
                | Self::DbDecSucceed
                | Self::DbDecFail
                | Self::DbDecMap2
                | Self::DbDecMap3
                | Self::DbDecMap4
                | Self::DbDecRequired
                | Self::DbDecOptional
        )
    }

    /// Return `true` when this variant is one of the TEA (`Cmd` / `Sub` /
    /// `Time.every`) kernel variants introduced in M5c, **including** the M6
    /// reserved variants that must not be emitted yet.
    ///
    /// Used by `sky_lower` and `sky_backend_rust` to detect tea-kernel call
    /// sites and to guard the `emit_tea_call` hardening path.
    ///
    /// # Exhaustiveness note
    ///
    /// Same caveat as [`Self::is_db`]: `matches!` carries an implicit
    /// `_ => false` arm.  `emit_tea_call` uses this as a *guard* inside its
    /// own exhaustive `match` so the compiler flags any missing variant.
    #[must_use]
    pub const fn is_tea(self) -> bool {
        matches!(
            self,
            Self::CmdNone
                | Self::CmdBatch
                | Self::CmdPerform
                | Self::SubNone
                | Self::SubBatch
                | Self::SubEvery
                | Self::TimeEvery
                | Self::CmdPublish
                | Self::CmdPublishNoEcho
                | Self::SubSubscribeTopic
                | Self::PubSubPublish
                | Self::PubSubPublishNoEcho
        )
    }

    /// Return `true` when this variant is one of the `Sky.Http.Server` kernel
    /// variants introduced in M6.
    ///
    /// Used by `sky_lower` and `sky_backend_rust` to detect server-kernel call
    /// sites and to guard the `emit_server_call` hardening path.
    ///
    /// # Exhaustiveness note
    ///
    /// `matches!` carries an implicit `_ => false` arm.  `emit_server_call`
    /// uses this as a *guard* inside its own exhaustive `match` so the compiler
    /// flags any missing variant at compile time.
    #[must_use]
    pub const fn is_server(self) -> bool {
        matches!(
            self,
            Self::ServerGet
                | Self::ServerPost
                | Self::ServerPut
                | Self::ServerDelete
                | Self::ServerAny
                | Self::ServerApi
                | Self::ServerStatic
                | Self::ServerListen
                | Self::ServerText
                | Self::ServerJson
                | Self::ServerHtml
                | Self::ServerWithStatus
                | Self::ServerWithHeader
                | Self::ServerRedirect
                | Self::ServerParam
                | Self::ServerQueryParam
                | Self::ServerHeader
                | Self::ServerGetCookie
                | Self::ServerBody
                | Self::ServerPath
                | Self::ServerMethod
                | Self::ServerCookieNew
                | Self::ServerWithCookie
                | Self::MiddlewareWithCors
                | Self::MiddlewareWithLogging
                | Self::MiddlewareWithBasicAuth
                | Self::MiddlewareWithRateLimit
                | Self::RateLimitAllow
        )
    }
}

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
    /// nested ctor / tuple / record sub-patterns are all permitted).
    Ctor {
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
                    ty,
                    variant: inc,
                    args: vec![],
                },
                body: Expr::Int(0),
            },
            Arm {
                pat: Pat::Ctor {
                    ty,
                    variant: inc,
                    args: vec![],
                },
                body: Expr::Int(1),
            },
            Arm {
                pat: Pat::Ctor {
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
                    ty,
                    variant: inc,
                    args: vec![],
                },
                body: Expr::Int(0),
            },
            Arm {
                pat: Pat::Ctor {
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
                    ty,
                    variant: inc,
                    args: vec![],
                },
                body: Expr::Int(0),
            },
            Arm {
                pat: Pat::Ctor {
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
                        ty: opt,
                        variant: som,
                        args: vec![Pat::Ctor {
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
                    ty: opt,
                    variant: som,
                    args: vec![Pat::Ctor {
                        ty: opt,
                        variant: non,
                        args: vec![],
                    }],
                },
                body: Expr::Int(0),
            },
            Arm {
                pat: Pat::Ctor {
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
            BinOp::Eq,
            BinOp::Neq,
            BinOp::Lt,
            BinOp::Gt,
            BinOp::Le,
            BinOp::Ge,
            BinOp::And,
            BinOp::Or,
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
            name: maybe,
            args: vec![IrType::Int],
        };
        assert_eq!(use_ty, use_ty.clone());
        // A non-generic enum use carries no args and is distinct from the applied
        // form.
        let bare = IrType::Enum {
            name: maybe,
            args: vec![],
        };
        assert_ne!(use_ty, bare);

        // Construction `Just 5` carries its payload argument.
        let ctor = Expr::Ctor {
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
                    ty: maybe,
                    variant: just,
                    args: vec![Pat::Var(x)],
                },
                body: Expr::Var(x),
            },
            Arm {
                pat: Pat::Ctor {
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
            name: tree,
            args: vec![],
        };
        let def = EnumDef {
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
}
