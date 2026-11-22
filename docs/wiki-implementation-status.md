# Wiki implementation status

Last updated: 2026-11-23

## Frozen design

- [Dark native design](wiki-ui-design.html) / [PNG](wiki-ui-design.png)
- 4 columns: nav 236, article, outline 176 centered, inspector 340
- Tabs, inspector `リンク`/`情報`, bottom 2-hop shelf
- Ctrl shortcuts: `Ctrl+K` search, `Ctrl+E` Neovide, `Ctrl+I` info,
  `Ctrl+N` new, `Ctrl+P` quick open, `Ctrl+W` close tab,
  `Ctrl+1`/`Ctrl+2` inspector mode, `Ctrl+Alt+L` 2-hop shelf
- Code snippets must be syntax highlighted (FR-55)

## Implemented and verified

### `crates/wiki-core`

- `!` fence parsing, preset/language/kinds, canonical `preset|kinds||id`
- `[[target|label]]`, `![[target]]`, `![provider(args)]`
- Markdown outline extraction, code-fence masking
- Link index: outgoing, backlinks, 2-hop suggestions
- 17 unit tests pass

### `apps/wiki-server`

- Local-first Unix-socket JSON-RPC; existing `list/open/workspace/search/save`
- Atomic JSON persistence (`serve --data PATH`, 0600), in-memory fixtures by default
- `status`, `create_page`, `create_snippet`, `convert_fence`, `delete`,
  `transclude`, `export`, `conflicts`
- `workspace` returns page, before/after, snippet, outline, links,
  related snippets, recent, page info
- Dev-only `--node DIR` read snapshot using `harmion-client::Client::from_directory`
  (admin secret). Writes return 501 instead of guessing tokens.
- 28 + 1 tests pass (unit + integration)

### `apps/wiki-lsp`

- stdio LSP; `wiki://<id>` virtual documents; Content-Length framing
- diagnostics, definition, references, completion, hover
- Server unavailable still yields local diagnostics
- Unit tests pass; live probe against `wiki-server` confirmed initialize,
  diagnostics, definition to `wiki://p-goals`, references, completion
- No write path and no credentials/tokens

### `apps/wiki/nvim`

- `:WikiOpen`, `:WikiSave`, `:WikiList`, `:WikiSearch`, `:WikiWorkspace`,
  `:WikiLspStart`
- `wiki://<id>` buffers, `BufWriteCmd`, LSP auto-start
- Headless test opens p-home, saves rev 1 → 2, opens a ledger snippet with
  `filetype=ledger`

## In progress

- `apps/wiki-gui`: porting the frozen v5 shell to AntiWebUI (tabs, inspector
  modes, bottom shelf, syntax highlighting, Neovide manager, Ctrl shortcuts).

## Platform gaps / honest limits

- **App API writes are not wired yet.** The dev `--node` adapter reads a
  snapshot; mutations return 501. The main blocker is OAuth refresh: access
  tokens live ~10 minutes, and `harmion-client` has no refresh helper. The
  integrated connect flow also needs a redirect/listener and credential store.
- **No app-visible full-text Search**; local substring search is used.
- **No app-level kind/delete conflict resolution**; those are owner/admin only.
- **Interpreter v1 caps** (64 contents / 2 MiB / 128 KiB input) do not satisfy
  the Wiki all-content 128 MiB streaming requirement; the platform contract
  change is still pending.
- **No atomic host+snippet save**; fence conversion is two saves and reports
  partial failure.
- Git commit signing may be skipped when the GPG passphrase is unavailable.

## Next

1. Finish the AntiWebUI v5 port and Neovide manager.
2. Wire App API writes with an OAuth token store/refresh path.
3. Add the Wiki provider loop (`wiki/backlinks`, `wiki/outline`,
   `wiki/transclude`) and streaming contract.
4. Add real-node integration tests (connect, query, save, conflict, 410/409).
5. Add document examples (code, reading note, diary) on the frozen shell.
