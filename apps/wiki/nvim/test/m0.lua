vim.opt.runtimepath:prepend(vim.fn.getcwd() .. "/apps/wiki/nvim")
vim.cmd("runtime! plugin/wiki.lua")
vim.g.wiki_socket = vim.fn.getcwd() .. "/target/wiki-m0.sock"
vim.g.wiki_server_cmd = { vim.fn.getcwd() .. "/target/debug/wiki-server" }
vim.g.wiki_lsp_cmd = { vim.fn.getcwd() .. "/target/debug/wiki-lsp" }

vim.cmd("WikiOpen p-home")
local lines = vim.api.nvim_buf_get_lines(0, 0, -1, false)
assert(vim.b.wiki_id == "p-home", "expected p-home, got " .. tostring(vim.b.wiki_id))
assert(vim.bo.filetype == "markdown", "expected markdown, got " .. vim.bo.filetype)
assert(table.concat(lines, "\n"):find("家計の記録", 1, true), "page body missing")
print("opened p-home, buf=" .. vim.api.nvim_buf_get_name(0) .. ", rev=" .. vim.b.wiki_rev)

vim.api.nvim_buf_set_lines(0, 0, 1, false, { "# 家計の記録 (edited)" })
vim.cmd("write")
assert(vim.b.wiki_rev == 2, "expected rev 2, got " .. tostring(vim.b.wiki_rev))
print("saved p-home, rev=" .. vim.b.wiki_rev)

local first = vim.api.nvim_get_current_buf()
vim.cmd("WikiOpen p-home")
assert(vim.api.nvim_get_current_buf() == first, "reopen should reuse the existing buffer")

vim.cmd("WikiOpen 3f2a9c1e00000000000000000000000000000000000000000000000000000001")
assert(vim.bo.filetype == "ledger", "expected ledger, got " .. vim.bo.filetype)
assert(vim.api.nvim_buf_get_name(0):match("wiki://3f2a"), "unexpected buffer name")
print("opened snippet, ft=" .. vim.bo.filetype .. ", rev=" .. vim.b.wiki_rev)

vim.cmd("qa!")
