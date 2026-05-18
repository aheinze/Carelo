# Carelo

Carelo is a local-first dual-pane file manager built with Tauri 2, Rust, Vue 3,
Pinia, Vite, and plain JavaScript.

It targets power users who want a keyboard-friendly desktop file manager with
real local file operations, remote-volume groundwork, a preview inspector, and
an embedded terminal.

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
  FTP, WebDAV, S3-compatible storage, Backblaze B2, Google Drive, OneDrive,
  Dropbox, and Swift/Rackspace-style object storage.
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

The release script runs:

```sh
tauri build --ci
```

Current bundle targets are configured for Linux `deb` and `rpm` packages.

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
