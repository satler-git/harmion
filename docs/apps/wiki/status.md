# Wiki implementation status

Last updated: 2026-11-24

## Design

The v5 mockup ([HTML](../../assets/source/wiki-ui-design.html) / [PNG](../../assets/wiki/wiki-ui-design.png)) is superseded by
the refreshed shell and reading surface in [Wiki GUI refresh](design.md). The
refresh keeps the feature set but changes the visual system and layout: serif display
type, a content-first reading block (article + 目次 rail), a separate on-demand
ページ情報 inspector, and an explicit 2-hop shelf. Screenshots: [home](../../assets/wiki/wiki-gui-home.png),
[search](../../assets/wiki/wiki-gui-search.png), [shelf](../../assets/wiki/wiki-gui-shelf.png),
[inspector](../../assets/wiki/wiki-gui-info.png), [outline](../../assets/wiki/wiki-gui-outline.png),
[shortcuts](../../assets/wiki/wiki-gui-shortcuts.png), [Markdown](../../assets/wiki/wiki-gui-markdown.png),
[empty](../../assets/wiki/wiki-gui-empty.png), [1440 with properties](../../assets/wiki/wiki-gui-1440-info.png).

- Reading block: library 264 + `article (max 800) + 目次 (176)` centred as a unit;
  the rail hides when the remaining block would be narrower than 900 px, then `Ctrl+3`
  opens a standalone 目次 panel
- Inspector is a 360 px on-demand column (Ctrl+I), hidden by default; it has only
  `リンク` / `ページ情報`, so 目次 and properties never share a segmented control
- 2-hop shelf is click/`Ctrl+Alt+L`, not hover; only its 36 px handle is visible
- Tabs, backlink footer, rich inline Markdown, copyable code blocks
- Ctrl shortcuts: `Ctrl+K` search, `Ctrl+E` Neovide, `Ctrl+I` info,
  `Ctrl+N` new, `Ctrl+P` quick open, `Ctrl+W` close tab, `Ctrl+[`/`Ctrl+]`
  history, `Ctrl+1`/`Ctrl+2`/`Ctrl+3` inspector mode, `Ctrl+\` sidebar,
  `Ctrl+Alt+L` 2-hop shelf, `?` shortcut sheet
- Code snippets must be syntax highlighted (FR-55)

## Framework decision

Stay on the pinned AntiWebUI revision. The audit found no framework capability that
blocks the Wiki UI; the visible gaps were app code (hand-written Markdown, no link
routing, always-open inspector) or unused APIs (`markdown`, SVG images, popups). See [the refresh document](design.md#framework-decision) for the
comparison and the maintenance caveat (git-only pin, single maintainer).

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
- 64 + 3 tests pass (unit + socket integration); client 7

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

- Refreshed shell is implemented in AntiWebUI: pinned library header, a centred reading
  block (`article ≤800 + 目次 176`), a separate on-demand ページ情報 inspector, explicit
  2-hop shelf, palette, shortcut cheat sheet and the Neovide manager.
- 目次 and ページ情報 are independent surfaces. Ctrl+3 collapses/restores the rail when it
  fits, or opens a floating 目次 panel when it does not; Esc closes the topmost layer.
- A wiki with no pages gets a real welcome state with retry/create guidance; fixtures are
  snapshot-only, and the status bar no longer labels an empty wiki as a built-in sample.
- Tab clicks and links to already-open pages switch the article; each tab keeps its own
  scroll offset and active heading (persisted across launches), `Ctrl+Tab` cycles tabs with
  a focus ring, and Space/PageUp/PageDown scroll the article without a pointer.
- Search highlights matched text in titles and excerpts. The editor watcher polls the
  active page revision every 2s while Neovide is attached and refreshes the article in
  place without losing the scroll position. Outline clicks land the
  heading below the top edge and scrolling updates the active outline entry. Outline and
  fragment navigation also send `WikiGoTo` to Neovim, so the editor buffer and cursor
  follow the GUI (one-way; see the gaps).
- Reading surface: IBM Plex Sans JP + Noto Serif CJK JP, 800 px article,
  inline bold/italic/strike/code/links, clickable `[[wiki links]]`, backlinks
  footer, code copy button, quieter tables/quotes.
- Material Symbols Outlined icons (one subset font per used glyph), tooltips
  with shortcuts, and right-click menus on tabs, page/snippet rows, links,
  2-hop rows and the article. Copy actions show a toast.
- Prose, lists, tables and quotes go through `markdown_view`; headings, code, embeds,
  transclusions and interpreters stay app-rendered so outline anchors and the code copy
  button survive. Fenced code is coloured by `anti-webui-code` (tree-sitter, 14
  languages); ledger/hledger keeps the app-specific highlighter.
- `?` opens a shortcut cheat sheet; `Ctrl+\` toggles the library and `Ctrl+3` toggles the
  目次 rail/panel. Tooltips carry the chords of icon-only controls.
- The property panel shows aliases, a clickable backlink count opens links, and the
  article header/footer expose backlinks instead of hiding them.
- Offscreen snapshots are documented with `--snapshot`, `--search`, `--shelf`,
  `--info`, `--outline`, `--shortcuts`, `--empty`, `--tab`, `--page`, `--width` and
  `--height`; the 1600x1000 and 1280x800 layouts were both rendered.
- New page is wired through `create_page` and `Ctrl+N`; suggestions choose
  distinct via groups.
- `cargo test -p harmion-wiki-gui` (8 passed, 1 ignored) and
  `cargo clippy -p harmion-wiki-gui --all-targets --no-deps -- -D warnings`
  pass.

## Implemented against a real node

- The `--node` adapter performs real create/edit/convert/delete with a body-diff
  guard; integrated connect, refresh-token rotation and the 0600 credential
  store are implemented.
- SQLite FTS5/trigram search with NFKC normalization and tag queries; the export
  tree can be re-imported without duplicate Content.
- GUI keeps a 0600 offline cache, long-polls the `watch` endpoint, reconnects,
  and supports revision-checked delete and a Neovim conflict diff.
- Interpreter streaming v1 is implemented: >64 items, chunked input, 128 MiB
  explicit limit, cancellation and tests.

## Platform gaps / honest limits

- Reads are still a startup snapshot. The GUI long-polls `watch` (20s), applies
  updates on the UI thread, reconnects, and keeps an offline cache of
  summaries/workspaces. A WebSocket push path is not used.
- **Bulk `import` is not wired to the node backend**: `import` is outside the
  node write allowlist (`apps/wiki-server/src/store.rs`), so importing an
  existing Scrapbox/Obsidian export into a node-backed wiki returns 501. Use the
  JSON backend, or extend the allowlist.
- **No app-level kind/delete conflict resolution**; those are owner/admin only.
- No atomic host+snippet save; fence conversion is two saves and reports partial
  failure.
- Interpreter output streaming remains out of scope.
- Git commit signing may be skipped when the GPG passphrase is unavailable.

## Remaining app gaps

- Updates use a 20s long-poll `watch`; there is no WebSocket push or resume
  backfill across a node restart.
- Conflict merge is a 2-way diff; no common-ancestor 3-way merge.

- Cache/subscription follow-up, offline snapshot and permission-loss handling (FR-19, FR-23).
- Conflict candidates/manual merge UI (FR-25) and deletion confirmation (FR-26).
- Import, block references, snapshots and layout presets (FR-29, FR-31, FR-35, FR-52).
- 10k-page FTS5 performance and Japanese 2-character search (FR-21).
- Provider uniqueness/replacement and registration approval (FR-43, FR-44).
- GUI: keyboard focus rings on rows/tabs, local image/SVG embeds, AccessKit/AT-SPI.
- Markdown segmentation is line-based: setext headings and fences nested in lists render
  but do not reach the outline. A block scanner or the upstream heading-anchor API is the
  next correctness step.
- The watch loop flips the source to offline when the server connection drops, instead of
  claiming a live connection while calls fail.
- Neovim position sync is one-way: the GUI calls `:WikiGoTo <id> <line>` when the outline or
  a fragment link is used. Neovim -> GUI scroll reporting still needs an `editor_state` RPC
  and must yield to edits and conflicts.
- Visual pass still owed for tab shape/overflow and the three-column 2-hop shelf.

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
Content data. [Home](../../assets/wiki/wiki-gui-home.png), [search](../../assets/wiki/wiki-gui-search.png),
[shelf](../../assets/wiki/wiki-gui-shelf.png), [inspector](../../assets/wiki/wiki-gui-info.png) and
[Markdown](../../assets/wiki/wiki-gui-markdown.png) snapshots show the refreshed shell.

`target/debug/wiki-gui --page "Markdown 見本" --snapshot out.png` opens a page
directly; the GUI polls every 2s, reconnects, and falls back to the 0600
offline cache with an `オフラインキャッシュ` label. Add `--info` or `--shelf`
to capture the drawer and shelf states, and `--width/--height` for a
non-default viewport.

## UI/editor pass

The GUI is split into `ui`, `toolbar`, `shelf`, and `palette` modules plus the
existing `neovide`/`highlight` modules. Tabs only switch views; opening Neovide
is explicit (`Ctrl+E` or the Neovide control), so browsing no longer focuses or
launches the editor. Sidebar All/Recent modes, sidebar collapse, centered
search with arrow-key selection, 2-hop keyboard selection, aligned 30 px
toolbar controls, larger hit targets, pointer cursors, and an explicit shelf
close control were added.

Neovim receives absolute `WIKI_SERVER_CMD`, `WIKI_LSP_CMD`, `WIKI_SOCKET`, and
the dedicated server socket. The plugin honors those env values, sets
`titlestring = "Harmion Wiki — <title>"`, and the GUI focuses that titled
Neovide window only after an explicit edit request.

## Field conflict candidates

`save` and `update_meta` conflicts now record transient field candidates:
body/title/tags/aliases with `current`, `provided`, and both revisions. The
`conflicts` RPC returns them; Neovim's `:WikiConflict` shows the body diff and
names the conflicting fields, and `:WikiResolve` saves the manually merged
buffer with the current revision. This is still a 2-way merge (no common
ancestor) and the candidates are intentionally not persisted across restart.
