# Wiki M0 slice

This is the first executable slice of the Wiki app. It is intentionally small:
the server serves fixtures, the GUI is read-only, and editing happens in an
external Neovim.

## Pieces

- [`crates/wiki-core`](../../crates/wiki-core): `!` fenced blocks, preset/kinds,
  the canonical `preset|kinds||id` form, and snippet extraction.
- [`apps/wiki-server`](../wiki-server): Unix-socket JSON-RPC server with fixed
  fixtures and an in-memory `save`.
- [`apps/wiki-gui`](../wiki-gui): AntiWebUI screen. It reads the page and the
  embedded snippet from the server. The `Neovide で開く` button calls
  `nvim --server ... --remote-send`.
- [`apps/wiki/nvim`](nvim): `:WikiOpen`, `:WikiSave`, `:WikiList`.

## Run

```sh
nix develop
cargo build -p harmion-wiki-server -p harmion-wiki-gui
./target/debug/wiki-server serve --socket target/wiki-m0.sock
WIKI_SOCKET=target/wiki-m0.sock ./target/debug/wiki-gui
```

The GUI is read-only. Clicking `Neovide で開く` expects a listening Neovim:

```sh
nvim --headless -u NONE -i NONE --listen target/nvim.sock \
  -c 'set rtp+='"$PWD"'/apps/wiki/nvim' \
  -c 'lua vim.g.wiki_socket=vim.fn.getcwd().."/target/wiki-m0.sock"; vim.g.wiki_server_cmd={vim.fn.getcwd().."/target/debug/wiki-server"}; vim.cmd("runtime! plugin/wiki.lua")'
```

Then, in another shell:

```sh
nvim --server target/nvim.sock --remote-send ':WikiOpen p-home<CR>'
nvim --server target/nvim.sock --remote-expr 'bufname("%")'
nvim --server target/nvim.sock --remote-expr '&filetype'
```

For interactive use, put the plugin directory on `runtimepath`, set
`vim.g.wiki_socket` and `vim.g.wiki_server_cmd`, and run `:WikiOpen p-home`.
The buffer uses `wiki://<id>` and `BufWriteCmd` saves through the server.

## Snapshot

`wiki-gui --snapshot PATH` renders the same layout offscreen and writes a PNG.
On NixOS with a Mesa lavapipe ICD:

```sh
VK_ICD_FILENAMES=/run/opengl-driver/share/vulkan/icd.d/lvp_icd.x86_64.json \
WGPU_BACKEND=vulkan \
WIKI_SOCKET=target/wiki-m0.sock \
./target/debug/wiki-gui --snapshot docs/wiki-m0-screen.png
```

The current screenshot is [docs/wiki-m0-screen.png](../../docs/wiki-m0-screen.png).

## Not in M0

- Real App API, OAuth, Query/Subscribe, persistence.
- Snippet creation and kind Write checks.
- niri window groups and focus sync.
- LSP.
- GUI editing (deliberately).
