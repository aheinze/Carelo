export function renamePromptInputSelection(entry) {
  const name = String(entry?.name || '');

  if (!name) {
    return null;
  }

  return {
    start: 0,
    end: entry?.kind === 'file' ? renameBaseNameEnd(name) : name.length,
  };
}

function renameBaseNameEnd(name) {
  const extensionIndex = name.lastIndexOf('.');

  return extensionIndex > 0 ? extensionIndex : name.length;
}
