vim.opt.runtimepath:prepend(vim.fn.getcwd() .. "/apps/wiki/nvim")
vim.g.wiki_socket = vim.fn.getcwd() .. "/target/wiki-m0.sock"
vim.g.wiki_server_cmd = { vim.fn.getcwd() .. "/target/debug/wiki-server" }
vim.g.wiki_lsp = false
vim.cmd("runtime! plugin/wiki.lua")
vim.cmd("edit wiki://p-home")
print("name=[" .. vim.api.nvim_buf_get_name(0) .. "] bt=" .. vim.bo.buftype)
print("id=" .. tostring(vim.b.wiki_id) .. " ft=" .. vim.bo.filetype)
assert(vim.api.nvim_buf_get_name(0) == "wiki://p-home")
assert(vim.b.wiki_id == "p-home")
assert(vim.bo.filetype == "markdown")
print("bufread: " .. vim.api.nvim_buf_get_name(0))
vim.cmd("qa!")
