import { onMounted, onUnmounted } from 'vue';
import {
  createFolder,
  deleteItems,
  editFile,
  getFileMetadata,
  isRemotePath,
  openWithDefaultApp,
  readSystemFileClipboard,
  writeSystemFileClipboard,
} from './useFileOperations';
import { useDialog } from './useDialog';
import {
  joinPath,
  useFileTransferGuards,
} from './useFileTransferGuards';
import { useShortcutsModal } from './useShortcutsModal';
import { useFileManagerStore } from '../stores/fileManagerStore';
import { archiveRootPath, isArchiveEntry, isArchivePath } from '../utils/archivePaths';
import {
  CREATE_SIDEBAR_GROUP_EVENT,
  OPEN_REMOTE_STORAGE_EVENT,
  RUN_COMMAND_EVENT,
} from '../utils/appEvents';

let fileClipboard = null;
const FILE_CLIPBOARD_STORAGE_KEY = 'carelo.fileClipboard';
const FILE_CLIPBOARD_MODES = new Set(['copy', 'move']);

function isCommand(event) {
  return event.metaKey || event.ctrlKey;
}

function isEditableTarget(target) {
  return Boolean(
    target?.closest?.('input, textarea, select, [contenteditable="true"], .terminal-panel'),
  );
}

function otherPaneId(paneId) {
  return paneId === 'left' ? 'right' : 'left';
}

function nameFromPath(path) {
  const value = String(path || '').replace(/\/+$/, '');

  if (!value || value === '/' || value === '~') {
    return value || '';
  }

  if (value.startsWith('remote://')) {
    const parts = value.slice('remote://'.length).split('/').filter(Boolean);
    return parts.at(-1) || parts[0] || value;
  }

  return value.split('/').filter(Boolean).at(-1) || value;
}

function normalizeClipboardPath(path) {
  return String(path || '').trim().replace(/\/+$/, '') || '/';
}

function clipboardUriForPath(path) {
  const value = String(path || '');

  if (value.startsWith('remote://')) {
    return value;
  }

  return `file://${value.split('/').map((part) => encodeURIComponent(part)).join('/')}`;
}

function clipboardTextForEntries(mode, entries) {
  const action = mode === 'move' ? 'cut' : 'copy';
  const fileUris = entries.map((entry) => clipboardUriForPath(entry.path));

  return ['x-special/gnome-copied-files', action, ...fileUris].join('\n');
}

function clipboardGnomePayloadForEntries(mode, entries) {
  const action = mode === 'move' ? 'cut' : 'copy';
  const fileUris = entries.map((entry) => clipboardUriForPath(entry.path));

  return [action, ...fileUris].join('\n');
}

function clipboardUriListForEntries(entries) {
  return entries.map((entry) => clipboardUriForPath(entry.path)).join('\n');
}

function isLocalSystemClipboardPath(path) {
  const value = String(path || '');

  return value.startsWith('/') && !value.startsWith('remote://') && !isArchivePath(value);
}

function canUseSystemFileClipboard(entries) {
  return entries.length > 0 && entries.every((entry) => isLocalSystemClipboardPath(entry.path));
}

function samePathList(paths, entries) {
  const left = paths.map(normalizeClipboardPath);
  const right = entries.map((entry) => normalizeClipboardPath(entry.path));

  return left.length === right.length && left.every((path, index) => path === right[index]);
}

function stripClipboardLine(line) {
  const trimmed = String(line || '').trim();

  if (
    (trimmed.startsWith('"') && trimmed.endsWith('"')) ||
    (trimmed.startsWith("'") && trimmed.endsWith("'"))
  ) {
    return trimmed.slice(1, -1);
  }

  return trimmed;
}

function pathFromClipboardLine(line) {
  const value = stripClipboardLine(line);

  if (!value) {
    return '';
  }

  if (value.startsWith('file://')) {
    try {
      return decodeURIComponent(new URL(value).pathname);
    } catch {
      return decodeURIComponent(value.replace(/^file:\/\//, ''));
    }
  }

  if (value.startsWith('/') || value.startsWith('~/') || value === '~' || value.startsWith('remote://')) {
    return value;
  }

  return '';
}

function parseClipboardText(text) {
  const lines = String(text || '')
    .split(/\r?\n/)
    .map((line) => line.trim())
    .filter(Boolean);

  if (lines.length === 0) {
    return null;
  }

  let mode = 'copy';
  let pathLines = lines;

  if (lines[0] === 'x-special/gnome-copied-files') {
    mode = lines[1] === 'cut' ? 'move' : 'copy';
    pathLines = lines.slice(2);
  } else if (lines[0] === 'copy' || lines[0] === 'cut') {
    mode = lines[0] === 'cut' ? 'move' : 'copy';
    pathLines = lines.slice(1);
  }

  const paths = [...new Set(pathLines.map(pathFromClipboardLine).filter(Boolean))];

  return paths.length > 0 ? { mode, paths } : null;
}

function storedClipboard() {
  try {
    const parsed = JSON.parse(window.localStorage?.getItem(FILE_CLIPBOARD_STORAGE_KEY) || 'null');

    if (!parsed || !FILE_CLIPBOARD_MODES.has(parsed.mode) || !Array.isArray(parsed.entries)) {
      return null;
    }

    return parsed;
  } catch {
    return null;
  }
}

function storeClipboard(payload) {
  try {
    window.localStorage?.setItem(FILE_CLIPBOARD_STORAGE_KEY, JSON.stringify(payload));
  } catch {
    // Clipboard still works in memory if persistence is unavailable.
  }
}

function clearStoredClipboard() {
  try {
    window.localStorage?.removeItem(FILE_CLIPBOARD_STORAGE_KEY);
  } catch {
    // Ignore storage cleanup failures.
  }
}

async function clipboardEntryFromPath(path) {
  const metadata = await getFileMetadata(path);

  return {
    name: nameFromPath(metadata.path || path),
    path: metadata.path || path,
    kind: metadata.kind || 'file',
    size: metadata.size,
    modifiedAt: metadata.modifiedAt,
    isHidden: metadata.isHidden,
    isSymlink: metadata.isSymlink,
    isReadonly: metadata.isReadonly,
  };
}

async function entriesFromClipboardPaths(paths) {
  const settled = await Promise.allSettled(paths.map((path) => clipboardEntryFromPath(path)));

  return settled
    .filter((result) => result.status === 'fulfilled')
    .map((result) => result.value);
}

function focusSearch() {
  const input = document.querySelector('[data-search-field]');

  if (input) {
    input.focus();
    input.select?.();
  }
}

function dispatchSelectedContextMenu(store) {
  const tab = store.activeTabFor(store.activePaneId);
  const index = tab?.selectedIndex ?? -1;
  const row = document.querySelector(`.pane--active [data-file-index="${index}"]`);
  const rect = row?.getBoundingClientRect();

  if (!row || !rect) {
    return;
  }

  row.dispatchEvent(new MouseEvent('contextmenu', {
    bubbles: true,
    cancelable: true,
    clientX: rect.left + 28,
    clientY: rect.top + Math.min(rect.height - 4, 22),
  }));
}

function shortcutHelpText() {
  return [
    'F3 Preview, F4 Edit, F5 Copy, F6 Move, F7 New Folder, F8/Delete Delete',
    'Ctrl+Shift+P Commands, Ctrl+P Fuzzy Search, Ctrl+Shift+F Content Search, Ctrl+C Copy, Ctrl+X Cut, Ctrl+V Paste',
    'Tab Switch Pane, Alt+Left/Right History, Ctrl+\\ Root, Ctrl+PageUp Parent',
    'Insert/Space Toggle Selection, Ctrl+A/Num+ Select All, Num- Clear, Num* Invert',
    'Ctrl+F1 Grid, Ctrl+F2 List, Ctrl+F3 Name, Ctrl+F4 Extension, Ctrl+F5 Date, Ctrl+F6 Size, Ctrl+F7 Unsorted',
  ].join('\n');
}

export function useKeyboardShortcuts() {
  const store = useFileManagerStore();
  const dialog = useDialog();
  const transfers = useFileTransferGuards();
  const shortcutsModal = useShortcutsModal();

  function activePane() {
    return store.activePaneId;
  }

  function activeTab() {
    return store.activeTabFor(activePane());
  }

  function currentPath(paneId = activePane()) {
    return store.effectiveDirectoryFor(paneId) || store.activeTabFor(paneId)?.currentPath || '~';
  }

  function operationEntries() {
    return store.operationEntriesFor(activePane());
  }

  async function reloadPane(paneId = activePane()) {
    await store.reloadDirectoryInPanes(currentPath(paneId), [paneId]);
  }

  function parentDirectoriesForEntries(entries) {
    return [...new Set(
      entries
        .map((entry) => store.parentDirectoryFor(entry.path))
        .filter(Boolean),
    )];
  }

  async function refreshDirectories(paths, paneIds = null) {
    const uniquePaths = [...new Set(paths.filter(Boolean))];

    await Promise.all(uniquePaths.map((path) => store.reloadDirectoryInPanes(path, paneIds)));
  }

  async function reloadTransferPanes(sourcePaneId, targetPaneId, paths = []) {
    const paneIds = [...new Set([sourcePaneId, targetPaneId].filter(Boolean))];

    if (paths.length > 0) {
      await refreshDirectories(paths, paneIds);
      return;
    }

    await Promise.all(paneIds.map((paneId) => reloadPane(paneId)));
  }

  async function copyToDirectory(targetDirectory, options = {}) {
    const paneId = activePane();
    const entries = operationEntries();

    if (entries.length === 0 || !targetDirectory) {
      return;
    }

    let nameForEntry = null;

    if (options.promptRename && entries.length === 1) {
      const nextName = (await dialog.prompt({
        title: 'Copy Item',
        message: entries[0].name,
        inputLabel: 'Copy as',
        inputValue: entries[0].name,
        confirmLabel: 'Copy',
      }))?.trim();

      if (!nextName) {
        return;
      }

      nameForEntry = () => nextName;
    }

    const copied = await transfers.copyEntries({
      entries,
      targetDirectory,
      nameForEntry,
    });

    if (copied) {
      await reloadTransferPanes(paneId, options.targetPaneId || paneId, [targetDirectory]);
    }
  }

  async function moveToDirectory(targetDirectory) {
    const paneId = activePane();
    const entries = operationEntries();

    if (entries.length === 0 || !targetDirectory) {
      return;
    }

    const moved = await transfers.moveEntries({
      entries,
      targetDirectory,
    });

    if (moved) {
      await reloadTransferPanes(paneId, otherPaneId(paneId), [
        targetDirectory,
        ...parentDirectoriesForEntries(entries),
      ]);
    }
  }

  async function renameFocused() {
    const paneId = activePane();
    const entry = store.selectedEntryFor(paneId);

    if (!entry) {
      return;
    }

    const nextName = (await dialog.prompt({
      title: 'Rename Item',
      message: entry.name,
      inputLabel: 'Name',
      inputValue: entry.name,
      confirmLabel: 'Rename',
    }))?.trim();

    if (!nextName || nextName === entry.name) {
      return;
    }

    const renamed = await transfers.renameEntry(entry, nextName);

    if (renamed) {
      await refreshDirectories([store.parentDirectoryFor(entry.path)]);
    }
  }

  async function createDirectory(targetPaneId = activePane(), seedName = 'New Folder') {
    const targetDirectory = currentPath(targetPaneId);

    if (isArchivePath(targetDirectory)) {
      await dialog.alert({
        title: 'New Folder Not Available',
        message: 'Archive contents are read-only while browsing.',
        variant: 'warning',
      });
      return;
    }

    const name = (await dialog.prompt({
      title: 'Create Folder',
      icon: 'folder',
      inputLabel: 'Folder name',
      inputValue: seedName,
      confirmLabel: 'Create',
      inputRequired: true,
    }))?.trim();

    if (!name || !targetDirectory) {
      return;
    }

    await createFolder(joinPath(targetDirectory, name));
    await refreshDirectories([targetDirectory], [targetPaneId]);
  }

  async function deleteSelected() {
    const paneId = activePane();
    const entries = operationEntries();

    if (entries.length === 0) {
      return;
    }

    if (entries.some((entry) => isArchivePath(entry.path))) {
      await dialog.alert({
        title: 'Delete Not Available',
        message: 'Archive contents are read-only while browsing.',
        variant: 'warning',
      });
      return;
    }

    const useTrash = store.appSettings.deleteMode === 'trash';

    if (useTrash && entries.some((entry) => isRemotePath(entry.path))) {
      await dialog.alert({
        title: 'Trash Not Available',
        message: 'Remote storage items can only be deleted permanently. Change deletion behavior to Delete permanently to continue.',
        variant: 'warning',
      });
      return;
    }

    const label = entries.length === 1 ? `"${entries[0].name}"` : `${entries.length} items`;

    const confirmed = store.appSettings.confirmDelete
      ? await dialog.confirm({
          title: useTrash ? 'Move to Trash' : 'Delete Items',
          message: useTrash ? `Move ${label} to Trash?` : `Delete ${label} permanently?`,
          detail: useTrash
            ? 'Local items can be restored from the system Trash.'
            : 'This cannot be undone from inside the app.',
          confirmLabel: useTrash ? 'Move to Trash' : 'Delete',
          variant: useTrash ? 'warning' : 'danger',
          destructive: !useTrash,
        })
      : true;

    if (!confirmed) {
      return;
    }

    const touchedDirectories = parentDirectoriesForEntries(entries);
    await deleteItems(entries.map((entry) => entry.path), store.appSettings.deleteMode);
    store.clearSelection(paneId);
    await refreshDirectories(touchedDirectories);
  }

  async function openFocusedExternally() {
    const entry = store.selectedEntryFor(activePane());

    if (!entry) {
      return;
    }

    if (entry.kind === 'directory') {
      store.openSelectedEntry(activePane());
      return;
    }

    await openWithDefaultApp(entry.path);
  }

  async function editFocusedFile() {
    const entry = store.selectedEntryFor(activePane());

    if (!entry || entry.kind !== 'file' || isArchivePath(entry.path)) {
      return;
    }

    await editFile(entry.path, store.appSettings.editorCommand);
  }

  function previewFocused() {
    if (!store.previewPanelVisible) {
      store.togglePreviewPanel(true);
    }
  }

  async function readSystemClipboardText() {
    if (navigator.clipboard?.read) {
      try {
        const items = await navigator.clipboard.read();
        const preferredTypes = [
          'x-special/gnome-copied-files',
          'text/uri-list',
          'text/plain',
        ];

        for (const preferredType of preferredTypes) {
          const item = items.find((candidate) => candidate.types.includes(preferredType));

          if (item) {
            return await (await item.getType(preferredType)).text();
          }
        }
      } catch {
        // Fall back to readText below.
      }
    }

    if (!navigator.clipboard?.readText) {
      return null;
    }

    try {
      return await navigator.clipboard.readText();
    } catch {
      return null;
    }
  }

  async function writeSystemClipboard(mode, entries) {
    const gnomeText = clipboardTextForEntries(mode, entries);
    const gnomePayload = clipboardGnomePayloadForEntries(mode, entries);
    const uriList = clipboardUriListForEntries(entries);

    if (canUseSystemFileClipboard(entries)) {
      try {
        const wroteNativeClipboard = await writeSystemFileClipboard(
          mode,
          entries.map((entry) => entry.path),
        );

        if (wroteNativeClipboard) {
          return;
        }
      } catch {
        // Fall back to Web Clipboard below.
      }
    }

    if (navigator.clipboard?.write && typeof ClipboardItem !== 'undefined') {
      const clipboardTypes = {
        'text/plain': new Blob([gnomeText], { type: 'text/plain' }),
        'text/uri-list': new Blob([uriList], { type: 'text/uri-list' }),
      };

      try {
        clipboardTypes['x-special/gnome-copied-files'] = new Blob(
          [gnomePayload],
          { type: 'x-special/gnome-copied-files' },
        );
        await navigator.clipboard.write([new ClipboardItem(clipboardTypes)]);
        return;
      } catch {
        try {
          delete clipboardTypes['x-special/gnome-copied-files'];
          await navigator.clipboard.write([new ClipboardItem(clipboardTypes)]);
          return;
        } catch {
          // Fall back to writeText below.
        }
      }
    }

    await navigator.clipboard?.writeText(gnomeText);
  }

  function clipboardPayloadForEntries(mode, entries, sourcePaneId = activePane()) {
    return {
      mode,
      sourcePaneId,
      entries: entries.map((entry) => ({
        name: entry.name,
        path: entry.path,
        kind: entry.kind,
        size: entry.size,
        modifiedAt: entry.modifiedAt,
        isHidden: entry.isHidden,
        isSymlink: entry.isSymlink,
        isReadonly: entry.isReadonly,
      })),
    };
  }

  async function setClipboard(mode) {
    const entries = operationEntries();

    if (!FILE_CLIPBOARD_MODES.has(mode) || entries.length === 0) {
      return;
    }

    if (mode === 'move' && entries.some((entry) => isArchivePath(entry.path))) {
      await dialog.alert({
        title: 'Cut Not Available',
        message: 'Archive contents are read-only while browsing.',
        detail: 'Use copy to extract items from the archive.',
        variant: 'warning',
      });
      return;
    }

    fileClipboard = clipboardPayloadForEntries(mode, entries);
    storeClipboard(fileClipboard);

    writeSystemClipboard(mode, entries).catch(() => {});
  }

  async function clipboardPayloadFromSystemText(text) {
    const parsed = parseClipboardText(text);

    if (!parsed) {
      return null;
    }

    const stored = storedClipboard();

    if (stored && samePathList(parsed.paths, stored.entries)) {
      return stored;
    }

    if (fileClipboard && samePathList(parsed.paths, fileClipboard.entries)) {
      return fileClipboard;
    }

    const entries = await entriesFromClipboardPaths(parsed.paths);

    if (entries.length === 0) {
      return null;
    }

    return clipboardPayloadForEntries(parsed.mode || 'copy', entries, null);
  }

  async function clipboardPayloadFromSystemFiles(payload) {
    if (!payload?.paths?.length) {
      return null;
    }

    const stored = storedClipboard();

    if (stored && samePathList(payload.paths, stored.entries)) {
      return stored;
    }

    if (fileClipboard && samePathList(payload.paths, fileClipboard.entries)) {
      return fileClipboard;
    }

    const entries = await entriesFromClipboardPaths(payload.paths);

    if (entries.length === 0) {
      return null;
    }

    return clipboardPayloadForEntries(payload.mode || 'copy', entries, null);
  }

  async function currentClipboardPayload() {
    try {
      const systemFileClipboard = await readSystemFileClipboard();
      const parsedSystemFileClipboard = await clipboardPayloadFromSystemFiles(systemFileClipboard);

      if (parsedSystemFileClipboard) {
        fileClipboard = parsedSystemFileClipboard;
        return parsedSystemFileClipboard;
      }
    } catch {
      // Fall back to text and internal clipboard below.
    }

    const clipboardText = await readSystemClipboardText();

    if (clipboardText !== null) {
      const parsed = await clipboardPayloadFromSystemText(clipboardText);

      if (parsed) {
        fileClipboard = parsed;
        return parsed;
      }

      return null;
    }

    return fileClipboard || storedClipboard();
  }

  async function pasteClipboard() {
    const clipboardPayload = await currentClipboardPayload();

    if (!clipboardPayload?.entries?.length) {
      return;
    }

    const targetPaneId = activePane();
    const targetDirectory = currentPath();
    if (clipboardPayload.mode === 'move') {
      const moved = await transfers.moveEntries({
        entries: clipboardPayload.entries,
        targetDirectory,
      });

      if (moved) {
        await reloadTransferPanes(clipboardPayload.sourcePaneId, targetPaneId, [
          targetDirectory,
          ...parentDirectoriesForEntries(clipboardPayload.entries),
        ]);
        fileClipboard = null;
        clearStoredClipboard();
      }
    } else {
      const copied = await transfers.copyEntries({
        entries: clipboardPayload.entries,
        targetDirectory,
      });

      if (copied) {
        await refreshDirectories([targetDirectory], [targetPaneId]);
      }
    }
  }

  function copyFocusedName(fullPath = false) {
    const entry = store.selectedEntryFor(activePane());
    const value = fullPath ? entry?.path : entry?.name;

    if (value) {
      navigator.clipboard?.writeText(value).catch(() => {});
    }
  }

  function copyCurrentPath() {
    navigator.clipboard?.writeText(currentPath()).catch(() => {});
  }

  function createTabFromFocused() {
    const entry = store.selectedEntryFor(activePane());

    if (entry?.kind === 'directory') {
      store.addPaneTab(activePane(), entry.path);
    } else if (isArchiveEntry(entry)) {
      store.addPaneTab(activePane(), archiveRootPath(entry.path));
    } else {
      store.addPaneTab(activePane());
    }
  }

  async function executeCommand(commandId) {
    const paneId = activePane();
    const targetPaneId = otherPaneId(paneId);
    const targetPath = currentPath(targetPaneId);

    try {
      switch (commandId) {
        case 'palette.commands':
          store.openCommandPalette();
          return;
        case 'palette.files':
          store.openFileSearch();
          return;
        case 'palette.content':
          store.openContentSearch();
          return;
        case 'file.open':
          await openFocusedExternally();
          return;
        case 'file.edit':
          await editFocusedFile();
          return;
        case 'file.preview':
          previewFocused();
          return;
        case 'file.copyOtherPane':
          await copyToDirectory(targetPath, { targetPaneId });
          return;
        case 'file.copyHereRename':
          await copyToDirectory(currentPath(), { promptRename: true });
          return;
        case 'file.moveOtherPane':
          await moveToDirectory(targetPath);
          return;
        case 'file.rename':
          await renameFocused();
          return;
        case 'file.newFolder':
          await createDirectory();
          return;
        case 'file.newFolderOtherPane':
          await createDirectory(targetPaneId, store.selectedEntryFor(paneId)?.name || 'New Folder');
          return;
        case 'file.delete':
          await deleteSelected();
          return;
        case 'file.contextMenu':
          dispatchSelectedContextMenu(store);
          return;
        case 'clipboard.copy':
          await setClipboard('copy');
          return;
        case 'clipboard.cut':
          await setClipboard('move');
          return;
        case 'clipboard.paste':
          await pasteClipboard();
          return;
        case 'clipboard.copyName':
          copyFocusedName(false);
          return;
        case 'clipboard.copyFocusedPath':
          copyFocusedName(true);
          return;
        case 'clipboard.copyCurrentPath':
          copyCurrentPath();
          return;
        case 'pane.switch':
          store.switchActivePane();
          return;
        case 'pane.left':
          store.setActivePane('left');
          return;
        case 'pane.right':
          store.setActivePane('right');
          return;
        case 'pane.back':
          store.goBack();
          return;
        case 'pane.forward':
          store.goForward();
          return;
        case 'pane.parent':
          store.goToParent(paneId);
          return;
        case 'pane.root':
          store.setPanePath(paneId, '/');
          return;
        case 'pane.openInOtherPane':
          store.openFocusedDirectoryInOtherPane(paneId);
          return;
        case 'pane.newTabFromFocused':
          createTabFromFocused();
          return;
        case 'pane.refresh':
          await reloadPane();
          return;
        case 'selection.selectAll':
          store.selectAllEntries(paneId);
          return;
        case 'selection.clear':
          store.clearSelection(paneId);
          return;
        case 'selection.invert':
          store.invertSelection(paneId);
          return;
        case 'tabs.new':
          store.addPaneTab(paneId);
          return;
        case 'tabs.close':
          store.closeActivePaneTab();
          return;
        case 'tabs.next':
          store.activateAdjacentTab(paneId, 1);
          return;
        case 'tabs.previous':
          store.activateAdjacentTab(paneId, -1);
          return;
        case 'layout.swapPanes':
          store.swapPanes();
          return;
        case 'layout.sidebar':
          store.toggleSidebar();
          return;
        case 'layout.preview':
          store.togglePreviewPanel();
          return;
        case 'layout.terminal':
          store.toggleTerminalPanel();
          return;
        case 'view.hidden':
          store.toggleHiddenFiles();
          return;
        case 'view.grid':
          store.setPaneView(paneId, 'grid');
          return;
        case 'view.list':
          store.setPaneView(paneId, 'list');
          return;
        case 'sort.name':
          store.setPaneSortKey(paneId, 'name');
          return;
        case 'sort.extension':
          store.setPaneSortKey(paneId, 'extension');
          return;
        case 'sort.modifiedAt':
          store.setPaneSortKey(paneId, 'modifiedAt');
          return;
        case 'sort.size':
          store.setPaneSortKey(paneId, 'size');
          return;
        case 'sort.none':
          store.setPaneSortKey(paneId, 'none');
          return;
        case 'sort.direction':
          store.togglePaneSortDirection(paneId);
          return;
        case 'sidebar.remoteStorage':
          window.dispatchEvent(new CustomEvent(OPEN_REMOTE_STORAGE_EVENT));
          return;
        case 'sidebar.newGroup':
          window.dispatchEvent(new CustomEvent(CREATE_SIDEBAR_GROUP_EVENT));
          return;
        case 'app.settings':
          store.openSettings();
          return;
        case 'app.shortcuts':
          shortcutsModal.show();
          return;
        default:
          return;
      }
    } catch (error) {
      console.error(error);
      await dialog.alert({
        title: 'Command Failed',
        message: error?.message || 'The selected command could not be completed.',
        variant: 'warning',
      });
    }
  }

  async function handleShortcut(event) {
    const key = event.key;
    const code = event.code;
    const lowerKey = key.toLowerCase();
    const command = isCommand(event);
    const onlyCommand = command && !event.altKey && !event.shiftKey;

    if (command && event.shiftKey && lowerKey === 'p' && !event.altKey) {
      event.preventDefault();
      store.openCommandPalette();
      return;
    }

    if (command && event.shiftKey && lowerKey === 'f' && !event.altKey) {
      event.preventDefault();
      store.openContentSearch();
      return;
    }

    if (onlyCommand && lowerKey === 'p') {
      event.preventDefault();
      store.openFileSearch();
      return;
    }

    if (key === 'F1' && !command && !event.altKey && !event.shiftKey) {
      event.preventDefault();
      shortcutsModal.show();
      return;
    }

    if (isEditableTarget(event.target)) {
      return;
    }

    const paneId = activePane();
    const targetPaneId = otherPaneId(paneId);
    const targetPath = currentPath(targetPaneId);

    try {
      if (onlyCommand && key === ',') {
        event.preventDefault();
        store.openSettings();
        return;
      }

      if (command && key === 'Tab') {
        event.preventDefault();
        store.activateAdjacentTab(paneId, event.shiftKey ? -1 : 1);
        return;
      }

      if (event.altKey && !command && key === 'F1') {
        event.preventDefault();
        store.setActivePane('left');
        return;
      }

      if (event.altKey && !command && key === 'F2') {
        event.preventDefault();
        store.setActivePane('right');
        return;
      }

      if (event.altKey && !command && key === 'F7') {
        event.preventDefault();
        focusSearch();
        return;
      }

      if (onlyCommand && key === 'F1') {
        event.preventDefault();
        store.setPaneView(paneId, 'grid');
        return;
      }

      if (onlyCommand && key === 'F2') {
        event.preventDefault();
        store.setPaneView(paneId, 'list');
        return;
      }

      if (onlyCommand && key === 'F3') {
        event.preventDefault();
        store.setPaneSortKey(paneId, 'name');
        return;
      }

      if (onlyCommand && key === 'F4') {
        event.preventDefault();
        store.setPaneSortKey(paneId, 'extension');
        return;
      }

      if (onlyCommand && key === 'F5') {
        event.preventDefault();
        store.setPaneSortKey(paneId, 'modifiedAt');
        return;
      }

      if (onlyCommand && key === 'F6') {
        event.preventDefault();
        store.setPaneSortKey(paneId, 'size');
        return;
      }

      if (onlyCommand && key === 'F7') {
        event.preventDefault();
        store.setPaneSortKey(paneId, 'none');
        return;
      }

      if (key === 'Tab') {
        event.preventDefault();
        store.switchActivePane();
        return;
      }

      if (event.shiftKey && key === 'Tab') {
        event.preventDefault();
        store.switchActivePane();
        return;
      }

      if (key === 'ArrowUp') {
        event.preventDefault();
        store.moveSelection(paneId, -1, { extend: event.shiftKey });
        return;
      }

      if (key === 'ArrowDown') {
        event.preventDefault();
        store.moveSelection(paneId, 1, { extend: event.shiftKey });
        return;
      }

      if (key === 'Enter' && !event.altKey && !command) {
        event.preventDefault();
        await openFocusedExternally();
        return;
      }

      if (key === 'Backspace' && !event.altKey && !command) {
        event.preventDefault();
        store.goToParent(paneId);
        return;
      }

      if (key === 'F1' && !command && !event.altKey && !event.shiftKey) {
        event.preventDefault();
        shortcutsModal.show();
        return;
      }

      if (key === 'F2' || (onlyCommand && lowerKey === 'r')) {
        event.preventDefault();
        await reloadPane();
        return;
      }

      if (key === 'F3') {
        event.preventDefault();
        previewFocused();
        return;
      }

      if (key === 'F4' && !command && !event.altKey) {
        event.preventDefault();
        await editFocusedFile();
        return;
      }

      if (key === 'F5' && event.shiftKey && !command && !event.altKey) {
        event.preventDefault();
        await copyToDirectory(currentPath(), { promptRename: true });
        return;
      }

      if (key === 'F5' && !command && !event.altKey) {
        event.preventDefault();
        await copyToDirectory(targetPath, { targetPaneId });
        return;
      }

      if (key === 'F6' && event.shiftKey && !command && !event.altKey) {
        event.preventDefault();
        await renameFocused();
        return;
      }

      if (key === 'F6' && !command && !event.altKey) {
        event.preventDefault();
        await moveToDirectory(targetPath);
        return;
      }

      if (key === 'F7' && event.shiftKey && !command && !event.altKey) {
        event.preventDefault();
        await createDirectory(targetPaneId, store.selectedEntryFor(paneId)?.name || 'New Folder');
        return;
      }

      if (key === 'F7' && !command && !event.altKey) {
        event.preventDefault();
        await createDirectory();
        return;
      }

      if (key === 'F8' || key === 'Delete') {
        event.preventDefault();
        await deleteSelected();
        return;
      }

      if ((key === 'F10' && event.shiftKey) || key === 'ContextMenu') {
        event.preventDefault();
        dispatchSelectedContextMenu(store);
        return;
      }

      if (event.altKey && key === 'ArrowLeft') {
        event.preventDefault();
        store.goBack();
        return;
      }

      if (event.altKey && key === 'ArrowRight') {
        event.preventDefault();
        store.goForward();
        return;
      }

      if (event.altKey && key === 'Enter') {
        event.preventDefault();
        if (!store.previewPanelVisible) {
          store.togglePreviewPanel(true);
        }
        return;
      }

      if (onlyCommand && key === '[') {
        event.preventDefault();
        store.goBack();
        return;
      }

      if (onlyCommand && key === ']') {
        event.preventDefault();
        store.goForward();
        return;
      }

      if (onlyCommand && lowerKey === 'b') {
        event.preventDefault();
        store.toggleSidebar();
        return;
      }

      if (onlyCommand && lowerKey === 'i') {
        event.preventDefault();
        if (event.ctrlKey && !event.metaKey) {
          store.switchActivePane();
        } else {
          store.togglePreviewPanel();
        }
        return;
      }

      if (onlyCommand && key === '`') {
        event.preventDefault();
        store.toggleTerminalPanel();
        return;
      }

      if (onlyCommand && lowerKey === 'f') {
        event.preventDefault();
        focusSearch();
        return;
      }

      if (onlyCommand && lowerKey === 's') {
        event.preventDefault();
        focusSearch();
        return;
      }

      if (onlyCommand && lowerKey === 't') {
        event.preventDefault();
        store.addPaneTab(paneId);
        return;
      }

      if (onlyCommand && lowerKey === 'w') {
        event.preventDefault();
        store.closeActivePaneTab();
        return;
      }

      if (onlyCommand && lowerKey === 'u') {
        event.preventDefault();
        store.swapPanes();
        return;
      }

      if (onlyCommand && lowerKey === 'q') {
        event.preventDefault();
        store.togglePreviewPanel();
        return;
      }

      if (onlyCommand && lowerKey === 'm') {
        event.preventDefault();
        await renameFocused();
        return;
      }

      if (onlyCommand && lowerKey === 'c') {
        event.preventDefault();
        await setClipboard('copy');
        return;
      }

      if (onlyCommand && lowerKey === 'x') {
        event.preventDefault();
        await setClipboard('move');
        return;
      }

      if (onlyCommand && lowerKey === 'v') {
        event.preventDefault();
        await pasteClipboard();
        return;
      }

      if (onlyCommand && lowerKey === 'a') {
        event.preventDefault();
        store.selectAllEntries(paneId);
        return;
      }

      if (onlyCommand && key === '.') {
        event.preventDefault();
        store.toggleHiddenFiles();
        return;
      }

      if (command && key === 'Enter') {
        event.preventDefault();
        copyFocusedName(event.shiftKey);
        return;
      }

      if (command && key === 'Insert') {
        event.preventDefault();
        await setClipboard('copy');
        return;
      }

      if (event.shiftKey && key === 'Insert') {
        event.preventDefault();
        await pasteClipboard();
        return;
      }

      if (onlyCommand && key === '\\') {
        event.preventDefault();
        store.setPanePath(paneId, '/');
        return;
      }

      if (command && key === 'PageUp') {
        event.preventDefault();
        store.goToParent(paneId);
        return;
      }

      if (command && key === 'PageDown') {
        event.preventDefault();
        store.openSelectedEntry(paneId);
        return;
      }

      if (command && key === 'ArrowLeft') {
        event.preventDefault();
        store.openFocusedDirectoryInOtherPane(paneId);
        return;
      }

      if (command && key === 'ArrowRight') {
        event.preventDefault();
        store.openFocusedDirectoryInOtherPane(paneId);
        return;
      }

      if (command && key === 'ArrowUp') {
        event.preventDefault();
        createTabFromFocused();
        return;
      }

      if (key === 'Insert' || key === ' ') {
        event.preventDefault();
        store.toggleEntrySelection(paneId, null, key === 'Insert');
        return;
      }

      if (key === 'Home') {
        event.preventDefault();
        store.selectFirstEntry(paneId, { extend: event.shiftKey });
        return;
      }

      if (key === 'End') {
        event.preventDefault();
        store.selectLastEntry(paneId, { extend: event.shiftKey });
        return;
      }

      if (key === 'PageUp') {
        event.preventDefault();
        store.pageSelection(paneId, -1, { extend: event.shiftKey });
        return;
      }

      if (key === 'PageDown') {
        event.preventDefault();
        store.pageSelection(paneId, 1, { extend: event.shiftKey });
        return;
      }

      if (code === 'NumpadAdd' || key === '+') {
        event.preventDefault();
        if (event.altKey) {
          store.selectEntriesWithFocusedExtension(paneId, true);
        } else {
          store.selectAllEntries(paneId);
        }
        return;
      }

      if (code === 'NumpadSubtract' || key === '-') {
        event.preventDefault();
        if (event.altKey) {
          store.selectEntriesWithFocusedExtension(paneId, false);
        } else {
          store.clearSelection(paneId);
        }
        return;
      }

      if (code === 'NumpadMultiply' || key === '*') {
        event.preventDefault();
        store.invertSelection(paneId);
        return;
      }

      if (code === 'NumpadDivide' || key === '/') {
        event.preventDefault();
        store.clearSelection(paneId);
      }
    } catch (error) {
      console.error(error);
      await dialog.alert({
        title: 'Shortcut Action Failed',
        message: error?.message || 'The requested shortcut action could not be completed.',
        variant: 'warning',
      });
    }
  }

  function handleKeydown(event) {
    handleShortcut(event);
  }

  function handleCommandEvent(event) {
    const commandId = event?.detail?.id;

    if (commandId) {
      executeCommand(commandId);
    }
  }

  onMounted(() => {
    window.addEventListener('keydown', handleKeydown);
    window.addEventListener(RUN_COMMAND_EVENT, handleCommandEvent);
  });

  onUnmounted(() => {
    window.removeEventListener('keydown', handleKeydown);
    window.removeEventListener(RUN_COMMAND_EVENT, handleCommandEvent);
  });
}
