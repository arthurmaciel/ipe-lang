//! Zed extension for Ipê: launches `ipe lsp` from the project's `PATH`.
//!
//! Highlighting comes from the tree-sitter grammar declared in
//! `extension.toml`; this crate only supplies the language-server command.

use zed_extension_api::{self as zed, LanguageServerId, Result};

struct IpeExtension;

impl zed::Extension for IpeExtension {
    fn new() -> Self {
        Self
    }

    fn language_server_command(
        &mut self,
        _language_server_id: &LanguageServerId,
        worktree: &zed::Worktree,
    ) -> Result<zed::Command> {
        let command = worktree.which("ipe").ok_or_else(|| {
            "`ipe` is not on PATH — install it: https://github.com/ipe-lang/compiler".to_owned()
        })?;
        Ok(zed::Command {
            command,
            args: vec!["lsp".to_owned()],
            env: worktree.shell_env(),
        })
    }
}

zed::register_extension!(IpeExtension);
