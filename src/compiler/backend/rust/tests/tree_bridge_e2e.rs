//! Recursive-ADT kernel-bridge SEAL proof (#2493).
//!
//! Hand-builds an IR program that:
//!   * calls the `Tree.demoTree` producer kernel (a kernel RETURNING a recursive,
//!     payload-carrying, runtime-backed stdlib ADT — the precedent-free capability
//!     this bridge adds);
//!   * CONSTRUCTS `Tree` values via its `Leaf` / `Node` constructors; and
//!   * RECURSIVELY `case`-matches them (two mutually-recursive helpers that walk
//!     the `Node (List Tree)` children and sum the `Leaf` payloads).
//!
//! The load-bearing proof is THE SEAL: under `IPE_E2E=1` the emitted crate must
//! `cargo build` AND run, printing the correct sum. `demoTree 5` builds
//! `Node [Leaf 5, Node [Leaf 6], Leaf 10]`, whose leaves sum to `21`.
//!
//! Gated on `IPE_E2E=1` so the default `cargo test` stays fast and offline.

mod seal_e2e;

use ipe_backend::Backend;
use ipe_backend_rust::RustBackend;
use ipe_diagnostics::DResult;
use ipe_intern::Interner;
use ipe_ir::{
    Arm, BinOp, CallPin, Callee, Expr, Func, FuncId, IrType, KernelFn, Match, ModPath, Module,
    OnFormKind, Pat, Program,
};

/// A `Module` with every `uses_*` flag false except the ones passed — here only
/// `uses_tree` (and `uses_async_runtime` for the `Io.println`/`Task` entry).
#[allow(clippy::missing_const_for_fn)] // nursery false-positive: owns runtime-built Vecs, not const-constructible
fn tree_module(name: ModPath, funcs: Vec<Func>, entry: FuncId) -> Module {
    Module {
        name,
        types: vec![],
        funcs,
        entry: Some(entry),
        records: vec![],
        uses_tea: false,
        uses_server: false,
        uses_http: false,
        uses_config: false,
        uses_compression: false,
        uses_csv: false,
        uses_cache: false,
        uses_tree: true,
        uses_encoding: false,
        uses_regex: false,
        uses_uuid: false,
        uses_random: false,
        uses_log: false,
        uses_decimal: false,
        uses_char_category: false,
        uses_crypto_core: false,
        uses_secret: false,
        uses_json: false,
        uses_crypto: false,
        uses_jwt: false,
        uses_url: false,
        uses_ui: false,
        uses_web: false,
        uses_tui: false,
        uses_console: false,
        uses_webview: false,
        uses_css: false,
        uses_auth: false,
        uses_principal: false,
        uses_websocket: false,
        uses_email: false,
        uses_locale: false,
        uses_time: false,
        uses_env_public: false,
        uses_debug: false,
        uses_ffi: false,
        uses_async_runtime: true,
    }
}

/// Build the whole program:
///   `sumTree : Tree -> Int`      (`FuncId` 0)  — recursion over the sum
///   `sumChildren : List Tree -> Int` (`FuncId` 1) — recursion over the children
///   `main = Io.println (String.fromInt (sumTree (Tree.demoTree 5)))` (`FuncId` 2)
#[allow(clippy::too_many_lines)] // one linear hand-built IR builder — the whole program in one place
fn tree_program(interner: &mut Interner) -> DResult<Program> {
    let main_mod = interner.intern("Main")?;
    let ipe = interner.intern("Ipe")?;
    let tree_mod = interner.intern("Tree")?;
    let tree_ty = interner.intern("Tree")?;
    let leaf = interner.intern("Leaf")?;
    let node = interner.intern("Node")?;
    let sum_tree = interner.intern("sumTree")?;
    let sum_children = interner.intern("sumChildren")?;
    let main = interner.intern("main")?;
    let t = interner.intern("t")?;
    let n = interner.intern("n")?;
    let kids = interner.intern("kids")?;
    let xs = interner.intern("xs")?;
    let h = interner.intern("h")?;
    let tl = interner.intern("tl")?;

    let home = ModPath(vec![main_mod]);
    let tree_home = ModPath(vec![ipe, tree_mod]);
    let tree_type = IrType::Enum {
        home: tree_home.clone(),
        name: tree_ty,
        args: vec![],
    };

    let call_func = |id: u32, args: Vec<Expr>| Expr::Call {
        callee: Callee::Func(FuncId::from_raw(id)),
        args,
        pin: CallPin::None,
        on_form: OnFormKind::NotForm,
    };

    // sumTree t = case t of { Leaf n -> n ; Node kids -> sumChildren kids }
    let sum_tree_body = {
        let scrutinee = Expr::Var(t);
        let leaf_arm = Arm {
            pat: Pat::Ctor {
                home: tree_home.clone(),
                ty: tree_ty,
                variant: leaf,
                args: vec![Pat::Var(n)],
            },
            guard: None,
            body: Expr::Var(n),
        };
        let node_arm = Arm {
            pat: Pat::Ctor {
                home: tree_home,
                ty: tree_ty,
                variant: node,
                args: vec![Pat::Var(kids)],
            },
            guard: None,
            body: call_func(1, vec![Expr::Var(kids)]),
        };
        Expr::Match(Match::new(
            scrutinee,
            vec![leaf_arm, node_arm],
            &[leaf, node],
        )?)
    };
    let sum_tree_fn = Func {
        id: FuncId::from_raw(0),
        name: sum_tree,
        home: home.clone(),
        type_params: vec![],
        row_params: vec![],
        params: vec![(t, tree_type.clone())],
        ret: IrType::Int,
        body: sum_tree_body,
    };

    // sumChildren xs = case xs of { [] -> 0 ; h :: tl -> sumTree h + sumChildren tl }
    let sum_children_body = {
        let scrutinee = Expr::Var(xs);
        let nil_arm = Arm {
            pat: Pat::Slice {
                prefix: vec![],
                rest: None,
            },
            guard: None,
            body: Expr::Int(0),
        };
        let cons_arm = Arm {
            pat: Pat::Slice {
                prefix: vec![Pat::Var(h)],
                rest: Some(Box::new(Pat::Var(tl))),
            },
            guard: None,
            body: Expr::BinOp {
                op: BinOp::IntAdd,
                lhs: Box::new(call_func(0, vec![Expr::Var(h)])),
                rhs: Box::new(call_func(1, vec![Expr::Var(tl)])),
            },
        };
        Expr::Match(Match::new_flat(scrutinee, vec![nil_arm, cons_arm])?)
    };
    let sum_children_fn = Func {
        id: FuncId::from_raw(1),
        name: sum_children,
        home: home.clone(),
        type_params: vec![],
        row_params: vec![],
        params: vec![(xs, IrType::List(Box::new(tree_type)))],
        ret: IrType::Int,
        body: sum_children_body,
    };

    // main = Io.println (String.fromInt (sumTree (Tree.demoTree 5)))
    let demo = Expr::Call {
        callee: Callee::Kernel(KernelFn::TreeDemo),
        args: vec![Expr::Int(5)],
        pin: CallPin::None,
        on_form: OnFormKind::NotForm,
    };
    let main_body = Expr::Call {
        callee: Callee::Kernel(KernelFn::IoPrintln),
        args: vec![Expr::Call {
            callee: Callee::Kernel(KernelFn::StringFromInt),
            args: vec![call_func(0, vec![demo])],
            pin: CallPin::None,
            on_form: OnFormKind::NotForm,
        }],
        pin: CallPin::None,
        on_form: OnFormKind::NotForm,
    };
    let main_fn = Func {
        id: FuncId::from_raw(2),
        name: main,
        home,
        type_params: vec![],
        row_params: vec![],
        params: vec![],
        ret: IrType::Task(Box::new(IrType::Unit)),
        body: main_body,
    };

    Ok(Program {
        imports_unsafe_submodule: false,
        imported_web_capabilities: std::collections::BTreeSet::new(),
        modules: vec![tree_module(
            ModPath(vec![main_mod]),
            vec![sum_tree_fn, sum_children_fn, main_fn],
            FuncId::from_raw(2),
        )],
    })
}

/// THE SEAL: emit the program, vendor the runtime beside it, `cargo build`, run,
/// and assert stdout is the correct recursive sum with a clean exit.
#[test]
fn recursive_tree_bridge_builds_and_runs() -> DResult<()> {
    if std::env::var("IPE_E2E").is_err() {
        return Ok(());
    }
    let Some(runtime) = seal_e2e::resolve_runtime() else {
        return Ok(());
    };

    let mut interner = Interner::new();
    let prog = tree_program(&mut interner)?;
    let emitted = RustBackend::new(&interner).emit(&prog)?;

    let out = std::env::temp_dir().join("ipe_tree_bridge_e2e");
    let _ = std::fs::remove_dir_all(&out);
    let src = out.join("src");
    std::fs::create_dir_all(&src).map_err(|e| seal_e2e::io_bug(&src, &e))?;
    seal_e2e::copy_dir(&runtime, &src.join("ipe_runtime"))?;

    let cargo_toml = out.join("Cargo.toml");
    std::fs::write(&cargo_toml, &emitted.cargo_toml)
        .map_err(|e| seal_e2e::io_bug(&cargo_toml, &e))?;
    for (rel, contents) in &emitted.files {
        let path = out.join(rel.as_str());
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| seal_e2e::io_bug(parent, &e))?;
        }
        std::fs::write(&path, contents).map_err(|e| seal_e2e::io_bug(&path, &e))?;
    }

    let target_dir = seal_e2e::emitted_run_target_dir(&out);
    let status = std::process::Command::new("cargo")
        .arg("build")
        .current_dir(&out)
        .env("CARGO_TARGET_DIR", &target_dir)
        .status();
    assert!(
        matches!(&status, Ok(s) if s.success()),
        "the emitted recursive-Tree bridge project must `cargo build` (THE SEAL): {status:?}"
    );

    let bin = target_dir.join("debug").join("ipe-app");
    let output = std::process::Command::new(&bin)
        .output()
        .map_err(|e| seal_e2e::io_bug(&bin, &e))?;
    assert_eq!(
        String::from_utf8_lossy(&output.stdout),
        "21\n",
        "sumTree (demoTree 5) = 5 + 6 + 10 = 21 — the constructed tree must build \
         AND recursively case-match correctly at runtime"
    );
    assert!(
        output.status.success(),
        "the recursive-Tree program must exit 0; got {:?}",
        output.status.code()
    );
    if target_dir == out.join("target") {
        let _ = std::fs::remove_dir_all(&target_dir);
    }
    Ok(())
}
