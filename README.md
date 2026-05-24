<img width="1672" height="941" alt="Image" src="https://github.com/user-attachments/assets/4dea1f56-3a30-4ade-8c49-429600ac5caf" />


# Carelo

Carelo is fast, local-first file management with dual panes, previews, remotes,
and custom tools.

## Features

- Dual-pane file browsing with tabs per pane.
- List, grid, and column view modes.
- Real local file metadata in the preview panel, including permissions, owner,
  group, timestamps, hidden/read-only state, and size.
- Image, video, and audio preview support where the platform/webview can render
  the file.
- Mouse and keyboard selection, including multi-select and range selection.
- Drag-and-drop moves between panes, column targets, and folders.
- Folder favorites in the sidebar, stored in SQLite and reorderable by drag and
  drop.
- Copy, cut, paste, rename, delete, create folder, open, reveal, and open in the
  other pane.
- Zip archive and unarchive actions with progress, cancellation, and automatic
  panel refresh.
- Current work indicator with running tasks, progress, log view, and cancel
  actions.
- Embedded xterm terminal panel on Unix platforms.
- Remote volume dialog backed by OpenDAL for supported providers such as SFTP,
  FTP, WebDAV, S3-compatible storage.
- Sudo password retry flow for local file operations that require elevated
  permissions.
- Light and dark themes based on the system color scheme.
- Window size persistence.

## Requirements

- Node.js 20 or newer
- npm
- Rust 1.77 or newer
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
app starts. Favorites are stored there.

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
- `Cmd/Ctrl + P`: fuzzy file search in the current folder
- `Cmd/Ctrl + Shift + F`: content search in the current folder
- `Backspace` or `Cmd/Ctrl + Up`: go to parent folder
- `Alt + Left` / `Alt + Right`: navigation history
- `F2`: refresh
- `F3`: preview
- `F4`: open
- `F5`: copy to other pane
- `F6`: move to other pane
- `F7`: create folder
- `F8` or `Delete`: delete
- `Cmd/Ctrl + A`: select all
- `Cmd/Ctrl + F1`: grid view
- `Cmd/Ctrl + F2`: list view
- `Cmd/Ctrl + .`: toggle hidden files

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
