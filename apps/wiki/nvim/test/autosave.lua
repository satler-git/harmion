vim.opt.runtimepath:prepend(vim.fn.getcwd() .. "/apps/wiki/nvim")
vim.g.wiki_socket = vim.fn.getcwd() .. "/target/wiki-auto.sock"
vim.g.wiki_server_cmd = { vim.fn.getcwd() .. "/target/debug/wiki-server" }
vim.g.wiki_lsp = false
vim.cmd("runtime! plugin/wiki.lua")

vim.cmd("WikiOpen p-home")
local buf = vim.api.nvim_get_current_buf()
local rev = vim.b[buf].wiki_rev
vim.api.nvim_buf_set_lines(buf, 0, 1, false, { "# 自動保存" })
vim.cmd("doautocmd TextChanged")
local saved = vim.wait(4000, function()
  return vim.b[buf].wiki_rev > rev
end, 50)
assert(saved, "autosave did not advance the revision")
assert(vim.bo[buf].modified == false, "buffer still modified after autosave")
print("autosave: rev " .. rev .. " -> " .. vim.b[buf].wiki_rev)
vim.cmd("qa!")
