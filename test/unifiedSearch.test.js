import test from 'node:test';
import assert from 'node:assert/strict';
import {
  UNIFIED_SEARCH_FILE_CATEGORIES,
  buildUnifiedSearchOptions,
  canRunUnifiedSearch,
  categoriesForSearchKind,
  contentSnippetSegments,
  createUnifiedSearchFilters,
  defaultUnifiedSearchLocation,
  formatSearchModified,
  formatSearchSize,
  isUnifiedSearchRootSupported,
  normalizeSearchExtensions,
  normalizeUnifiedSearchPayload,
  parseSearchExtensions,
  reconcileUnifiedSearchFiltersForScope,
  resolveUnifiedSearchRoot,
  searchKindForCategories,
} from '../src/utils/unifiedSearch.js';

test('resolves active and Home search roots across local, remote, and archive locations', () => {
  const local = '/home/artur/projects';
  const remote = 'remote://server/projects';
  const archive = 'archive:///home/artur/files.zip!/';

  assert.equal(defaultUnifiedSearchLocation(local), 'active');
  assert.equal(defaultUnifiedSearchLocation(remote), 'active');
  assert.equal(defaultUnifiedSearchLocation(archive), 'home');
  assert.equal(resolveUnifiedSearchRoot(local, 'active'), local);
  assert.equal(resolveUnifiedSearchRoot(remote, 'active'), remote);
  assert.equal(resolveUnifiedSearchRoot(archive, 'home'), '~');
  assert.equal(resolveUnifiedSearchRoot(remote, 'home'), '~');
  assert.equal(isUnifiedSearchRootSupported(local), true);
  assert.equal(isUnifiedSearchRootSupported(remote), true);
  assert.equal(isUnifiedSearchRootSupported(archive), false);
  assert.equal(isUnifiedSearchRootSupported('~'), true);
});

test('maps the compact Any, Files, and Folders kind filter to backend categories', () => {
  const files = categoriesForSearchKind('files');
  const folders = categoriesForSearchKind('folders');

  assert.deepEqual(categoriesForSearchKind('any'), []);
  assert.deepEqual(folders, ['directory']);
  assert.deepEqual(files, UNIFIED_SEARCH_FILE_CATEGORIES);
  assert.equal(files.includes('directory'), false);
  assert.equal(searchKindForCategories([]), 'any');
  assert.equal(searchKindForCategories(folders), 'folders');
  assert.equal(searchKindForCategories([...files].reverse()), 'files');

  const fileOptions = buildUnifiedSearchOptions(createUnifiedSearchFilters({ categories: files }));
  const folderOptions = buildUnifiedSearchOptions(createUnifiedSearchFilters({ categories: folders }));

  assert.equal(fileOptions.includeFiles, true);
  assert.equal(fileOptions.includeDirectories, false);
  assert.equal(folderOptions.includeFiles, false);
  assert.equal(folderOptions.includeDirectories, true);
});

test('normalizes extensions and builds direct, filtered search options', () => {
  const filters = createUnifiedSearchFilters({
    depth: 'direct',
    matchScope: 'all',
    categories: ['directory', 'document', 'bogus'],
    extensions: '.PDF, docx pdf tar.gz bad/ext',
    modified: 'week',
    size: 'medium',
    includeHidden: true,
    respectIgnore: false,
    followSymlinks: true,
    caseSensitive: true,
    regex: true,
  });
  const options = buildUnifiedSearchOptions(filters, {
    now: Date.UTC(2026, 6, 11, 12),
    limit: 900,
  });

  assert.deepEqual(normalizeSearchExtensions('.PDF, *.docx pdf tar.gz bad/ext'), ['pdf', 'docx', 'tar.gz']);
  assert.deepEqual(options.categories, ['directory', 'document']);
  assert.deepEqual(options.extensions, ['pdf', 'docx', 'tar.gz']);
  assert.equal(options.matchScope, 'all');
  assert.equal(options.maxDepth, 1);
  assert.equal(options.includeFiles, true);
  assert.equal(options.includeDirectories, true);
  assert.equal(options.minSize, 1024 ** 2);
  assert.equal(options.maxSize, (100 * 1024 ** 2) - 1);
  assert.equal(options.modifiedAfter, Math.floor(Date.UTC(2026, 6, 4, 12) / 1000));
  assert.equal(options.includeHidden, true);
  assert.equal(options.respectIgnore, false);
  assert.equal(options.followSymlinks, true);
  assert.equal(options.limit, 500);
});

test('parses valid extensions while reporting invalid tokens separately', () => {
  const tooMany = parseSearchExtensions(
    Array.from({ length: 34 }, (_, index) => `ext${index}`).join(','),
  );

  assert.deepEqual(parseSearchExtensions('pdf, .DOCX; *.tar.gz'), {
    extensions: ['pdf', 'docx', 'tar.gz'],
    invalidTokens: [],
    truncatedCount: 0,
  });
  assert.deepEqual(parseSearchExtensions('pdf, bad/ext, *docx'), {
    extensions: ['pdf'],
    invalidTokens: ['bad/ext', '*docx'],
    truncatedCount: 0,
  });
  assert.deepEqual(parseSearchExtensions('.PDF pdf'), {
    extensions: ['pdf'],
    invalidTokens: [],
    truncatedCount: 0,
  });
  assert.equal(tooMany.extensions.length, 32);
  assert.equal(tooMany.truncatedCount, 2);
});

test('reconciles match scope without leaving inactive content filters', () => {
  const filters = createUnifiedSearchFilters({
    location: 'home',
    depth: 'direct',
    matchScope: 'all',
    categories: categoriesForSearchKind('files'),
    extensions: ['md'],
    modified: 'week',
    size: 'medium',
    includeHidden: true,
    respectIgnore: false,
    followSymlinks: true,
    caseSensitive: true,
    regex: true,
    maxFileBytes: 100 * 1024 * 1024,
  });
  const name = reconcileUnifiedSearchFiltersForScope(filters, 'name');
  const content = reconcileUnifiedSearchFiltersForScope(filters, 'content');
  const contentFolders = reconcileUnifiedSearchFiltersForScope({
    ...filters,
    categories: categoriesForSearchKind('folders'),
  }, 'content');
  const contentLarge = reconcileUnifiedSearchFiltersForScope({
    ...filters,
    size: 'large',
  }, 'content');
  const contentHuge = reconcileUnifiedSearchFiltersForScope({
    ...filters,
    size: 'huge',
  }, 'content');
  const folders = reconcileUnifiedSearchFiltersForScope({
    ...filters,
    categories: categoriesForSearchKind('folders'),
  }, 'all');

  assert.equal(name.matchScope, 'name');
  assert.equal(name.caseSensitive, false);
  assert.equal(name.regex, false);
  assert.equal(name.maxFileBytes, 24 * 1024 * 1024);
  assert.equal(name.location, 'home');
  assert.equal(name.depth, 'direct');
  assert.deepEqual(name.categories, categoriesForSearchKind('files'));
  assert.deepEqual(name.extensions, ['md']);
  assert.equal(name.modified, 'week');
  assert.equal(name.size, 'medium');
  assert.equal(name.includeHidden, true);
  assert.equal(name.respectIgnore, false);
  assert.equal(name.followSymlinks, true);
  assert.deepEqual(reconcileUnifiedSearchFiltersForScope(name, 'name'), name);
  assert.equal(content.caseSensitive, true);
  assert.equal(content.regex, true);
  assert.equal(content.maxFileBytes, 100 * 1024 * 1024);
  assert.equal(content.size, 'medium');
  assert.deepEqual(content.categories, categoriesForSearchKind('files'));
  assert.deepEqual(contentFolders.categories, []);
  assert.equal(contentLarge.size, 'any');
  assert.equal(contentHuge.size, 'any');
  assert.deepEqual(
    reconcileUnifiedSearchFiltersForScope(contentLarge, 'content'),
    contentLarge,
  );
  assert.deepEqual(folders.categories, ['directory']);
  assert.equal(folders.caseSensitive, false);
  assert.equal(folders.regex, false);
  assert.equal(folders.maxFileBytes, 24 * 1024 * 1024);

  const options = buildUnifiedSearchOptions(name);
  assert.equal(options.caseSensitive, false);
  assert.equal(options.regex, false);
  assert.equal(options.maxFileBytes, 24 * 1024 * 1024);

  const contentLargeOptions = buildUnifiedSearchOptions({
    ...filters,
    matchScope: 'content',
    size: 'large',
  });
  const allLargeOptions = buildUnifiedSearchOptions({
    ...filters,
    matchScope: 'all',
    size: 'large',
  });
  assert.equal(contentLargeOptions.minSize, null);
  assert.equal(contentLargeOptions.maxSize, null);
  assert.equal(allLargeOptions.minSize, 100 * 1024 * 1024);
  assert.equal(allLargeOptions.maxSize, (1024 ** 3) - 1);
});

test('allows empty-query searches only for direct children', () => {
  const defaults = createUnifiedSearchFilters();
  const direct = { ...defaults, depth: 'direct' };

  assert.equal(canRunUnifiedSearch('needle', defaults), true);
  assert.equal(canRunUnifiedSearch('   ', defaults), false);
  assert.equal(canRunUnifiedSearch('', direct), true);
  assert.equal(canRunUnifiedSearch('', {
    ...defaults,
    categories: categoriesForSearchKind('files'),
  }), false);
  assert.equal(canRunUnifiedSearch('', { ...defaults, extensions: ['docx'] }), false);
  assert.equal(canRunUnifiedSearch('', { ...defaults, modified: 'week' }), false);
  assert.equal(canRunUnifiedSearch('', { ...defaults, size: 'large' }), false);
  assert.equal(canRunUnifiedSearch('', {
    ...direct,
    matchScope: 'all',
    categories: categoriesForSearchKind('folders'),
  }), true);
  assert.equal(canRunUnifiedSearch('', { ...direct, extensions: ['docx'] }), true);
  assert.equal(canRunUnifiedSearch('', { ...direct, modified: 'week' }), true);
  assert.equal(canRunUnifiedSearch('', { ...direct, size: 'large' }), true);
  assert.equal(canRunUnifiedSearch('', {
    ...direct,
    matchScope: 'content',
    extensions: ['docx'],
  }), false);
  assert.equal(canRunUnifiedSearch('', { ...defaults, location: 'home' }), false);
  assert.equal(canRunUnifiedSearch('', { ...defaults, matchScope: 'all' }), false);
  assert.equal(canRunUnifiedSearch('', {
    ...defaults,
    includeHidden: true,
    respectIgnore: false,
    followSymlinks: true,
  }), false);
  assert.equal(canRunUnifiedSearch('', {
    ...defaults,
    extensions: normalizeSearchExtensions('bad/ext'),
  }), false);
});

test('size presets use non-overlapping inclusive backend boundaries', () => {
  const small = buildUnifiedSearchOptions(createUnifiedSearchFilters({ size: 'small' }));
  const medium = buildUnifiedSearchOptions(createUnifiedSearchFilters({ size: 'medium' }));
  const large = buildUnifiedSearchOptions(createUnifiedSearchFilters({ size: 'large' }));
  const huge = buildUnifiedSearchOptions(createUnifiedSearchFilters({ size: 'huge' }));

  assert.equal(small.maxSize + 1, medium.minSize);
  assert.equal(medium.maxSize + 1, large.minSize);
  assert.equal(large.maxSize + 1, huge.minSize);
});

test('content scope excludes directories and extension-only searches target files', () => {
  const content = buildUnifiedSearchOptions(createUnifiedSearchFilters({
    matchScope: 'content',
    categories: ['directory'],
  }));
  const extensions = buildUnifiedSearchOptions(createUnifiedSearchFilters({
    extensions: ['md'],
  }));

  assert.equal(content.includeDirectories, false);
  assert.equal(content.includeFiles, true);
  assert.deepEqual(content.categories, []);
  assert.equal(extensions.includeFiles, true);
  assert.equal(extensions.includeDirectories, false);
});

test('normalizes a unified response and drops malformed result rows', () => {
  const payload = normalizeUnifiedSearchPayload({
    results: [
      {
        name: 'notes.txt',
        path: '/tmp/notes.txt',
        kind: 'file',
        category: 'unknown',
        matchSource: 'both',
        matchIndices: [3, 1, 3, -1, '2'],
        size: 12,
      },
      { name: '', path: '/tmp/missing-name' },
      null,
    ],
    scannedEntries: 10.4,
    matchedEntries: 2,
    done: true,
  });

  assert.equal(payload.results.length, 1);
  assert.equal(payload.results[0].category, 'file');
  assert.equal(payload.results[0].matchSource, 'both');
  assert.deepEqual(payload.results[0].matchIndices, [1, 2, 3]);
  assert.equal(payload.scannedEntries, 10);
  assert.equal(payload.matchedEntries, 2);
  assert.equal(payload.done, true);
  assert.equal(normalizeUnifiedSearchPayload({ results: null }), null);
});

test('highlights Unicode content matches using character indices', () => {
  const segments = contentSnippetSegments({
    lineText: 'a🧳文z',
    matchStart: 1,
    matchEnd: 3,
  });

  assert.deepEqual(segments, [
    { text: 'a', match: false },
    { text: '🧳文', match: true },
    { text: 'z', match: false },
  ]);
});

test('formats compact file size and modified metadata', () => {
  assert.equal(formatSearchSize(1536), '1.5 KB');
  assert.equal(formatSearchSize(null), '');
  assert.equal(formatSearchModified(1_000, 1_960_000), '16m');
  assert.equal(formatSearchModified(null), '');
});
