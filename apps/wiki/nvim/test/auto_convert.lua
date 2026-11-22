-- Headless test: saving a page with a loose `!` fence auto-converts it into a
-- snippet and rewrites the host buffer to the canonical body-less fence.
vim.opt.runtimepath:prepend(vim.fn.getcwd() .. "/apps/wiki/nvim")
vim.g.wiki_socket = vim.fn.getcwd() .. "/target/wiki-auto.sock"
vim.g.wiki_server_cmd = { vim.fn.getcwd() .. "/target/debug/wiki-server" }
vim.g.wiki_lsp = false
vim.cmd("runtime! plugin/wiki.lua")

vim.cmd("WikiOpen p-home")
local buf = vim.api.nvim_get_current_buf()
assert(vim.b[buf].wiki_id == "p-home")
vim.api.nvim_buf_set_lines(buf, 0, -1, false, {
  "# Auto converted",
  "",
  "!```rust|code",
  "fn main() {",
  "    println!(\"hi\");",
  "}",
  "```",
  "",
})

vim.cmd("write")
local body = table.concat(vim.api.nvim_buf_get_lines(buf, 0, -1, false), "\n") .. "\n"
assert(body:find("!```rust|code||%x+"), body)
assert(body:find("fn main", 1, true) == nil, body)
assert(vim.b[buf].wiki_rev >= 2, tostring(vim.b[buf].wiki_rev))
print("auto-convert: canonical host OK\n" .. body)
vim.cmd("qa!")
