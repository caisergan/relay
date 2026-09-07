# Phase 4 — The Relay Experience (weeks 10–12)

Objective: the features that make Relay feel like Relay rather than "FileZilla with new
paint": editor, Quick Look, command palette, activity feed, settings, import — every
overlay and flow the design specifies, wired to the real engine.

## 4.1 Built-in remote editor (editor tab kind)

- Editor component: **CodeMirror 6** (MIT, tree-shakeable, ~300 KB with a language
  pack set) — replaces the design's regex-highlight textarea with the same visual
  treatment (tokens themed via CSS variables; line numbers; JetBrains Mono).
  Languages: js/ts, html/css, json/yaml/toml, php, py, sh, sql, md (lazy-loaded).
- Open flow: file row → "Edit" pill (text extensions per an editable allowlist) →
  `read_file` (limit 2 MB; larger → toast "use external editor") → new editor tab
  (design: tabs are session-scoped kinds: session | editor | connect).
- Save flow (⌘S / "Save to server"): button states idle → Uploading… → Saved · time
  (design). Steps: write local backup
  `app_data/backups/{server}/{path-hash}/{timestamp}-{name}` (design promises
  "previous version backed up locally"; keep last 5), then upload via the queue with
  priority=highest and conflict_policy=Overwrite; editor listens for its JobUpdate.
- Dirty tracking: tab dot, close-with-unsaved confirm sheet. Remote changed detection:
  stat mtime before save; if remote is newer than at open → conflict sheet variant
  ("Server copy changed — Overwrite / Keep both / Cancel").

## 4.2 External editor ("Open in VS Code" + watch → upload)

Reimplements FileZilla's edithandler with the `notify` crate:
- `EditSession { job: download to temp dir (app_data/edit/{uuid}/name), spawn editor,
  watch }`. Editor resolution: settings "external editor" (default: VS Code detection —
  `code` on PATH, `/Applications/Visual Studio Code.app`, `%LOCALAPPDATA%\Programs\...`;
  fallback OS default opener via `tauri-plugin-opener`).
- `notify` watcher (debounced 500 ms — editors write multiple events; use
  notify-debouncer-mini) on the temp file → per settings either auto-upload or toast
  "file changed — Upload?" with action button. Upload = highest-priority queue job.
- "Files being edited" list in the Activity slide-over (state: editing/uploading/synced);
  cleanup of temp dir on session close with unsaved-changes warning. Executable
  extensions (.sh, .exe, …) get a confirm sheet before opening (FileZilla's dangerous-
  filetype lesson).

## 4.3 Quick Look (Space)

- `Prompt-free path`: Space on selection → sheet with designed layout (path/size/perms/
  mtime bar + Open in editor / Download buttons).
- Text ≤ 512 KB: `read_file` → CodeMirror read-only with highlight.
- Images (png/jpg/gif/webp/svg ≤ 5 MB): `read_file` → blob URL render (design has the
  placeholder frame). SVG rendered sandboxed (`<img>`, never inline DOM injection).
- Dir: entry count + size-if-known (design's dir info card). Other: designed
  "no preview" fallback. Local files: same sheet, read directly.
- Cache last 10 previews per session (LRU) so Space-flipping through files feels native.

## 4.4 Command palette (⌘K)

- Registry-driven: `Action { id, title, keywords, scope, run() }` — sources:
  static actions (theme, settings, go to parent, upload/download selection ⌘U/⌘D,
  quick look, activity, disconnect), dynamic providers (servers → "Connect to X",
  active-server bookmarks → "Jump to Y", open tabs → "Switch to Z").
- Fuzzy match: `fzf`-style scorer (small local implementation or `fzy.js`-class lib,
  ~1 KB — no heavy dep), ↑↓/↵/esc per design, recents boost.
- Same registry powers the menu bar (macOS native menu via Tauri) and keyboard map —
  single source of truth for shortcuts shown in palette rows.

## 4.5 Activity slide-over & logging

- Human feed: `Activity` events (uploaded/downloaded/error/connected — design's icons +
  relative timestamps) with per-session filter; raw-log toggle swaps to the P3 ring
  buffer view (Status/Command/Response/Error coloring per design).
- "Copy log" button; log-to-file setting (rotating, 5 MB × 3, in app_log_dir via
  `tracing-appender`) — support-request friendly.

## 4.6 Settings, servers, bookmarks, import

- Settings sheet (design): theme (light/dark/system), row density, concurrent
  transfers slider 1–8 (live — scheduler listens), default conflict action
  (Ask/Overwrite/Skip). Extended page (non-design, plain list): default download dir,
  external editor path, auto-upload-on-change toggle, keepalive interval, log level,
  editor allowlist. Persist: `settings.json` via a small typed store (serde struct,
  atomic write; not tauri-plugin-store — we want types).
- Sidebar CRUD complete: groups (create/rename/collapse state persisted), server
  reorder (drag), color tags, duplicate server, per-server bookmarks add/rename/delete
  (+ "bookmark current location" palette action).
- **FileZilla import**: parse `sitemanager.xml` (quick-xml) — map Host/Port/Protocol
  (0=FTP,1=SFTP,3/4=FTPS)/User/LogonType/EncodedPass (base64 in FZ3)… passwords import
  ONLY with explicit user consent checkbox → keychain. Also import folder structure →
  groups. This is data-format compatibility, not code reuse — clean re: GPL.
  Export: our own `relay-servers.json` (documented schema) + optional FZ-format export
  deferred to v1.x.
- Recents on ConnectView: last 8 ad-hoc connections (host+proto only, no secrets),
  "promote to saved server" affordance (FileZilla's copy-to-sitemanager lesson).

## 4.7 Polish pass required by the design

- All designed empty/edge states wired: empty folder, permission denied, connection
  lost pane variant, loading skeletons, sidebar-collapsed floating button.
- Toast system finalized (ok/danger/transit + action button, stacking, auto-dismiss
  with hover-pause).
- Keyboard map complete: ⌘T/⌘W/⌘K/⌘S/⌘U/⌘D/Space/Esc/F2/Del/⌘R(refresh)/⌘L(focus path),
  Windows equivalents (Ctrl), all listed in palette.
- `prefers-reduced-motion`: gutter pills become static progress chips; sheets fade.
- A11y baseline: focus traps in sheets, aria-labels on icon buttons, list keyboard nav,
  visible focus rings (design has focus states), contrast check both themes.

## 4.8 Exit criteria

1. Edit a remote config file in-app, ⌘S, see backup on disk and new content on server;
   external-edit the same file in VS Code, save, get the upload toast, confirm upload.
2. Space through a directory of mixed text/images without opening dialogs; palette
   reaches every major action; "Connect to <server>" works from anywhere.
3. Import a real FileZilla `sitemanager.xml` (build a fixture with 20 sites/folders/
   all logon types) → sidebar matches, passwords in keychain after consent.
4. Design QA session: side-by-side with Relay.dc.html — every screen/state present,
   token-accurate in both themes and densities.
5. No unhandled-promise/panic paths: error taxonomy renders a designed state or toast
   for every `EngineError` variant (checklist test).
