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
- 19 unit tests pass

### `apps/wiki-server`

- Local-first Unix-socket JSON-RPC; existing `list/open/workspace/search/save`
- Atomic JSON persistence (`serve --data PATH`, 0600), in-memory fixtures by default
- `status`, `create_page`, `create_snippet`, `convert_fence`, `delete`,
  `transclude`, `export`, `conflicts`
- `workspace` returns page, before/after, snippet, outline, links,
  related snippets, recent, page info
- `save --auto-convert` creates snippets for loose `!` fences and canonicalizes
  the host; the Neovim plugin rewrites the buffer after conversion
- `provider_list`/`provider_call` implement local `wiki/backlinks`,
  `wiki/outline` and `wiki/transclude`; platform registration is separate
- Dev-only `--node DIR` snapshot using `harmion-client::Client::from_directory`
  (admin secret). `save` is body-only and refuses if the node body changed since
  the snapshot; create/convert/delete still return 501.
- 50 + 2 tests pass (unit + integration); client 7

### `apps/wiki-lsp`

- stdio LSP; `wiki://<id>` virtual documents; Content-Length framing
- diagnostics, definition, references, completion, hover
- Server unavailable still yields local diagnostics
- Unit tests pass; live probe against `wiki-server` confirmed initialize,
  diagnostics, definition to `wiki://p-goals`, references, completion
- No write path and no credentials/tokens

### `apps/wiki/nvim`

- `:WikiOpen`, `:WikiNew`, `:WikiSave`, `:WikiList`, `:WikiSearch`,
  `:WikiWorkspace`, `:WikiTitle`, `:WikiTags`, `:WikiDelete`, `:WikiLspStart`
- `wiki://<id>` buffers, `BufWriteCmd`, 1.5s debounced autosave,
  swap-file recovery, LSP auto-start
- Headless tests cover open/save, auto-convert, new page, autosave, metadata
  and delete

## GUI

- Frozen v5 shell is implemented in AntiWebUI: 4 columns, tabs, inspector
  `リンク`/`情報`, centered outline, bottom 2-hop shelf, Ctrl shortcuts,
  syntax highlighting and the Neovide manager.
- `target/wiki-final.png` is a 1600x1000 snapshot; the actual window was
  launched on niri and the server-backed list/workspace path was verified.
- New page is wired through `create_page` and `Ctrl+N`; suggestions choose
  distinct via groups.

## Platform gaps / honest limits

- **App API writes are partial.** The dev `--node` adapter can save the body
  after a fresh GET and refuses if the body changed since the snapshot, but
  create/convert/delete return 501. Production OAuth is blocked on refresh:
  access tokens live ~10 minutes, and `harmion-client` has no refresh helper;
  the integrated connect flow also needs a redirect/listener and credential store.
- SQLite FTS5/trigram search with NFKC normalization and tag queries; the
  export tree can be re-imported without duplicate Content.
- GUI keeps a 0600 offline cache, polls every 2s, reconnects, and supports
  revision-checked delete and a Neovim conflict diff.
- Reads are still a startup snapshot; the GUI polls every 2s, reconnects
automatically, and keeps an offline cache of summaries/workspaces. The
WebSocket subscription path is still pending.
- **No app-level kind/delete conflict resolution**; those are owner/admin only.
- **Interpreter streaming v1 is implemented**: >64 items, chunked input,
  128 MiB explicit limit, cancellation and tests. Output streaming remains out
  of scope.
- **No atomic host+snippet save**; fence conversion is two saves and reports
  partial failure.
- Git commit signing may be skipped when the GPG passphrase is unavailable.

## Remaining app gaps

- WebSocket subscription is still replaced by 2s polling; no server push.
- Conflict merge is a 2-way diff; no common-ancestor 3-way merge.
- Node backend `update_meta` uses a body-guarded Edit with wiki attributes.

- Cache/subscription follow-up, offline snapshot and permission-loss handling (FR-19, FR-23).
- Tag list/filter pages and workspace filters (FR-22, FR-34).
- GUI rendering of transclusions and interpreter results (FR-12, FR-18).
- Conflict candidates/manual merge UI (FR-25) and deletion confirmation (FR-26).
- Import, block references, snapshots and layout presets (FR-29, FR-31, FR-35, FR-52).
- 10k-page FTS5 performance and Japanese 2-character search (FR-21).
- Provider uniqueness/replacement and registration approval (FR-43, FR-44).

## Next

1. Wire the full OAuth connect/refresh path and real-node save/conflict tests.
2. Add subscription catch-up and the offline cache.
3. Add the streaming interpreter contract and provider registration.
4. Fill the app gaps above, starting with transclusion rendering and conflicts.

## Conflict and delete flow

`save`, `update_meta` and `delete` are revision-checked. A stale save keeps the
buffer dirty and records the current server body; `:WikiConflict` opens a
side-by-side diff for manual merge. The GUI delete action requires a second
click and sends the displayed revision. This is a 2-way manual flow, not a
3-way merge against a common ancestor.

## Functional UI evidence

The GUI is no longer a static mock: the sidebar lists every page, tag chips
filter pages/snippets, search uses the server, tabs load workspaces, outline
entries scroll the article, links/2-hop rows open Content, new/rename/tags/
delete/conflict routes call the server, and Markdown headings/lists/tables/
quotes/code plus embeds/transclusions/interpreter panels are rendered from
Content data. [Demo snapshot](wiki-demo.png) and [home snapshot](wiki-final.png)
are generated from the live server.

`target/debug/wiki-gui --page "Markdown 見本" --snapshot out.png` opens a page
directly; the GUI polls every 2s, reconnects, and falls back to the 0600
offline cache with an `オフラインキャッシュ` label.
