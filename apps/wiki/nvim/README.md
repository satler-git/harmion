# wiki.nvim

Neovim integration for the Harmion Wiki app. It talks only to `wiki-server`;
the server owns credentials, edit tokens and the local index. Buffers are
virtual `wiki://<content-id>` documents and are saved with `BufWriteCmd`.

## Setup

```lua
vim.opt.runtimepath:prepend("/path/to/harmion/apps/wiki/nvim")
vim.g.wiki_socket = "/path/to/harmion/target/wiki.sock"
vim.g.wiki_server_cmd = { "/path/to/harmion/target/debug/wiki-server" }
vim.g.wiki_lsp_cmd = { "/path/to/harmion/target/debug/wiki-lsp" }
```

`wiki-server` and `wiki-lsp` must be running / executable. The LSP is optional;
if it fails, the plugin keeps editing and saving through JSON-RPC.

## Commands

| Command | Action |
|---|---|
| `:WikiOpen [id-or-title]` | Open a page/snippet as `wiki://<id>` |
| `:WikiNew [title]` | Create a page and open it |
| `:WikiTitle <title>` | Rename the current Content |
| `:WikiTags a,b` | Replace tags |
| `:WikiDelete!` | Delete the current Content (revision-checked) |
| `:WikiConflict` | Diff the body and list conflicting fields after a 409 |
| `:WikiResolve` | Save the merged buffer against the current server revision |
| `:WikiSave` | `:write` through the server |
| `:WikiList` | Quickfix list of pages and snippets |
| `:WikiSearch <query>` | Quickfix search |
| `:WikiWorkspace <id-or-title>` | Quickfix outline + 2-hop links |
| `:WikiGoTo <id> <line>` | Switch to the page and move the cursor (used by the GUI outline) |
| `:WikiLspStart` | Start/attach the LSP explicitly |

The LSP starts automatically when a Wiki buffer is opened. It provides
diagnostics, definition, references, completion and hover for
`[[...]]`, `![[...]]`, `!` fences and interpreter calls. It never writes.

## Test

```sh
# with a server on target/wiki-m0.sock
nvim --headless -u NONE -i NONE -c 'luafile apps/wiki/nvim/test/m0.lua'
# auto-convert test (server on target/wiki-auto.sock):
nvim --headless -u NONE -i NONE -c 'luafile apps/wiki/nvim/test/auto_convert.lua'
nvim --headless -u NONE -i NONE -c 'luafile apps/wiki/nvim/test/new_page.lua'
nvim --headless -u NONE -i NONE -c 'luafile apps/wiki/nvim/test/autosave.lua'
nvim --headless -u NONE -i NONE -c 'luafile apps/wiki/nvim/test/metadata.lua'
nvim --headless -u NONE -i NONE -c 'luafile apps/wiki/nvim/test/bufread.lua'
nvim -n --headless -u NONE -i NONE -c 'luafile apps/wiki/nvim/test/conflict.lua'
```

The test opens `p-home`, edits it, saves it, then opens a ledger snippet and
checks `filetype=ledger`.

## Profile and cache

The plugin uses the editor's normal profile. It does not set `NVIM_APPNAME`
and does not create a separate Wiki config. When Neovide is started from the
GUI, the manager removes repository-local `XDG_CACHE_HOME` / `XDG_STATE_HOME` /
`XDG_DATA_HOME` values (the development-shell workaround) so the editor uses
the usual `~/.cache/nvim`, `~/.local/state/nvim` and `~/.local/share/nvim`.
Wiki-specific state is limited to the dedicated `--listen` socket and the
`WIKI_SOCKET` / `WIKI_NVIM_SERVER` environment variables.
