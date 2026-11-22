# Wiki app

A local-first personal wiki on Harmion. The design is frozen in
[docs/wiki-ui-design.html](../../docs/wiki-ui-design.html); the current
implementation status and platform gaps are in
[docs/wiki-implementation-status.md](../../docs/wiki-implementation-status.md).

## Layout

- [`crates/wiki-core`](../../crates/wiki-core): wiki syntax, links,
  transclusion, outline, canonical fences, link index.
- [`apps/wiki-server`](../wiki-server): Unix-socket JSON-RPC, local store,
  workspace/index, snippet conversion, export, dev node snapshot.
- [`apps/wiki-gui`](../wiki-gui): AntiWebUI shell.
- [`apps/wiki-lsp`](../wiki-lsp): stdio LSP for `wiki://<id>`.
- [`apps/wiki/nvim`](nvim): Neovim plugin and integration test.

## Run locally

```sh
nix develop
cargo build -p harmion-wiki-server -p harmion-wiki-gui -p harmion-wiki-lsp
./target/debug/wiki-server serve --socket target/wiki.sock --data target/wiki-data.json
WIKI_SOCKET=target/wiki.sock ./target/debug/wiki-gui
```

If the GUI cannot create a window, run it inside `nix develop`; the devShell
sets the Vulkan/EGL paths. `wiki-gui --snapshot PATH` renders offscreen.

Neovim:

```sh
nvim --headless -u NONE -i NONE --listen target/nvim.sock \
  -c 'set rtp+='"$PWD"'/apps/wiki/nvim' \
  -c 'lua vim.g.wiki_socket=vim.fn.getcwd().."/target/wiki.sock"; vim.g.wiki_server_cmd={vim.fn.getcwd().."/target/debug/wiki-server"}; vim.g.wiki_lsp_cmd={vim.fn.getcwd().."/target/debug/wiki-lsp"}; vim.cmd("runtime! plugin/wiki.lua")'
```

See [apps/wiki/nvim/README.md](nvim/README.md).

## Local API

`wiki-server` speaks one JSON object per line over a Unix socket:

- `list`, `open`, `workspace`, `search`, `save`
- `status`, `create_page`, `create_snippet`, `convert_fence`, `delete`,
  `transclude`, `export`, `conflicts`

CLI helpers: `list`, `open`, `workspace`, `search`, `save`, `status`,
`conflicts`, `transclude`, `export`.

## Dev node mode

`serve --node DIR` loads a read snapshot of `kind=wiki` Contents using the
node's `admin.secret` (`harmion-client`). It is intentionally read-only until
the OAuth token/refresh and edit-session path lands. Do not point production
credentials at this mode.

## Tests

```sh
cargo test -p harmion-wiki-core
cargo test -p harmion-wiki-server
cargo test -p harmion-wiki-lsp
# with a server on target/wiki-m0.sock:
nvim --headless -u NONE -i NONE -c 'luafile apps/wiki/nvim/test/m0.lua'
```
