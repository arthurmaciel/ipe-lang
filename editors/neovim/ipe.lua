-- Ipê support for Neovim 0.11+: filetype, tree-sitter highlighting, and the
-- `ipe lsp` client (completion, go-to-definition, code actions, formatting).
--
-- Installed by editors/neovim/configure.sh as a `plugin/` file under
-- stdpath("data")/site, next to the `parser/ipe` grammar and `queries/ipe/`
-- it uses; Neovim loads it at startup with no init.lua change.

if vim.g.loaded_ipe then
  return
end
vim.g.loaded_ipe = true

if vim.fn.has("nvim-0.11") == 0 then
  vim.notify("Ipê support needs Neovim 0.11 or newer", vim.log.levels.WARN)
  return
end

vim.filetype.add({ extension = { ipe = "ipe" } })

vim.lsp.config("ipe", {
  cmd = { "ipe", "lsp" },
  filetypes = { "ipe" },
  root_markers = { "package.ipe", ".git" },
})
vim.lsp.enable("ipe")

local group = vim.api.nvim_create_augroup("ipe", { clear = true })

vim.api.nvim_create_autocmd("FileType", {
  group = group,
  pattern = "ipe",
  callback = function(args)
    local bo = vim.bo[args.buf]
    bo.commentstring = "-- %s"
    bo.expandtab = true
    bo.shiftwidth = 4
    bo.tabstop = 4
    local ok, err = pcall(vim.treesitter.start, args.buf, "ipe")
    if not ok then
      vim.notify("Ipê highlighting unavailable: " .. tostring(err), vim.log.levels.WARN)
    end
  end,
})

vim.api.nvim_create_autocmd("LspAttach", {
  group = group,
  callback = function(args)
    local client = vim.lsp.get_client_by_id(args.data.client_id)
    if client and client.name == "ipe" and client:supports_method("textDocument/completion") then
      vim.lsp.completion.enable(true, client.id, args.buf, { autotrigger = true })
    end
  end,
})
