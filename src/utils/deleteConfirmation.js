function entryIsRemote(entry) {
  return String(entry?.path || '').startsWith('remote://');
}

export function hasRemoteEntries(entries = []) {
  return entries.some(entryIsRemote);
}

export function shouldConfirmDelete(confirmDelete, deleteMode, entries = []) {
  return Boolean(confirmDelete) || (deleteMode === 'trash' && hasRemoteEntries(entries));
}

export function deleteConfirmationOptions({
  entries = [],
  deleteMode = 'trash',
  label = 'selected items',
  singleTitle = 'Delete Item',
  pluralTitle = 'Delete Items',
} = {}) {
  const useTrash = deleteMode === 'trash';
  const remoteCount = entries.filter(entryIsRemote).length;
  const localCount = Math.max(0, entries.length - remoteCount);

  if (useTrash && remoteCount > 0) {
    if (localCount > 0) {
      return {
        title: pluralTitle,
        message: 'Delete remote items permanently and move local items to Trash?',
        detail: 'Remote storage does not support Trash. Local items can still be restored from the system Trash.',
        confirmLabel: 'Delete and Move to Trash',
        variant: 'danger',
        destructive: true,
      };
    }

    return {
      title: entries.length === 1 ? singleTitle : pluralTitle,
      message: `Delete ${label} permanently?`,
      detail: 'Remote storage does not support Trash. This cannot be undone from inside the app.',
      confirmLabel: 'Delete',
      variant: 'danger',
      destructive: true,
    };
  }

  return {
    title: useTrash ? 'Move to Trash' : (entries.length === 1 ? singleTitle : pluralTitle),
    message: useTrash ? `Move ${label} to Trash?` : `Delete ${label} permanently?`,
    detail: useTrash
      ? 'Local items can be restored from the system Trash.'
      : 'This cannot be undone from inside the app.',
    confirmLabel: useTrash ? 'Move to Trash' : 'Delete',
    variant: useTrash ? 'warning' : 'danger',
    destructive: !useTrash,
  };
}
