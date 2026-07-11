<img width="1672" height="941" alt="Carelo Banner" src="https://github.com/user-attachments/assets/a5033332-ddbe-4ea9-b29a-d69c52a97d45" />


# Carelo

Carelo is fast, local-first file management with dual panes, previews, remotes,
and custom tools.

## Features

- Dual-pane file browsing with tabs per pane.
- List, grid, and column (Miller) view modes.
- Real local file metadata in the preview panel, including permissions, owner,
  group, timestamps, hidden/read-only state, and size.
- Preview panel for images, video, audio, PDFs, and text/code, where the
  platform/webview can render the file.
- Quick Look: press `Space` for a large preview overlay with image/video/audio/
  PDF/text rendering, syntax highlighting for code, and arrow-key browsing;
  works for both local and remote files.
- Copy, cut, paste, rename, delete, create file, create folder, open, reveal, and
  open in the other pane.
- Type-ahead find: start typing in a pane to jump to the first item whose name
  matches in the current folder; repeat the key to cycle matches.
- Unified recursive search for names, paths, and file contents, with location,
  depth, type, extension, modified-date, size, hidden/ignored-file, symlink,
  case-sensitive, regex, and content-size facets. Name and content shortcuts open
  the same search surface with the appropriate preset.
- Permissions editor: a dedicated dialog to change Unix permissions — owner,
  group, and other read/write/execute plus the setuid, setgid, and sticky bits —
  with octal entry, an `ls -l` style preview, and optional recursive apply for
  folders. Works on local files (with sudo elevation when required) and on remote
  volumes (mount-backed and SFTP).
- Undo/redo (`Cmd/Ctrl + Z` / `Cmd/Ctrl + Shift + Z`) for move, copy, rename, and
  Trash deletes.
- Color tags (Finder-style) assignable from the context menu and shown as dots
  in every view, as tinted pane tabs, and in the breadcrumbs.
- Folder compare and sync between the two panes, with per-item diff, additive
  copy in either direction, and optional mirror-delete (to Trash).
- Checksum tools: compute a file's SHA-256 or compare two files for an exact
  match.
- Editable address bar (`Cmd/Ctrl + L`) plus clickable breadcrumbs.
- Mouse and keyboard selection, including multi-select and range selection.
- Drag-and-drop moves between panes, column targets, and folders.
- Sidebar with folder favorites (stored in SQLite, reorderable by drag and drop),
  custom favorite groups, and a Recent list of the last visited locations.
- Parallel, storage-aware file operations: copy and move run several files at
  once, with concurrency chosen by storage type — parallel for SSDs and remotes,
  sequential for spinning disks. Fast local copies use copy-on-write reflinks and
  `copy_file_range` where available, preserve metadata, and continue past
  per-file errors. Toggle parallel transfers in Settings.
- Conflict resolution on copy/move (skip, keep both, replace, replace-if-newer,
  checksum compare) with apply-to-all.
- Zip/7z/tar archive and unarchive actions with progress, cancellation, and
  automatic panel refresh.
- Image conversion and PDF tools (compress, merge, extract, split, rotate).
- Current work indicator with running tasks, progress, log view, and cancel
  actions.
- Embedded xterm terminal panel on Unix platforms; tabs are labeled by the
  shell's working directory and follow `cd` on Linux.
- Remote volume dialog backed by OpenDAL for supported providers such as SFTP,
  FTP, WebDAV, S3-compatible storage.
- Sudo password retry flow for local file operations that require elevated
  permissions.
- Multiple color themes with light, dark, and system appearance modes.
- Window size persistence.

## Requirements

- Node.js 20 or newer
- npm
- Rust 1.85 or newer
- Tauri 2 system dependencies for your OS

For Linux, install the packages required by Tauri/WebKitGTK for your
distribution before running the desktop app.

## Development

Install dependencies:

```sh
npm install
```

Run the Tauri desktop app:

```sh
npm run tauri dev
```

Run only the Vite dev server:

```sh
npm run dev
```

The Tauri dev configuration expects Vite on:

```text
http://127.0.0.1:1422
```

If Node is installed through nvm and is not on the default shell path, prefix
commands with your Node bin directory:

```sh
PATH=/home/artur/.nvm/versions/node/v22.19.0/bin:$PATH npm install
PATH=/home/artur/.nvm/versions/node/v22.19.0/bin:$PATH npm run tauri dev
```

## Build And Release

Build the frontend:

```sh
npm run build
```

Check the Rust side:

```sh
cargo check --manifest-path src-tauri/Cargo.toml
```

Create release bundles:

```sh
npm run release
```

Prepare a new release version and tag:

```sh
npm run release:prepare -- 0.4.1
```

This updates the app version files, adds a Linux metadata release entry, creates
a `chore: release v0.4.1` commit, and creates the annotated `v0.4.1` tag.
Use `--dry-run` to preview the changes first. The release-prep script expects a
clean git worktree.

The release script runs:

```sh
tauri build --ci
```

Current bundle targets are configured for Linux `deb`, `rpm`, and `AppImage`
packages. The updater uses the `AppImage` artifact for in-app updates.

### GitHub Releases Updates

Carelo checks for updater metadata at:

```text
https://github.com/aheinze/Carelo/releases/latest/download/latest.json
```

The updater public key is committed in `src-tauri/tauri.conf.json`. The private
signing key was generated at:

```text
/home/artur/.tauri/carelo.key
```

For local signed release builds:

```sh
TAURI_SIGNING_PRIVATE_KEY_PATH=/home/artur/.tauri/carelo.key npm run release
```

For GitHub Actions releases, add these repository secrets:

```text
TAURI_SIGNING_PRIVATE_KEY
TAURI_SIGNING_PRIVATE_KEY_PASSWORD (optional)
```

`TAURI_SIGNING_PRIVATE_KEY_PASSWORD` can be omitted while the generated key has
no password. Pushing a tag like `v0.4.1` runs `.github/workflows/release.yml`,
creates a draft GitHub Release, uploads the Linux bundles, and publishes
`latest.json` for the in-app updater.

## Data Storage

Carelo stores durable app data in:

```text
~/.local/share/carelo/carelo.store
```

The SQLite database file and required tables are created automatically when the
app starts. Favorites, color tags, remote volume configurations, and app
settings are stored there.

UI settings such as pane layout, theme-dependent dimensions, sidebar state, and
window dimensions are stored in browser/Tauri local storage.

## Architecture

The frontend is a Vue application organized around panes, tabs, sidebar
locations, keyboard shortcuts, dialogs, preview state, file-operation queues,
terminal state, and app settings.

The frontend never reads the local filesystem directly. It calls Rust through
Tauri commands exposed from `src/composables/useFileOperations.js` and related
composables.

The Rust side is split into:

- `src-tauri/src/commands/` for Tauri command handlers.
- `src-tauri/src/fs/` for local, sudo, archive, and remote filesystem logic.
- `src-tauri/src/store/` for the SQLite app store.
- `src-tauri/src/queue/` for long-running operation progress models.
- `src-tauri/src/settings/` for app path/settings helpers.

Local file operations use a provider trait so the UI is not coupled to one
backend. Remote volumes use OpenDAL where providers are available. Archive
operations use native Rust libraries, including the `zip` crate.

## Project Structure

```text
.
|-- package.json
|-- vite.config.js
|-- src/
|   |-- App.vue
|   |-- main.js
|   |-- assets/
|   |-- components/
|   |-- composables/
|   |-- directives/
|   |-- stores/
|   `-- utils/
`-- src-tauri/
    |-- Cargo.toml
    |-- tauri.conf.json
    |-- capabilities/
    |-- icons/
    `-- src/
        |-- commands/
        |-- fs/
        |-- queue/
        |-- settings/
        |-- store/
        |-- lib.rs
        `-- main.rs
```

## Useful Shortcuts

- `Tab`: switch active pane
- `Cmd/Ctrl + Shift + P`: command palette
- `Cmd/Ctrl + P`: unified search with the name/path preset
- `Cmd/Ctrl + Shift + F`: unified search with the content preset
- `Cmd/Ctrl + L`: edit the address bar / go to a path
- `Space`: Quick Look (preview overlay); `Insert`: toggle item selection
- `Backspace` or `Cmd/Ctrl + Up`: go to parent folder
- `Alt + Left` / `Alt + Right`: navigation history
- `F2`: rename
- `F3`: preview
- `F4`: open
- `F5`: copy to other pane
- `F6`: move to other pane
- `F7`: create folder
- `F8` or `Delete`: delete
- `Cmd/Ctrl + Z` / `Cmd/Ctrl + Shift + Z`: undo / redo
- `Cmd/Ctrl + A`: select all
- `Cmd/Ctrl + F1`: grid view
- `Cmd/Ctrl + F2`: list view
- `Cmd/Ctrl + .`: toggle hidden files
- Start typing in a pane to jump to a file by name (type-ahead find)

Use `F1` in the app to open the shortcut reference.

## Verification

Before shipping a change, run:

```sh
npm run build
cargo check --manifest-path src-tauri/Cargo.toml
```

Expected behavior:

- `npm run tauri dev` opens the Carelo desktop window.
- Both panes list real local directory contents.
- List, grid, and column views switch correctly.
- Drag/drop moves update source and target panes, including column targets.
- Favorites persist after restart.
- Archive/unarchive work appears in the current work indicator.
- The red close button exits the app and preserves the last window dimensions.
