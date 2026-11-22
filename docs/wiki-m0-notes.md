# Wiki M0 spike notes

Date: 2026-11-22

Screenshot: [wiki-m0-screen.png](wiki-m0-screen.png)

## What works

- `crates/wiki-core` parses `!` fenced blocks, resolves `ledger` / `hledger`
  presets, carries `kinds`, and round-trips the canonical
  `preset|kinds||id` form. 12 unit tests pass.
- `apps/wiki-server` serves two pages and two snippets over a Unix-socket
  JSON-RPC line protocol. `list`, `open` and `save` work; `save` checks the
  revision.
- `apps/wiki/nvim` opens a page as Markdown and a snippet as `ledger`, saves
  through `BufWriteCmd`, and updates the revision.
- `apps/wiki-gui` renders the page, the embedded snippet, the sidebar and the
  provider list with AntiWebUI. It loads the data from the server, not from
  a hard-coded copy.
- External Neovim integration was verified over RPC: `:WikiOpen` opened the
  snippet with `wiki://<id>` / `filetype=ledger`, `:write` saved it, and the
  server revision advanced.

## Findings

- AntiWebUI renders Japanese and the dark theme correctly, but text color is
  not inherited automatically. `text()` defaults to black; every panel needs
  an explicit `text_color(theme().text)` or the text disappears on dark
  backgrounds.
- `anti_webui::app::headless::render_png` is a good screenshot and CI path.
  It needs a working wgpu adapter; on this NixOS host, lavapipe plus
  `VK_ICD_FILENAMES` worked.
- The real Wayland/X11 window surface was not verified in this sandbox. The
  niri group/focus behavior still needs a desktop run.
- The three-column layout is readable, but sparse at 1440x900. It needs a
  max article width, a status bar, scrolling, and better sidebar ordering.
  The `Neovide で開く` button uses the default widget style.
- The page/snippet data model works with only the syntax layer. Snippet
  creation, kind Write permission, and partial failures are still M4 work.
- Neovim buffers use `wiki://<id>` and `acwrite`; no swap file is created.
  The plugin does not use LSP yet.

## Next

1. Run the GUI on niri next to Neovide; check focus, group movement, SKK and
   window layout presets.
2. Replace the fake server with the real App API (OAuth, Query, Subscribe,
   local cache).
3. Add snippet creation / `!` fence conversion with kind permission checks.
4. Add `wiki-lsp` MVP.
5. Move the snapshot into a repeatable check.
