vim.opt.runtimepath:prepend(vim.fn.getcwd() .. "/apps/wiki/nvim")
vim.g.wiki_socket = vim.fn.getcwd() .. "/target/wiki-m0.sock"
vim.g.wiki_server_cmd = { vim.fn.getcwd() .. "/target/debug/wiki-server" }
vim.g.wiki_lsp = false
vim.g.wiki_autosave = false
vim.cmd("runtime! plugin/wiki.lua")
vim.cmd("WikiOpen p-home")
local buf = vim.api.nvim_get_current_buf()
local rev = vim.b[buf].wiki_rev
-- Make the server revision newer behind the editor's back.
local cmd = { vim.g.wiki_server_cmd[1], "save", "p-home", "--rev", tostring(rev), "--socket", vim.g.wiki_socket }
local out = vim.fn.system(cmd, "# externally edited\n")
assert(vim.v.shell_error == 0, out)
vim.api.nvim_buf_set_lines(buf, 0, 1, false, { "# local edit" })
local write_ok = pcall(vim.cmd, "write")
assert(not write_ok, "conflict save should fail")
vim.wait(200, function() return vim.bo[buf].modified == true end, 20)
print("conflict var=" .. tostring(vim.b[buf].wiki_conflict_body) .. " modified=" .. tostring(vim.bo[buf].modified))
assert(vim.bo[buf].modified == true, "conflict must leave the buffer dirty")
assert(vim.b[buf].wiki_conflict_body ~= nil, "conflict body should be recorded")
vim.cmd("WikiConflict")
assert(#vim.api.nvim_list_wins() >= 2, "conflict diff should open two windows")
vim.api.nvim_set_current_buf(buf)
vim.api.nvim_buf_set_lines(buf, 0, -1, false, { "# manually merged" })
vim.cmd("WikiResolve")
assert(vim.b[buf].wiki_rev > rev, "resolve should advance the revision")
assert(vim.bo[buf].modified == false, "resolve should leave the buffer clean")
local opened = vim.fn.system({ vim.g.wiki_server_cmd[1], "open", "p-home", "--socket", vim.g.wiki_socket })
assert(opened:find("manually merged", 1, true), opened)
vim.cmd("qa!")
