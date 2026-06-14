<script setup>
import { listen } from '@tauri-apps/api/event';
import { computed, nextTick, onBeforeUnmount, ref, watch } from 'vue';
import AppIcon from './AppIcon.vue';
import { canUseLocalFileAssets, searchContent, searchFiles } from '../composables/useFileOperations';
import { useScrollableContentState } from '../composables/useScrollableContentState';
import { useFileManagerStore } from '../stores/fileManagerStore';
import { isArchivePath } from '../utils/archivePaths';
import { RUN_COMMAND_EVENT } from '../utils/appEvents';

const SEARCH_LIMIT = 80;
const CONTENT_SEARCH_MAX_FILE_BYTES = 24 * 1024 * 1024;
const store = useFileManagerStore();
const input = ref(null);
const resultList = ref(null);
const resultButtons = ref([]);
const query = ref('');
const results = ref([]);
const selectedIndex = ref(0);
const loading = ref(false);
const error = ref('');
const activeSearchJobId = ref('');
let searchTimer = null;
let searchVersion = 0;
let stopFileSearchResultsListener = null;

function pluralize(count, singular, plural = `${singular}s`) {
  return `${count} ${count === 1 ? singular : plural}`;
}

const contentMatchedLineCount = computed(() => (
  results.value.reduce((total, result) => total + Math.max(Number(result.matchCount) || 1, 1), 0)
));
const activeSearchJob = computed(() => (
  activeSearchJobId.value
    ? store.queue.find((job) => job.id === activeSearchJobId.value) || null
    : null
));
const currentMode = computed(() => store.fileSearchMode || 'files');
const { isScrollable: resultListScrollable } = useScrollableContentState(resultList, {
  watch: [
    () => store.fileSearchVisible,
    currentMode,
    () => results.value.length,
    loading,
    error,
  ],
});
const isCommandMode = computed(() => currentMode.value === 'commands');
const activeRoot = computed(() => (
  store.effectiveDirectoryFor(store.activePaneId) || store.activeTabFor(store.activePaneId)?.currentPath || '~'
));
const targetPaneId = computed(() => (store.activePaneId === 'left' ? 'right' : 'left'));
const targetRoot = computed(() => (
  store.effectiveDirectoryFor(targetPaneId.value) || store.activeTabFor(targetPaneId.value)?.currentPath || '~'
));
const selectedEntry = computed(() => store.selectedEntryFor(store.activePaneId));
const operationEntries = computed(() => store.operationEntriesFor(store.activePaneId));
const hasOperationEntries = computed(() => operationEntries.value.length > 0);
const hasFocusedEntry = computed(() => Boolean(selectedEntry.value));
const hasArchiveOperationEntries = computed(() => (
  operationEntries.value.some((entry) => isArchivePath(entry.path))
));
const canBatchRenameOperationEntries = computed(() => (
  operationEntries.value.length > 1 &&
  operationEntries.value.every((entry) => canRenamePath(entry.path))
));
const canVerifyChecksum = computed(() => (
  operationEntries.value.length > 0 &&
  operationEntries.value.every((entry) => entry.kind === 'file' && !isArchivePath(entry.path))
));
const canCompareFolders = computed(() => {
  const isLocalDir = (path) => Boolean(path) && !path.startsWith('remote://') && !isArchivePath(path);
  const left = store.effectiveDirectoryFor('left') || store.activeTabFor('left')?.currentPath || '';
  const right = store.effectiveDirectoryFor('right') || store.activeTabFor('right')?.currentPath || '';
  return isLocalDir(left) && isLocalDir(right);
});
const canSearchRoot = computed(() => {
  const root = activeRoot.value;
  return isCommandMode.value || (canUseLocalFileAssets()
    && root
    && !isArchivePath(root));
});

function remoteVolumeIdForPath(path) {
  const value = String(path || '');

  if (!value.startsWith('remote://')) {
    return '';
  }

  return value.slice('remote://'.length).split('/').filter(Boolean)[0] || '';
}

function canRenamePath(path) {
  if (!path || isArchivePath(path)) {
    return false;
  }

  const volumeId = remoteVolumeIdForPath(path);

  if (!volumeId) {
    return true;
  }

  const rootPath = `remote://${volumeId}/`;
  const volume = (store.volumes || []).find((candidate) => candidate.path === rootPath);

  return !volume || volume.capabilities?.canRename !== false;
}

const commandDefinitions = [
  {
    id: 'edit.undo',
    section: 'Edit',
    title: 'Undo last operation',
    detail: () => (store.undoLabel ? `Reverse: ${store.undoLabel}` : 'Nothing to undo'),
    icon: 'undo',
    shortcut: 'Ctrl Z',
    when: () => store.canUndo,
    keywords: 'undo revert reverse back history move copy rename',
  },
  {
    id: 'edit.redo',
    section: 'Edit',
    title: 'Redo last operation',
    detail: () => (store.redoLabel ? `Reapply: ${store.redoLabel}` : 'Nothing to redo'),
    icon: 'redo',
    shortcut: 'Ctrl Shift Z',
    when: () => store.canRedo,
    keywords: 'redo reapply forward history move copy rename',
  },
  {
    id: 'palette.files',
    section: 'Search',
    title: 'Search files',
    detail: 'Fuzzy search the current folder',
    icon: 'file',
    shortcut: 'Ctrl P',
    keywords: 'finder filename current folder',
  },
  {
    id: 'palette.content',
    section: 'Search',
    title: 'Search file contents',
    detail: 'Find text inside files in the current folder',
    icon: 'search',
    shortcut: 'Ctrl Shift F',
    keywords: 'grep content text full text',
  },
  {
    id: 'file.open',
    section: 'File',
    title: 'Open selected item',
    detail: () => selectedEntry.value?.kind === 'directory' ? 'Open folder in the active pane' : 'Open with the default app',
    icon: 'file',
    shortcut: 'Enter',
    when: () => hasFocusedEntry.value,
    keywords: 'launch default app folder directory',
  },
  {
    id: 'file.edit',
    section: 'File',
    title: 'Edit selected file',
    detail: 'Open the focused file in your configured editor',
    icon: 'file-code',
    shortcut: 'F4',
    when: () => selectedEntry.value?.kind === 'file' && !isArchivePath(selectedEntry.value?.path),
    keywords: 'editor code modify',
  },
  {
    id: 'file.preview',
    section: 'File',
    title: 'Preview selected item',
    detail: 'Show the preview panel for the focused item',
    icon: 'eye',
    shortcut: 'F3',
    when: () => hasFocusedEntry.value,
    keywords: 'inspect info side panel',
  },
  {
    id: 'file.quickLook',
    section: 'File',
    title: 'Quick Look',
    detail: 'Open a large preview overlay for the focused item',
    icon: 'eye',
    shortcut: 'Space',
    when: () => hasFocusedEntry.value,
    keywords: 'quick look space preview overlay peek',
  },
  {
    id: 'file.copyOtherPane',
    section: 'File',
    title: 'Copy to other pane',
    detail: () => `Copy selection to ${targetRoot.value}`,
    icon: 'copy',
    shortcut: 'F5',
    when: () => hasOperationEntries.value,
    keywords: 'duplicate transfer',
  },
  {
    id: 'file.copyHereRename',
    section: 'File',
    title: 'Copy here with new name',
    detail: 'Copy the focused item in place and choose a new name',
    icon: 'copy',
    shortcut: 'Shift F5',
    when: () => operationEntries.value.length === 1,
    keywords: 'duplicate rename',
  },
  {
    id: 'file.moveOtherPane',
    section: 'File',
    title: 'Move to other pane',
    detail: () => `Move selection to ${targetRoot.value}`,
    icon: 'open-other-pane',
    shortcut: 'F6',
    when: () => hasOperationEntries.value && !hasArchiveOperationEntries.value,
    keywords: 'transfer relocate',
  },
  {
    id: 'file.rename',
    section: 'File',
    title: 'Rename selected item',
    detail: 'Change the focused file or folder name',
    icon: 'file-text',
    shortcut: 'F2',
    when: () => hasFocusedEntry.value && canRenamePath(selectedEntry.value?.path),
    keywords: 'name',
  },
  {
    id: 'tools.compareFolders',
    section: 'File',
    title: 'Compare & sync folders (left ↔ right)',
    detail: 'Diff the two panes and copy or mirror differences',
    icon: 'columns',
    when: () => canCompareFolders.value,
    keywords: 'compare sync diff folders directories mirror merge two-way panes difference',
  },
  {
    id: 'file.verifyChecksum',
    section: 'File',
    title: 'Verify checksum (SHA-256)',
    detail: () => (operationEntries.value.length === 1
      ? 'Compute the hash and compare it to an expected value'
      : `Compare checksums of ${operationEntries.value.length} files`),
    icon: 'shield',
    when: () => canVerifyChecksum.value,
    keywords: 'checksum hash sha256 sha-256 verify integrity digest fingerprint compare',
  },
  {
    id: 'file.batchRename',
    section: 'File',
    title: 'Batch rename selected items',
    detail: () => `${operationEntries.value.length} items selected`,
    icon: 'file-text',
    when: () => canBatchRenameOperationEntries.value && !hasArchiveOperationEntries.value,
    keywords: 'bulk rename multiple pattern replace number',
  },
  {
    id: 'file.newFolder',
    section: 'File',
    title: 'New folder',
    detail: 'Create a folder in the active pane',
    icon: 'folder-plus',
    shortcut: 'F7',
    when: () => !isArchivePath(activeRoot.value),
    keywords: 'directory mkdir create',
  },
  {
    id: 'file.newFolderOtherPane',
    section: 'File',
    title: 'New folder in other pane',
    detail: () => `Create a folder in ${targetRoot.value}`,
    icon: 'folder-plus',
    shortcut: 'Shift F7',
    when: () => !isArchivePath(targetRoot.value),
    keywords: 'directory mkdir create target',
  },
  {
    id: 'file.newFile',
    section: 'File',
    title: 'New file',
    detail: 'Create an empty file in the active pane',
    icon: 'file-plus',
    when: () => !isArchivePath(activeRoot.value),
    keywords: 'empty file touch create document',
  },
  {
    id: 'file.delete',
    section: 'File',
    title: 'Delete selected items',
    detail: 'Delete the current selection',
    icon: 'trash',
    shortcut: 'F8',
    when: () => hasOperationEntries.value && !hasArchiveOperationEntries.value,
    keywords: 'remove erase',
  },
  {
    id: 'file.contextMenu',
    section: 'File',
    title: 'Show context menu',
    detail: 'Open actions for the focused item',
    icon: 'menu',
    shortcut: 'Shift F10',
    when: () => hasFocusedEntry.value,
    keywords: 'right click actions tools open with',
  },
  {
    id: 'clipboard.copy',
    section: 'Clipboard',
    title: 'Copy files',
    detail: 'Copy selected files to the file clipboard',
    icon: 'copy',
    shortcut: 'Ctrl C',
    when: () => hasOperationEntries.value,
    keywords: 'clipboard duplicate',
  },
  {
    id: 'clipboard.cut',
    section: 'Clipboard',
    title: 'Cut files',
    detail: 'Mark selected files to move on paste',
    icon: 'copy',
    shortcut: 'Ctrl X',
    when: () => hasOperationEntries.value && !hasArchiveOperationEntries.value,
    keywords: 'clipboard move',
  },
  {
    id: 'clipboard.paste',
    section: 'Clipboard',
    title: 'Paste files here',
    detail: 'Paste files into the active pane',
    icon: 'download',
    shortcut: 'Ctrl V',
    when: () => !isArchivePath(activeRoot.value),
    keywords: 'clipboard insert',
  },
  {
    id: 'clipboard.copyName',
    section: 'Clipboard',
    title: 'Copy focused name',
    detail: 'Copy only the selected file name',
    icon: 'file-text',
    shortcut: 'Ctrl Enter',
    when: () => hasFocusedEntry.value,
    keywords: 'filename clipboard',
  },
  {
    id: 'clipboard.copyFocusedPath',
    section: 'Clipboard',
    title: 'Copy focused path',
    detail: 'Copy the selected file path',
    icon: 'file-text',
    shortcut: 'Ctrl Shift Enter',
    when: () => hasFocusedEntry.value,
    keywords: 'absolute path clipboard',
  },
  {
    id: 'clipboard.copyCurrentPath',
    section: 'Clipboard',
    title: 'Copy current folder path',
    detail: () => activeRoot.value,
    icon: 'folder',
    keywords: 'directory location cwd clipboard',
  },
  {
    id: 'pane.switch',
    section: 'Navigation',
    title: 'Switch active pane',
    detail: 'Move focus to the other file pane',
    icon: 'columns',
    shortcut: 'Tab',
    keywords: 'left right focus',
  },
  {
    id: 'pane.left',
    section: 'Navigation',
    title: 'Focus left pane',
    detail: 'Make the left pane active',
    icon: 'columns',
    shortcut: 'Alt F1',
    keywords: 'left focus',
  },
  {
    id: 'pane.right',
    section: 'Navigation',
    title: 'Focus right pane',
    detail: 'Make the right pane active',
    icon: 'columns',
    shortcut: 'Alt F2',
    keywords: 'right focus',
  },
  {
    id: 'pane.back',
    section: 'Navigation',
    title: 'Go back',
    detail: 'Navigate back in pane history',
    icon: 'chevron-left',
    shortcut: 'Alt Left',
    keywords: 'history previous',
  },
  {
    id: 'pane.forward',
    section: 'Navigation',
    title: 'Go forward',
    detail: 'Navigate forward in pane history',
    icon: 'chevron-right',
    shortcut: 'Alt Right',
    keywords: 'history next',
  },
  {
    id: 'pane.parent',
    section: 'Navigation',
    title: 'Go to parent folder',
    detail: 'Move one level up',
    icon: 'folder',
    shortcut: 'Backspace',
    keywords: 'up directory parent',
  },
  {
    id: 'pane.root',
    section: 'Navigation',
    title: 'Go to file system root',
    detail: 'Open / in the active pane',
    icon: 'drive',
    shortcut: 'Ctrl \\',
    keywords: 'root filesystem',
  },
  {
    id: 'pane.editPath',
    section: 'Navigation',
    title: 'Go to path…',
    detail: 'Type or paste a path in the active pane',
    icon: 'folder',
    shortcut: 'Ctrl L',
    keywords: 'address bar location path edit go to jump type navigate',
  },
  {
    id: 'pane.openInOtherPane',
    section: 'Navigation',
    title: 'Open focused folder in other pane',
    detail: 'Load the focused directory beside the current pane',
    icon: 'open-other-pane',
    shortcut: 'Ctrl Right',
    when: () => selectedEntry.value?.kind === 'directory',
    keywords: 'target pane sibling',
  },
  {
    id: 'pane.newTabFromFocused',
    section: 'Navigation',
    title: 'New tab from focused item',
    detail: 'Open the focused folder or archive in a new tab',
    icon: 'plus',
    shortcut: 'Ctrl Up',
    keywords: 'tab folder archive',
  },
  {
    id: 'pane.refresh',
    section: 'Navigation',
    title: 'Refresh current folder',
    detail: 'Reload the active pane',
    icon: 'refresh',
    shortcut: 'Ctrl R',
    keywords: 'reload rescan',
  },
  {
    id: 'selection.selectAll',
    section: 'Selection',
    title: 'Select all',
    detail: 'Select every visible item',
    icon: 'check',
    shortcut: 'Ctrl A',
    keywords: 'mark all',
  },
  {
    id: 'selection.clear',
    section: 'Selection',
    title: 'Clear selection',
    detail: 'Clear selected items',
    icon: 'minus',
    shortcut: 'Num -',
    keywords: 'unselect deselect',
  },
  {
    id: 'selection.invert',
    section: 'Selection',
    title: 'Invert selection',
    detail: 'Select unselected items and clear selected ones',
    icon: 'check',
    shortcut: 'Num *',
    keywords: 'toggle mark',
  },
  {
    id: 'tabs.new',
    section: 'Tabs',
    title: 'New tab',
    detail: 'Open a new tab in the active pane',
    icon: 'plus',
    shortcut: 'Ctrl T',
    keywords: 'create tab',
  },
  {
    id: 'tabs.close',
    section: 'Tabs',
    title: 'Close current tab',
    detail: 'Close the active pane tab',
    icon: 'x',
    shortcut: 'Ctrl W',
    keywords: 'remove tab',
  },
  {
    id: 'tabs.next',
    section: 'Tabs',
    title: 'Next tab',
    detail: 'Activate the next tab in this pane',
    icon: 'chevron-right',
    shortcut: 'Ctrl Tab',
    keywords: 'cycle tab',
  },
  {
    id: 'tabs.previous',
    section: 'Tabs',
    title: 'Previous tab',
    detail: 'Activate the previous tab in this pane',
    icon: 'chevron-left',
    shortcut: 'Ctrl Shift Tab',
    keywords: 'cycle tab',
  },
  {
    id: 'layout.swapPanes',
    section: 'Layout',
    title: 'Swap panes',
    detail: 'Exchange left and right pane contents',
    icon: 'columns',
    shortcut: 'Ctrl U',
    keywords: 'layout exchange',
  },
  {
    id: 'layout.sidebar',
    section: 'Layout',
    title: () => store.sidebarVisible ? 'Hide sidebar' : 'Show sidebar',
    detail: 'Toggle locations, favorites, devices, and remote storage',
    icon: 'sidebar',
    shortcut: 'Ctrl B',
    keywords: 'locations favorites devices remote',
  },
  {
    id: 'layout.preview',
    section: 'Layout',
    title: () => store.previewPanelVisible ? 'Hide preview panel' : 'Show preview panel',
    detail: 'Toggle the inspector and preview panel',
    icon: 'panel-right',
    shortcut: 'Ctrl I',
    keywords: 'inspector info details',
  },
  {
    id: 'layout.terminal',
    section: 'Layout',
    title: () => store.terminalPanelVisible ? 'Hide terminal' : 'Show terminal',
    detail: 'Toggle the integrated terminal',
    icon: 'terminal',
    shortcut: 'Ctrl `',
    keywords: 'shell command line',
  },
  {
    id: 'view.hidden',
    section: 'View',
    title: () => store.showHiddenFiles ? 'Hide hidden files' : 'Show hidden files',
    detail: 'Toggle dotfiles and hidden entries',
    icon: () => store.showHiddenFiles ? 'eye-off' : 'eye',
    shortcut: 'Ctrl .',
    keywords: 'dotfiles invisible',
  },
  {
    id: 'view.grid',
    section: 'View',
    title: 'Grid view',
    detail: 'Show the active pane as a grid',
    icon: 'grid',
    shortcut: 'Ctrl F1',
    keywords: 'icons layout',
  },
  {
    id: 'view.list',
    section: 'View',
    title: 'List view',
    detail: 'Show the active pane as a list',
    icon: 'list',
    shortcut: 'Ctrl F2',
    keywords: 'details table',
  },
  {
    id: 'sort.name',
    section: 'Sort',
    title: 'Sort by name',
    detail: 'Order entries by name',
    icon: 'list',
    shortcut: 'Ctrl F3',
    keywords: 'ordering alphabetical',
  },
  {
    id: 'sort.extension',
    section: 'Sort',
    title: 'Sort by extension',
    detail: 'Order entries by file extension',
    icon: 'file',
    shortcut: 'Ctrl F4',
    keywords: 'type suffix',
  },
  {
    id: 'sort.modifiedAt',
    section: 'Sort',
    title: 'Sort by date modified',
    detail: 'Order entries by modification date',
    icon: 'activity-list',
    shortcut: 'Ctrl F5',
    keywords: 'time date newest',
  },
  {
    id: 'sort.size',
    section: 'Sort',
    title: 'Sort by size',
    detail: 'Order entries by file size',
    icon: 'archive',
    shortcut: 'Ctrl F6',
    keywords: 'bytes large small',
  },
  {
    id: 'sort.none',
    section: 'Sort',
    title: 'No sorting',
    detail: 'Use the directory order from the source',
    icon: 'list',
    shortcut: 'Ctrl F7',
    keywords: 'unsorted natural',
  },
  {
    id: 'sort.direction',
    section: 'Sort',
    title: 'Reverse sort direction',
    detail: 'Toggle ascending and descending order',
    icon: 'chevron-down',
    keywords: 'ascending descending order',
  },
  {
    id: 'sidebar.remoteStorage',
    section: 'Sidebar',
    title: 'Connect remote storage',
    detail: 'Add SFTP, FTP, SMB, WebDAV, or S3 storage',
    icon: 'network',
    keywords: 'server network volume connect',
  },
  {
    id: 'sidebar.newGroup',
    section: 'Sidebar',
    title: 'New sidebar group',
    detail: 'Create a custom section for shortcuts',
    icon: 'folder-plus',
    keywords: 'favorites bookmarks section',
  },
  {
    id: 'app.settings',
    section: 'App',
    title: 'Open settings',
    detail: 'Change Carelo preferences',
    icon: 'settings',
    shortcut: 'Ctrl ,',
    keywords: 'preferences configure',
  },
  {
    id: 'app.shortcuts',
    section: 'App',
    title: 'Keyboard shortcuts',
    detail: 'Show the searchable shortcut overview',
    icon: 'info',
    shortcut: 'F1',
    keywords: 'help hotkeys keys',
  },
];
const statusText = computed(() => {
  if (isCommandMode.value) {
    return pluralize(results.value.length, 'command');
  }

  if (!canSearchRoot.value) {
    return 'Search unavailable';
  }

  if (!query.value.trim()) {
    return 'Ready';
  }

  if (loading.value) {
    if (currentMode.value === 'content' || currentMode.value === 'files') {
      const scannedFiles = Number(activeSearchJob.value?.processedEntries || 0);
      const matchedItems = Number(activeSearchJob.value?.currentBytes || 0);
      const scannedLabel = currentMode.value === 'content' ? 'file' : 'item';
      const matchedLabel = currentMode.value === 'content' ? 'file' : 'match';

      if (scannedFiles > 0) {
        return matchedItems > 0
          ? `${pluralize(scannedFiles, scannedLabel)} scanned, ${pluralize(matchedItems, matchedLabel)}`
          : `${pluralize(scannedFiles, scannedLabel)} scanned`;
      }
    }

    return 'Searching';
  }

  if (error.value) {
    return 'Search unavailable';
  }

  if (currentMode.value === 'content') {
    return `${pluralize(results.value.length, 'file')}, ${pluralize(contentMatchedLineCount.value, 'line')}`;
  }

  return pluralize(results.value.length, 'result');
});
const inputPlaceholder = computed(() => (
  currentMode.value === 'commands'
    ? 'Select a command...'
    : currentMode.value === 'content'
    ? 'Search file contents in current folder'
    : 'Search files in current folder'
));
const emptyPlaceholder = computed(() => (
  currentMode.value === 'commands'
    ? 'Type to find an app command'
    : currentMode.value === 'content'
    ? 'Type to search inside files in the current folder'
    : 'Type to fuzzy search the current folder'
));
const dialogLabel = computed(() => (
  currentMode.value === 'commands'
    ? 'Command palette'
    : currentMode.value === 'content'
      ? 'Content search'
      : 'Fuzzy file search'
));
const paletteSubtitle = computed(() => (
  currentMode.value === 'commands' ? `Active pane: ${activeRoot.value}` : activeRoot.value
));

function normalizeCommandText(value) {
  return String(value || '').toLowerCase().replace(/\s+/g, ' ').trim();
}

function commandValue(value) {
  return typeof value === 'function' ? value() : value;
}

function commandIsVisible(command) {
  return typeof command.when === 'function' ? command.when() : true;
}

function commandScore(command, query) {
  const title = normalizeCommandText(command.title);
  const section = normalizeCommandText(command.section);
  const shortcut = normalizeCommandText(command.shortcut);
  const detail = normalizeCommandText(command.detail);
  const keywords = normalizeCommandText(command.keywords);
  const terms = query.split(' ').filter(Boolean);

  if (terms.length === 0) {
    return 1;
  }

  let score = 0;

  for (const term of terms) {
    if (title.startsWith(term)) {
      score += 120;
    } else if (title.includes(term)) {
      score += 80;
    } else if (section.includes(term)) {
      score += 48;
    } else if (shortcut.includes(term)) {
      score += 42;
    } else if (keywords.includes(term)) {
      score += 32;
    } else if (detail.includes(term)) {
      score += 20;
    } else {
      return 0;
    }
  }

  return score;
}

const commandResults = computed(() => {
  const queryText = normalizeCommandText(query.value);

  return commandDefinitions
    .map((definition, index) => {
      const command = {
        ...definition,
        title: commandValue(definition.title),
        detail: commandValue(definition.detail),
        icon: commandValue(definition.icon) || 'app',
        shortcut: commandValue(definition.shortcut),
      };

      if (!commandIsVisible(command)) {
        return null;
      }

      const score = commandScore(command, queryText);

      if (queryText && score <= 0) {
        return null;
      }

      return {
        type: 'command',
        commandId: command.id,
        name: command.title,
        path: `command:${command.id}`,
        parentPath: command.detail,
        section: command.section,
        icon: command.icon,
        shortcut: command.shortcut,
        score,
        order: index,
      };
    })
    .filter(Boolean)
    .sort((a, b) => b.score - a.score || a.order - b.order)
    .slice(0, SEARCH_LIMIT);
});

function refreshCommandResults() {
  results.value = commandResults.value;
  selectedIndex.value = Math.min(selectedIndex.value, Math.max(results.value.length - 1, 0));
  error.value = '';
  loading.value = false;
}

function setMode(mode) {
  cancelActiveSearchJob();
  store.openFileSearch(mode);
  results.value = [];
  selectedIndex.value = 0;
  error.value = '';
  scheduleSearch();
}

function close() {
  store.closeFileSearch();
}

function resetSearch() {
  cancelActiveSearchJob();
  results.value = [];
  selectedIndex.value = 0;
  error.value = '';
  loading.value = false;
  searchVersion += 1;
  clearTimeout(searchTimer);
}

function isOperationCancelled(searchError) {
  return (
    searchError?.code === 'operation_cancelled' ||
    /cancelled/i.test(String(searchError?.message || searchError || ''))
  );
}

function cancelActiveSearchJob() {
  const jobId = activeSearchJobId.value;

  if (!jobId) {
    return;
  }

  activeSearchJobId.value = '';
  store.cancelQueueJob(jobId).catch(() => {});
}

async function ensureFileSearchResultsListener() {
  if (stopFileSearchResultsListener || !canUseLocalFileAssets()) {
    return;
  }

  try {
    stopFileSearchResultsListener = await listen('file-search-results', (event) => {
      const payload = event.payload || {};

      if (
        currentMode.value !== 'files' ||
        payload.jobId !== activeSearchJobId.value ||
        String(payload.query || '') !== query.value.trim()
      ) {
        return;
      }

      results.value = Array.isArray(payload.results) ? payload.results : [];
      selectedIndex.value = Math.min(selectedIndex.value, Math.max(results.value.length - 1, 0));
    });
  } catch {
    stopFileSearchResultsListener = null;
  }
}

async function runSearch() {
  cancelActiveSearchJob();
  const version = ++searchVersion;

  if (isCommandMode.value) {
    refreshCommandResults();
    return;
  }

  if (!query.value.trim()) {
    results.value = [];
    selectedIndex.value = 0;
    error.value = '';
    loading.value = false;
    return;
  }

  if (!store.fileSearchVisible || !canSearchRoot.value) {
    resetSearch();
    return;
  }

  loading.value = true;
  error.value = '';
  const isContentSearch = currentMode.value === 'content';
  const jobId = store.startQueueJob({
    operation: isContentSearch ? 'content-search' : 'file-search',
    label: isContentSearch ? 'Content search' : 'File search',
    detail: isContentSearch ? 'Scanning file contents' : 'Scanning file names',
    remotePaths: [activeRoot.value],
    pausable: false,
    cancelable: true,
  });

  if (jobId) {
    activeSearchJobId.value = jobId;
  }

  if (!isContentSearch) {
    await ensureFileSearchResultsListener();
  }

  try {
    const nextResults = isContentSearch
      ? await searchContent(activeRoot.value, query.value, {
          limit: 120,
          includeHidden: store.showHiddenFiles,
          respectIgnore: true,
          caseSensitive: false,
          regex: false,
          maxFileBytes: CONTENT_SEARCH_MAX_FILE_BYTES,
        }, jobId)
      : await searchFiles(activeRoot.value, query.value, {
          limit: SEARCH_LIMIT,
          includeHidden: store.showHiddenFiles,
          respectIgnore: true,
          includeFiles: true,
          includeDirectories: true,
          followSymlinks: false,
        }, jobId);

    if (version !== searchVersion) {
      if (jobId) {
        store.cancelQueueJobDone(jobId, 'Superseded');
      }
      return;
    }

    results.value = Array.isArray(nextResults) ? nextResults : [];
    selectedIndex.value = Math.min(selectedIndex.value, Math.max(results.value.length - 1, 0));

    if (jobId) {
      store.completeQueueJob(
        jobId,
        `${pluralize(results.value.length, isContentSearch ? 'file' : 'result')} found`,
      );
    }
  } catch (searchError) {
    if (isOperationCancelled(searchError)) {
      if (jobId) {
        store.cancelQueueJobDone(jobId, version === searchVersion ? 'Cancelled' : 'Superseded');
      }

      if (version === searchVersion) {
        results.value = [];
        selectedIndex.value = 0;
      }

      return;
    }

    if (version !== searchVersion) {
      if (jobId) {
        store.cancelQueueJobDone(jobId, 'Superseded');
      }
      return;
    }

    if (jobId) {
      store.failQueueJob(jobId, searchError?.message || 'Unable to search this folder.');
    }

    results.value = [];
    selectedIndex.value = 0;
    error.value = searchError?.message || 'Unable to search this folder.';
  } finally {
    if (jobId && activeSearchJobId.value === jobId) {
      activeSearchJobId.value = '';
    }

    if (version === searchVersion) {
      loading.value = false;
    }
  }
}

function scheduleSearch() {
  clearTimeout(searchTimer);
  searchVersion += 1;
  searchTimer = setTimeout(runSearch, 90);
}

function selectRelative(delta) {
  if (results.value.length === 0) {
    return;
  }

  selectedIndex.value = (selectedIndex.value + delta + results.value.length) % results.value.length;
}

function setResultButton(element, index) {
  if (element) {
    resultButtons.value[index] = element;
  }
}

async function scrollSelectedResultIntoView() {
  await nextTick();

  const container = resultList.value;
  const element = resultButtons.value[selectedIndex.value];

  if (!container || !element) {
    return;
  }

  const containerTop = container.scrollTop;
  const containerBottom = containerTop + container.clientHeight;
  const elementTop = element.offsetTop;
  const elementBottom = elementTop + element.offsetHeight;

  if (elementTop < containerTop) {
    container.scrollTop = elementTop;
  } else if (elementBottom > containerBottom) {
    container.scrollTop = elementBottom - container.clientHeight;
  }
}

async function openResult(result = results.value[selectedIndex.value]) {
  if (!result) {
    return;
  }

  if (result.type === 'command') {
    if (result.commandId === 'palette.files') {
      query.value = '';
      setMode('files');
      return;
    }

    if (result.commandId === 'palette.content') {
      query.value = '';
      setMode('content');
      return;
    }

    close();
    window.setTimeout(() => {
      window.dispatchEvent(new CustomEvent(RUN_COMMAND_EVENT, {
        detail: { id: result.commandId },
      }));
    }, 0);
    return;
  }

  await store.revealPathInPane(store.activePaneId, result.path, result.kind);
  close();
}

function handleKeydown(event) {
  if (event.key === 'Escape') {
    event.preventDefault();
    close();
  } else if (event.key === 'ArrowDown') {
    event.preventDefault();
    selectRelative(1);
  } else if (event.key === 'ArrowUp') {
    event.preventDefault();
    selectRelative(-1);
  } else if (event.key === 'Enter') {
    event.preventDefault();
    openResult();
  } else if (event.key === 'Tab') {
    event.preventDefault();
    const modes = ['commands', 'files', 'content'];
    const currentIndex = modes.indexOf(currentMode.value);
    const nextIndex = (currentIndex + (event.shiftKey ? -1 : 1) + modes.length) % modes.length;
    setMode(modes[nextIndex]);
  }
}

function resultIcon(result) {
  if (result?.type === 'command') {
    return result.icon || 'app';
  }

  return result?.kind === 'directory' ? 'folder' : 'file';
}

function resultKey(result) {
  return result.path;
}

function resultTitle(result) {
  return result.name;
}

function resultShortcut(result) {
  return result?.type === 'command' ? result.shortcut : '';
}

function resultTitleSegments(result) {
  const title = resultTitle(result);
  const chars = Array.from(title);
  const matched = new Set(Array.isArray(result?.matchIndices) ? result.matchIndices : []);

  if (matched.size === 0) {
    return [{ text: title, match: false }];
  }

  const segments = [];
  let current = '';
  let currentMatch = matched.has(0);

  chars.forEach((char, index) => {
    const charMatch = matched.has(index);

    if (charMatch !== currentMatch && current) {
      segments.push({ text: current, match: currentMatch });
      current = '';
    }

    current += char;
    currentMatch = charMatch;
  });

  if (current) {
    segments.push({ text: current, match: currentMatch });
  }

  return segments;
}

function contentResultMeta(result) {
  const lineNumber = Math.max(Number(result?.lineNumber) || 1, 1);
  const matchCount = Math.max(Number(result?.matchCount) || 1, 1);
  const lineCountText = matchCount === 1 ? '1 line' : `${matchCount} lines`;

  return `Line ${lineNumber} / ${lineCountText}`;
}

watch(
  () => store.fileSearchVisible,
  async (visible) => {
    if (!visible) {
      resetSearch();
      query.value = '';
      return;
    }

    await nextTick();
    input.value?.focus();
    input.value?.select?.();
    scheduleSearch();
  },
);

watch([query, activeRoot, currentMode, () => store.showHiddenFiles], () => {
  if (store.fileSearchVisible) {
    selectedIndex.value = 0;
    scheduleSearch();
  }
});

watch(commandResults, () => {
  if (store.fileSearchVisible && isCommandMode.value) {
    refreshCommandResults();
  }
});

watch(results, () => {
  resultButtons.value = [];
});

watch(selectedIndex, () => {
  if (store.fileSearchVisible) {
    scrollSelectedResultIntoView();
  }
});

onBeforeUnmount(() => {
  resetSearch();
  if (stopFileSearchResultsListener) {
    stopFileSearchResultsListener();
    stopFileSearchResultsListener = null;
  }
});
</script>

<template>
  <Teleport to="body">
    <Transition name="command-palette">
      <div
        v-if="store.fileSearchVisible"
        class="command-palette__overlay"
        @pointerdown.self="close"
      >
        <section
          class="command-palette"
          :class="{ 'command-palette--content-scrollable': resultListScrollable }"
          role="dialog"
          aria-modal="true"
          :aria-label="dialogLabel"
        >
          <header class="command-palette__header">
            <div class="command-palette__title-group">
              <div class="command-palette__title-text">
                <h2>{{ dialogLabel }}</h2>
                <span class="command-palette__root" :title="paletteSubtitle">{{ paletteSubtitle }}</span>
              </div>
            </div>

            <button
              type="button"
              class="command-palette__close"
              aria-label="Close palette"
              @click="close"
            >
              <AppIcon name="x" :size="14" :stroke-width="2" />
            </button>
          </header>

          <div class="command-palette__modes" role="tablist" aria-label="Palette mode">
            <button
              type="button"
              class="command-palette__mode"
              :class="{ 'command-palette__mode--active': currentMode === 'commands' }"
              role="tab"
              :aria-selected="currentMode === 'commands'"
              @click="setMode('commands')"
            >
              <AppIcon name="app" :size="13" :stroke-width="1.8" />
              <span>Commands</span>
            </button>
            <button
              type="button"
              class="command-palette__mode"
              :class="{ 'command-palette__mode--active': currentMode === 'files' }"
              role="tab"
              :aria-selected="currentMode === 'files'"
              @click="setMode('files')"
            >
              <AppIcon name="file" :size="13" :stroke-width="1.8" />
              <span>Files</span>
            </button>
            <button
              type="button"
              class="command-palette__mode"
              :class="{ 'command-palette__mode--active': currentMode === 'content' }"
              role="tab"
              :aria-selected="currentMode === 'content'"
              @click="setMode('content')"
            >
              <AppIcon name="search" :size="13" :stroke-width="1.8" />
              <span>Content</span>
            </button>
          </div>

          <div class="command-palette__search">
            <span class="command-palette__search-icon" aria-hidden="true">
              <AppIcon name="search" :size="16" :stroke-width="1.9" />
            </span>
            <input
              ref="input"
              v-model="query"
              class="command-palette__input"
              type="search"
              spellcheck="false"
              autocomplete="off"
              :placeholder="inputPlaceholder"
              @keydown="handleKeydown"
            >
            <span
              class="command-palette__status"
              :class="{
                'command-palette__status--loading': loading,
                'command-palette__status--error': Boolean(error),
              }"
            >
              <span v-if="loading" class="command-palette__spinner" aria-hidden="true"></span>
              {{ statusText }}
            </span>
          </div>

          <div ref="resultList" class="command-palette__results" role="listbox">
            <button
              v-for="(result, index) in results"
              :key="resultKey(result)"
              :ref="(element) => setResultButton(element, index)"
              class="command-palette__result"
              :class="{
                'command-palette__result--active': index === selectedIndex,
                'command-palette__result--content': currentMode === 'content',
                'command-palette__result--command': result.type === 'command',
              }"
              type="button"
              role="option"
              :aria-selected="index === selectedIndex"
              @mouseenter="selectedIndex = index"
              @click="openResult(result)"
            >
              <span class="command-palette__icon" aria-hidden="true">
                <AppIcon :name="resultIcon(result)" :size="16" :stroke-width="1.8" />
              </span>
              <span class="command-palette__result-main">
                <span class="command-palette__title-row">
                  <span class="command-palette__name">
                    <template
                      v-for="(segment, segmentIndex) in resultTitleSegments(result)"
                      :key="`${resultKey(result)}-${segmentIndex}`"
                    >
                      <mark
                        v-if="segment.match"
                        class="command-palette__name-match"
                      >{{ segment.text }}</mark>
                      <span v-else>{{ segment.text }}</span>
                    </template>
                  </span>
                  <span
                    v-if="currentMode === 'content'"
                    class="command-palette__match-meta"
                  >{{ contentResultMeta(result) }}</span>
                  <span
                    v-else-if="result.type === 'command'"
                    class="command-palette__match-meta"
                  >{{ result.section }}</span>
                </span>
                <span
                  v-if="result.type === 'command' && result.parentPath"
                  class="command-palette__path"
                >{{ result.parentPath }}</span>
                <span
                  v-else-if="currentMode === 'content' && result.lineText"
                  class="command-palette__snippet"
                >{{ result.lineText }}</span>
                <span
                  v-else-if="currentMode !== 'content' && result.parentPath"
                  class="command-palette__path"
                >{{ result.parentPath }}</span>
                <span
                  v-if="currentMode === 'content'"
                  class="command-palette__path"
                >{{ result.parentPath }}</span>
              </span>
              <span
                v-if="resultShortcut(result)"
                class="command-palette__shortcut"
                aria-hidden="true"
              >
                <kbd>{{ resultShortcut(result) }}</kbd>
              </span>
              <span
                v-else-if="index === selectedIndex"
                class="command-palette__enter-hint"
                aria-hidden="true"
              >
                <kbd>↵</kbd>
              </span>
            </button>

            <div v-if="error" class="command-palette__empty command-palette__empty--error">
              <AppIcon name="alert" :size="22" :stroke-width="1.6" />
              <span>{{ error }}</span>
            </div>
            <div
              v-else-if="!query.trim() && results.length === 0"
              class="command-palette__empty"
            >
              <AppIcon name="search" :size="22" :stroke-width="1.5" />
              <span>{{ emptyPlaceholder }}</span>
            </div>
            <div
              v-else-if="!loading && results.length === 0"
              class="command-palette__empty"
            >
              <AppIcon name="search" :size="22" :stroke-width="1.5" />
              <span>No matches</span>
            </div>
          </div>

          <footer class="command-palette__footer">
            <span class="command-palette__hint">
              <kbd>↑</kbd><kbd>↓</kbd>
              <span>Navigate</span>
            </span>
            <span class="command-palette__hint">
              <kbd>↵</kbd>
              <span>Open</span>
            </span>
            <span class="command-palette__hint">
              <kbd>Esc</kbd>
              <span>Close</span>
            </span>
          </footer>
        </section>
      </div>
    </Transition>
  </Teleport>
</template>

<style scoped>
/* ── Overlay ──────────────────────────────────────────────── */
.command-palette__overlay {
  position: fixed;
  z-index: 5050;
  inset: 0;
  display: grid;
  place-items: start center;
  padding: max(72px, 10vh) 24px 24px;
  background: var(--overlay-bg);
}

/* ── Panel ────────────────────────────────────────────────── */
.command-palette {
  display: flex;
  flex-direction: column;
  width: min(720px, calc(100vw - 48px));
  max-height: min(640px, calc(100vh - 120px));
  overflow: hidden;
  border: 1px solid var(--control-border);
  border-radius: 14px;
  background: var(--modal-bg);
  box-shadow: var(--shadow-overlay);
  color: var(--text);
}

/* ── Header ───────────────────────────────────────────────── */
.command-palette__header {
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: 12px;
  padding: 14px 16px;
  border-bottom: 1px solid transparent;
  flex-shrink: 0;
}

.command-palette--content-scrollable .command-palette__header {
  border-bottom-color: var(--hairline);
}

.command-palette__title-group {
  display: flex;
  min-width: 0;
  align-items: center;
  gap: 11px;
}

.command-palette__title-text {
  display: grid;
  min-width: 0;
  gap: 2px;
}

.command-palette__title-text h2 {
  margin: 0;
  color: var(--text);
  font-size: 14px;
  font-weight: 700;
  letter-spacing: -0.01em;
  line-height: 1.1;
}

.command-palette__root {
  overflow: hidden;
  color: var(--text-faint);
  font-size: 11.5px;
  font-weight: 560;
  line-height: 1.1;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.command-palette__close {
  display: grid;
  width: 26px;
  height: 26px;
  flex: 0 0 auto;
  place-items: center;
  border-radius: 7px;
  background: transparent;
  color: var(--icon);
  transition: background 100ms ease, color 100ms ease;
}

.command-palette__close:hover {
  background: var(--btn-hover);
  color: var(--text);
}

/* ── Mode tabs (segmented control) ────────────────────────── */
.command-palette__modes {
  display: inline-flex;
  align-self: flex-start;
  margin: 12px 16px 0;
  border: 1px solid var(--input-border);
  border-radius: 9px;
  padding: 3px;
  background: var(--input-bg);
  box-shadow: var(--input-shadow);
  gap: 2px;
}

.command-palette__mode {
  display: inline-flex;
  height: 26px;
  min-width: 86px;
  align-items: center;
  justify-content: center;
  gap: 6px;
  padding: 0 12px;
  border-radius: 6px;
  background: transparent;
  color: var(--text-muted);
  font-size: 12px;
  font-weight: 600;
  transition: background 120ms ease, color 120ms ease, box-shadow 120ms ease;
}

.command-palette__mode:hover {
  color: var(--text);
}

.command-palette__mode--active {
  background: var(--control-bg);
  color: var(--text);
  box-shadow:
    0 1px 2px rgb(0 0 0 / 0.25),
    inset 0 0 0 1px var(--control-border);
}

.command-palette__mode--active :deep(svg) {
  color: var(--accent);
}

/* ── Search row ───────────────────────────────────────────── */
.command-palette__search {
  display: grid;
  grid-template-columns: 18px minmax(0, 1fr) auto;
  gap: 11px;
  align-items: center;
  margin: 12px 16px 0;
  height: 42px;
  padding: 0 13px;
  border: 1px solid var(--input-border);
  border-radius: 10px;
  background: var(--input-bg);
  box-shadow: var(--input-shadow);
  color: var(--text-muted);
  transition: border-color 120ms ease, box-shadow 120ms ease;
}

.command-palette__search:focus-within {
  border-color: var(--accent-border);
  box-shadow:
    var(--accent-focus-ring),
    var(--input-shadow);
}

.command-palette__search-icon {
  display: grid;
  place-items: center;
  color: var(--accent);
}

.command-palette__input {
  width: 100%;
  min-width: 0;
  border: 0;
  outline: 0;
  background: transparent;
  color: var(--text);
  font-size: 14px;
  font-weight: 520;
}

.command-palette__input::placeholder {
  color: var(--text-faint);
  font-weight: 500;
}

.command-palette__input::-webkit-search-cancel-button {
  display: none;
}

.command-palette__status {
  display: inline-flex;
  align-items: center;
  gap: 6px;
  color: var(--text-faint);
  font-size: 11.5px;
  font-weight: 600;
  letter-spacing: 0.01em;
  white-space: nowrap;
}

.command-palette__status--error {
  color: var(--danger, #ff5d5d);
}

.command-palette__spinner {
  display: inline-block;
  width: 10px;
  height: 10px;
  border: 1.5px solid rgb(var(--accent-rgb) / 0.25);
  border-top-color: var(--accent);
  border-radius: 50%;
  animation: command-palette-spin 720ms linear infinite;
}

@keyframes command-palette-spin {
  to { transform: rotate(360deg); }
}

/* ── Results ──────────────────────────────────────────────── */
.command-palette__results {
  flex: 1 1 auto;
  min-height: 0;
  overflow-y: auto;
  padding: 8px;
  margin-top: 10px;
}

.command-palette__result {
  display: grid;
  width: 100%;
  min-height: 48px;
  grid-template-columns: 30px minmax(0, 1fr) auto;
  gap: 11px;
  align-items: center;
  border: 1px solid transparent;
  border-radius: 9px;
  padding: 8px 11px;
  background: transparent;
  color: var(--text);
  text-align: left;
  transition: background 80ms ease, border-color 80ms ease;
}

.command-palette__result + .command-palette__result {
  margin-top: 2px;
}

.command-palette__result--content {
  min-height: 64px;
  align-items: start;
  padding-top: 9px;
  padding-bottom: 9px;
}

.command-palette__result--command {
  min-height: 52px;
}

.command-palette__result--active {
  background: rgb(var(--accent-rgb) / 0.14);
  border-color: rgb(var(--accent-rgb) / 0.32);
}

.command-palette__icon {
  display: grid;
  width: 30px;
  height: 30px;
  place-items: center;
  border-radius: 8px;
  background: rgb(var(--accent-rgb) / 0.10);
  color: var(--accent);
}

.command-palette__result--active .command-palette__icon {
  background: rgb(var(--accent-rgb) / 0.20);
}

.command-palette__result--content .command-palette__icon {
  align-self: start;
  margin-top: 1px;
}

.command-palette__result-main {
  display: grid;
  min-width: 0;
  gap: 3px;
}

.command-palette__title-row {
  display: flex;
  min-width: 0;
  align-items: baseline;
  gap: 8px;
}

.command-palette__name,
.command-palette__path,
.command-palette__snippet,
.command-palette__match-meta {
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.command-palette__name {
  display: block;
  min-width: 0;
  color: var(--text);
  font-size: 13px;
  font-weight: 650;
  line-height: 1.15;
}

.command-palette__name-match {
  border-radius: 3px;
  padding: 0 1px;
  background: color-mix(in srgb, var(--accent) 24%, transparent);
  color: color-mix(in srgb, var(--accent) 78%, var(--text));
  font-weight: 760;
}

.command-palette__match-meta {
  flex: 0 0 auto;
  color: var(--text-faint);
  font-size: 11px;
  font-weight: 620;
  line-height: 1.15;
}

.command-palette__snippet {
  color: var(--text-muted);
  font-family: ui-monospace, "SF Mono", Menlo, Consolas, monospace;
  font-size: 11.5px;
  font-weight: 500;
  line-height: 1.3;
}

.command-palette__path {
  color: var(--text-faint);
  font-size: 11.5px;
  font-weight: 520;
  line-height: 1.2;
}

.command-palette__enter-hint {
  display: inline-flex;
  align-items: center;
  flex: 0 0 auto;
  opacity: 0.9;
}

.command-palette__shortcut {
  display: inline-flex;
  min-width: 0;
  max-width: 154px;
  align-items: center;
  justify-content: flex-end;
  flex: 0 1 auto;
  opacity: 0.86;
}

.command-palette__enter-hint kbd,
.command-palette__shortcut kbd {
  display: inline-flex;
  align-items: center;
  justify-content: center;
  min-width: 22px;
  height: 20px;
  padding: 0 6px;
  border: 1px solid var(--control-border);
  border-radius: 5px;
  background: var(--control-bg);
  box-shadow: var(--control-inset);
  color: var(--text);
  font-family: inherit;
  font-size: 11px;
  font-weight: 600;
}

.command-palette__shortcut kbd {
  max-width: 154px;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

/* ── Empty / error states ─────────────────────────────────── */
.command-palette__empty {
  display: grid;
  justify-items: center;
  gap: 10px;
  padding: 48px 16px;
  color: var(--text-faint);
  font-size: 12.5px;
  font-weight: 540;
  text-align: center;
}

.command-palette__empty :deep(svg) {
  opacity: 0.6;
}

.command-palette__empty--error {
  color: var(--danger, #ff5d5d);
}

.command-palette__empty--error :deep(svg) {
  opacity: 0.85;
}

/* ── Footer ───────────────────────────────────────────────── */
.command-palette__footer {
  display: flex;
  flex-wrap: wrap;
  align-items: center;
  gap: 14px;
  padding: 8px 14px;
  border-top: 1px solid transparent;
  background: color-mix(in srgb, var(--text) 2%, transparent);
  flex-shrink: 0;
}

.command-palette--content-scrollable .command-palette__footer {
  border-top-color: var(--hairline);
}

.command-palette__hint {
  display: inline-flex;
  align-items: center;
  gap: 5px;
  color: var(--text-faint);
  font-size: 10.5px;
  font-weight: 600;
  letter-spacing: 0.02em;
}

.command-palette__hint kbd {
  display: inline-flex;
  align-items: center;
  justify-content: center;
  min-width: 18px;
  height: 17px;
  padding: 0 5px;
  border: 1px solid var(--control-border);
  border-radius: 4px;
  background: var(--control-bg);
  box-shadow: var(--control-inset);
  color: var(--text-muted);
  font-family: inherit;
  font-size: 10px;
  font-weight: 600;
}

/* ── Transition ───────────────────────────────────────────── */
.command-palette-enter-active {
  transition: opacity 160ms ease;
}
.command-palette-leave-active {
  transition: opacity 120ms ease;
}
.command-palette-enter-active .command-palette {
  transition: transform 200ms cubic-bezier(0.2, 0, 0, 1), opacity 160ms ease;
}
.command-palette-leave-active .command-palette {
  transition: transform 120ms ease, opacity 100ms ease;
}
.command-palette-enter-from,
.command-palette-leave-to {
  opacity: 0;
}
.command-palette-enter-from .command-palette,
.command-palette-leave-to .command-palette {
  opacity: 0;
  transform: translateY(-6px) scale(0.985);
}

@media (prefers-reduced-motion: reduce) {
  .command-palette-enter-active,
  .command-palette-leave-active,
  .command-palette-enter-active .command-palette,
  .command-palette-leave-active .command-palette {
    transition: none;
  }
}
</style>
