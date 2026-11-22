vim.opt.runtimepath:prepend(vim.fn.getcwd() .. "/apps/wiki/nvim")
vim.g.wiki_socket = vim.fn.getcwd() .. "/target/wiki-auto.sock"
vim.g.wiki_server_cmd = { vim.fn.getcwd() .. "/target/debug/wiki-server" }
vim.g.wiki_lsp = false
vim.cmd("runtime! plugin/wiki.lua")
vim.cmd("WikiNew テストページ")
assert(vim.b.wiki_type == "page")
assert(vim.b.wiki_id ~= nil)
assert(vim.api.nvim_buf_get_name(0):match("^wiki://"))
print("new page: " .. vim.b.wiki_id)
vim.cmd("qa!")
