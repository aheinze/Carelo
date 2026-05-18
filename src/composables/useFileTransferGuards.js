import {
  copyItems,
  listDirectory,
  moveItems,
  renameItem,
} from './useFileOperations';
import { useDialog } from './useDialog';

export function joinPath(directory, name) {
  if (!directory || directory === '/') {
    return `/${name}`;
  }

  return directory.endsWith('/') ? `${directory}${name}` : `${directory}/${name}`;
}

export function cleanPath(path) {
  return String(path || '').replace(/\/+$/, '') || '/';
}

export function parentPath(path) {
  const clean = cleanPath(path);

  if (!clean || clean === '/' || clean === '~') {
    return clean || '~';
  }

  const index = clean.lastIndexOf('/');
  return index <= 0 ? '/' : clean.slice(0, index);
}

export function siblingPath(path, nextName) {
  return joinPath(parentPath(path), nextName);
}

export function isSameOrChildPath(path, parent) {
  const child = cleanPath(path);
  const base = cleanPath(parent);

  return child === base || (base !== '/' && child.startsWith(`${base}/`));
}

function itemLabel(count) {
  return count === 1 ? '1 item' : `${count} items`;
}

function entryKindLabel(entry) {
  return entry?.kind === 'directory' ? 'folder' : 'file';
}

function namesPreview(entries) {
  const names = entries.slice(0, 3).map((entry) => `"${entry.name}"`);
  const extra = entries.length > 3 ? ` and ${entries.length - 3} more` : '';
  return `${names.join(', ')}${extra}`;
}

async function entriesByName(directory) {
  const entries = await listDirectory(directory);
  return new Map(entries.map((entry) => [entry.name, entry]));
}

function targetNameFor(entry, nameForEntry) {
  if (typeof nameForEntry !== 'function') {
    return entry.name;
  }

  return nameForEntry(entry) || entry.name;
}

export function useFileTransferGuards() {
  const dialog = useDialog();

  async function prepareTransfer({
    entries,
    mode,
    targetDirectory,
    nameForEntry = null,
  }) {
    const sourceEntries = (Array.isArray(entries) ? entries : []).filter(Boolean);

    if (sourceEntries.length === 0 || !targetDirectory) {
      return null;
    }

    const targetEntries = await entriesByName(targetDirectory);
    const invalid = [];
    const directoryConflicts = [];
    const fileConflicts = [];
    const items = [];

    for (const entry of sourceEntries) {
      const targetName = targetNameFor(entry, nameForEntry);
      const targetPath = joinPath(targetDirectory, targetName);

      if (entry.kind === 'directory' && isSameOrChildPath(targetDirectory, entry.path)) {
        invalid.push({ entry, reason: 'contained-target' });
        continue;
      }

      if (cleanPath(targetPath) === cleanPath(entry.path)) {
        invalid.push({ entry, reason: 'same-location' });
        continue;
      }

      const existingEntry = targetEntries.get(targetName);

      if (existingEntry && cleanPath(existingEntry.path) !== cleanPath(entry.path)) {
        if (existingEntry.kind === 'directory' || entry.kind === 'directory') {
          directoryConflicts.push({ entry, existingEntry });
          continue;
        }

        fileConflicts.push({ entry, existingEntry });
      }

      items.push({ from: entry.path, to: targetPath });
    }

    if (invalid.length === sourceEntries.length) {
      await dialog.alert({
        title: mode === 'move' ? 'Move Not Possible' : 'Copy Not Possible',
        message: `No ${entryKindLabel(sourceEntries[0])} can be ${mode === 'move' ? 'moved' : 'copied'} there.`,
        detail: 'The target is the same location or inside one of the selected folders.',
        variant: 'warning',
      });
      return null;
    }

    if (directoryConflicts.length > 0) {
      await dialog.alert({
        title: 'Folder Conflict',
        message: `${namesPreview(directoryConflicts.map((conflict) => conflict.entry))} already exists in the target folder.`,
        detail: 'Replacing or merging folders is not supported yet. Rename the folder or choose another destination.',
        variant: 'warning',
      });
      return null;
    }

    if (fileConflicts.length > 0) {
      const confirmed = await dialog.confirm({
        title: 'Replace Existing Files?',
        message: `${itemLabel(fileConflicts.length)} already exist in the target folder.`,
        detail: `Replacing will overwrite ${namesPreview(fileConflicts.map((conflict) => conflict.entry))}.`,
        confirmLabel: 'Replace',
        variant: 'warning',
      });

      if (!confirmed) {
        return null;
      }
    }

    if (items.length === 0) {
      return null;
    }

    if (invalid.length > 0) {
      await dialog.alert({
        title: 'Some Items Skipped',
        message: `${itemLabel(invalid.length)} could not be ${mode === 'move' ? 'moved' : 'copied'}.`,
        detail: 'Skipped items would have ended up in the same location or inside themselves.',
        variant: 'warning',
      });
    }

    return items;
  }

  async function copyEntries(options) {
    const items = await prepareTransfer({ ...options, mode: 'copy' });

    if (!items) {
      return false;
    }

    await copyItems(items);
    return true;
  }

  async function moveEntries(options) {
    const items = await prepareTransfer({ ...options, mode: 'move' });

    if (!items) {
      return false;
    }

    await moveItems(items);
    return true;
  }

  async function renameEntry(entry, nextName) {
    const targetName = String(nextName || '').trim();

    if (!entry || !targetName || targetName === entry.name) {
      return false;
    }

    const targetDirectory = parentPath(entry.path);
    const targetPath = joinPath(targetDirectory, targetName);

    if (cleanPath(targetPath) === cleanPath(entry.path)) {
      return false;
    }

    const targetEntries = await entriesByName(targetDirectory);
    const existingEntry = targetEntries.get(targetName);

    if (existingEntry && cleanPath(existingEntry.path) !== cleanPath(entry.path)) {
      if (existingEntry.kind === 'directory' || entry.kind === 'directory') {
        await dialog.alert({
          title: 'Rename Conflict',
          message: `A ${entryKindLabel(existingEntry)} named "${targetName}" already exists.`,
          detail: 'Replacing folders during rename is not supported. Choose a different name.',
          variant: 'warning',
        });
        return false;
      }

      const confirmed = await dialog.confirm({
        title: 'Replace Existing File?',
        message: `A file named "${targetName}" already exists.`,
        detail: 'Renaming will overwrite the existing file.',
        confirmLabel: 'Replace',
        variant: 'warning',
      });

      if (!confirmed) {
        return false;
      }
    }

    await renameItem(entry.path, targetPath);
    return true;
  }

  return {
    copyEntries,
    moveEntries,
    renameEntry,
  };
}
