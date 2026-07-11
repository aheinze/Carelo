function uniquePaths(paths) {
  return [...new Set(
    paths
      .map((path) => String(path || '').trim())
      .filter(Boolean),
  )];
}

export function columnRefreshPaths(request) {
  if (!request) {
    return [];
  }

  if (Array.isArray(request.paths)) {
    return uniquePaths(request.paths);
  }

  return uniquePaths([request.path]);
}

export function reconcileRefreshedColumnTrail({
  trail,
  columnIndex,
  entries,
  visibleEntries,
  childPaths = [],
  focusedPath = '',
  anchorPath = '',
  normalizePath = (path) => String(path || ''),
}) {
  const columns = Array.isArray(trail) ? trail : [];
  const currentColumn = columns[columnIndex];

  if (!currentColumn) {
    return {
      trail: columns,
      descendantsPruned: false,
      focusedEntry: null,
    };
  }

  const refreshedEntries = Array.isArray(entries) ? entries : [];
  const refreshedVisibleEntries = Array.isArray(visibleEntries) ? visibleEntries : refreshedEntries;
  const refreshedPaths = new Set(refreshedEntries.map((entry) => entry.path));
  const selectedPaths = (currentColumn.selectedPaths || [])
    .filter((path) => refreshedPaths.has(path));
  const survivingFocusedPath = refreshedVisibleEntries.some((entry) => entry.path === focusedPath)
    ? focusedPath
    : selectedPaths.find((path) => refreshedVisibleEntries.some((entry) => entry.path === path)) || '';
  const selectedIndex = survivingFocusedPath
    ? refreshedVisibleEntries.findIndex((entry) => entry.path === survivingFocusedPath)
    : -1;
  const selectionAnchorIndex = anchorPath
    ? refreshedVisibleEntries.findIndex((entry) => entry.path === anchorPath)
    : -1;
  const refreshedColumn = {
    ...currentColumn,
    entries: refreshedEntries,
    rawEntryCount: refreshedEntries.length,
    selectedIndex,
    selectionAnchorIndex: selectionAnchorIndex >= 0 ? selectionAnchorIndex : selectedIndex,
    selectedPaths,
    loading: false,
    error: '',
  };
  const nextColumn = columns[columnIndex + 1];
  const normalizedChildPaths = new Set(childPaths.map(normalizePath));
  const keepDescendants = !nextColumn || normalizedChildPaths.has(normalizePath(nextColumn.path));
  const nextTrail = [
    ...columns.slice(0, columnIndex),
    refreshedColumn,
    ...(keepDescendants ? columns.slice(columnIndex + 1) : []),
  ];

  return {
    trail: nextTrail,
    descendantsPruned: Boolean(nextColumn && !keepDescendants),
    focusedEntry: selectedIndex >= 0 ? refreshedVisibleEntries[selectedIndex] || null : null,
  };
}
