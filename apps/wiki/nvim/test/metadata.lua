vim.opt.runtimepath:prepend(vim.fn.getcwd() .. "/apps/wiki/nvim")
vim.g.wiki_socket = vim.fn.getcwd() .. "/target/wiki-auto.sock"
vim.g.wiki_server_cmd = { vim.fn.getcwd() .. "/target/debug/wiki-server" }
vim.g.wiki_lsp = false
vim.cmd("runtime! plugin/wiki.lua")

vim.cmd("WikiNew メタデータ")
local buf = vim.api.nvim_get_current_buf()
vim.cmd("WikiTitle 新しい名前")
assert(vim.b[buf].wiki_title == "新しい名前", vim.b[buf].wiki_title)
vim.cmd("WikiTags a,b")
assert(vim.b[buf].wiki_tags == "a,b", vim.b[buf].wiki_tags)
local rev = vim.b[buf].wiki_rev
vim.api.nvim_buf_set_lines(buf, 0, 1, false, { "# after metadata" })
vim.cmd("write")
assert(vim.b[buf].wiki_rev > rev, "save after metadata should not conflict")
vim.cmd("WikiDelete!")
assert(not vim.api.nvim_buf_is_valid(buf), "buffer should be wiped")
print("metadata: title/tags/delete OK")
vim.cmd("qa!")
