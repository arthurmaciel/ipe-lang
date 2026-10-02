//! Names the root `clippy.toml` thread-spawn bans this crate's own code could
//! call, so rustc resolves each path and clippy's `unfulfilled_lint_expectations`
//! proves the ban still fires (mirrors the runtime's own seal,
//! `src/runtime/rust/src/clippy_paths_resolve.rs`, which carries the same
//! two namings against its own `clippy.toml`).

const _STD_THREAD_SPAWN: () = {
    #[expect(clippy::disallowed_methods)]
    let _ = ::std::thread::spawn::<fn(), ()>;
};

// `Scope::spawn` is a generic inherent method over the scope's own lifetime
// parameters; a bare path to it does not type as a const value, so the ban is
// named by calling it in a body that is never invoked.
fn _scope_spawn_seal(s: &::std::thread::Scope<'_, '_>) {
    #[expect(clippy::disallowed_methods)]
    let _ = s.spawn(|| ());
}
