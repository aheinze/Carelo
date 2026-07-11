import { isArchivePath } from './archivePaths.js';

export const UNIFIED_SEARCH_CATEGORIES = Object.freeze([
  'directory',
  'archive',
  'image',
  'video',
  'audio',
  'config',
  'code',
  'spreadsheet',
  'presentation',
  'document',
  'file',
]);
export const UNIFIED_SEARCH_FILE_CATEGORIES = Object.freeze(
  UNIFIED_SEARCH_CATEGORIES.filter((category) => category !== 'directory'),
);

const CATEGORY_SET = new Set(UNIFIED_SEARCH_CATEGORIES);
const MATCH_SCOPES = new Set(['all', 'name', 'content']);
const MATCH_SOURCES = new Set(['name', 'content', 'both']);
const LOCATIONS = new Set(['active', 'home']);
const DEPTHS = new Set(['direct', 'all']);
const MODIFIED_PRESETS = new Set(['any', 'today', 'week', 'month', 'year']);
const SIZE_PRESETS = new Set(['any', 'small', 'medium', 'large', 'huge']);
const DEFAULT_MAX_FILE_BYTES = 24 * 1024 * 1024;
const MAX_EXTENSION_COUNT = 32;
const EXTENSION_PATTERN = /^[a-z0-9][a-z0-9+_-]{0,31}(?:\.[a-z0-9][a-z0-9+_-]{0,31}){0,3}$/;

export function isUnifiedSearchRootSupported(root) {
  const value = String(root || '').trim();
  return Boolean(value) && !isArchivePath(value);
}

export function defaultUnifiedSearchLocation(activeRoot) {
  return isUnifiedSearchRootSupported(activeRoot) ? 'active' : 'home';
}

export function resolveUnifiedSearchRoot(activeRoot, location = 'active') {
  return location === 'home' ? '~' : String(activeRoot || '').trim();
}

export function categoriesForSearchKind(kind) {
  if (kind === 'folders') {
    return ['directory'];
  }

  if (kind === 'files') {
    return [...UNIFIED_SEARCH_FILE_CATEGORIES];
  }

  return [];
}

export function searchKindForCategories(categories) {
  const values = new Set(
    (Array.isArray(categories) ? categories : [])
      .map((category) => String(category || '').trim().toLowerCase())
      .filter((category) => CATEGORY_SET.has(category)),
  );

  if (values.size === 1 && values.has('directory')) {
    return 'folders';
  }

  if (
    values.size === UNIFIED_SEARCH_FILE_CATEGORIES.length
    && UNIFIED_SEARCH_FILE_CATEGORIES.every((category) => values.has(category))
  ) {
    return 'files';
  }

  return 'any';
}

function finiteNumber(value) {
  if (value === null || value === undefined || value === '') {
    return null;
  }

  const number = Number(value);
  return Number.isFinite(number) ? number : null;
}

function optionalNonNegativeInteger(value) {
  const number = finiteNumber(value);
  return number !== null && number >= 0 ? Math.round(number) : null;
}

export function parseSearchExtensions(value) {
  const values = Array.isArray(value)
    ? value
    : String(value || '').split(/[\s,;]+/);
  const extensions = [];
  const invalidTokens = [];
  const seen = new Set();

  values.forEach((valueToken) => {
    const token = String(valueToken || '').trim();

    if (!token) {
      return;
    }

    const extension = token
      .replace(/^\*+\./, '')
      .replace(/^\.+/, '')
      .toLowerCase();

    if (!EXTENSION_PATTERN.test(extension)) {
      invalidTokens.push(token);
      return;
    }

    if (!seen.has(extension)) {
      seen.add(extension);
      extensions.push(extension);
    }
  });

  return {
    extensions: extensions.slice(0, MAX_EXTENSION_COUNT),
    invalidTokens,
    truncatedCount: Math.max(0, extensions.length - MAX_EXTENSION_COUNT),
  };
}

export function normalizeSearchExtensions(value) {
  return parseSearchExtensions(value).extensions;
}

export function createUnifiedSearchFilters(overrides = {}) {
  return normalizeUnifiedSearchFilters({
    location: 'active',
    depth: 'all',
    matchScope: 'name',
    categories: [],
    extensions: [],
    modified: 'any',
    size: 'any',
    includeHidden: false,
    respectIgnore: true,
    followSymlinks: false,
    caseSensitive: false,
    regex: false,
    maxFileBytes: DEFAULT_MAX_FILE_BYTES,
    ...overrides,
  });
}

export function normalizeUnifiedSearchFilters(filters = {}) {
  const categories = (Array.isArray(filters.categories) ? filters.categories : [])
    .map((category) => String(category || '').trim().toLowerCase())
    .filter((category) => CATEGORY_SET.has(category));
  const maxFileBytes = optionalNonNegativeInteger(filters.maxFileBytes);

  return {
    location: LOCATIONS.has(filters.location) ? filters.location : 'active',
    depth: DEPTHS.has(filters.depth) ? filters.depth : 'all',
    matchScope: MATCH_SCOPES.has(filters.matchScope) ? filters.matchScope : 'name',
    categories: [...new Set(categories)],
    extensions: normalizeSearchExtensions(filters.extensions),
    modified: MODIFIED_PRESETS.has(filters.modified) ? filters.modified : 'any',
    size: SIZE_PRESETS.has(filters.size) ? filters.size : 'any',
    includeHidden: Boolean(filters.includeHidden),
    respectIgnore: filters.respectIgnore !== false,
    followSymlinks: Boolean(filters.followSymlinks),
    caseSensitive: Boolean(filters.caseSensitive),
    regex: Boolean(filters.regex),
    maxFileBytes: maxFileBytes && maxFileBytes > 0
      ? maxFileBytes
      : DEFAULT_MAX_FILE_BYTES,
  };
}

export function reconcileUnifiedSearchFiltersForScope(filters = {}, scope = filters.matchScope) {
  const normalized = normalizeUnifiedSearchFilters({
    ...filters,
    matchScope: scope,
  });
  const categories = normalized.matchScope === 'content'
    ? normalized.categories.filter((category) => category !== 'directory')
    : normalized.categories;
  const foldersOnly = searchKindForCategories(categories) === 'folders';
  const size = normalized.matchScope === 'content' && ['large', 'huge'].includes(normalized.size)
    ? 'any'
    : normalized.size;

  return {
    ...normalized,
    categories,
    size,
    caseSensitive: normalized.matchScope !== 'name' && !foldersOnly
      ? normalized.caseSensitive
      : false,
    regex: normalized.matchScope !== 'name' && !foldersOnly
      ? normalized.regex
      : false,
    maxFileBytes: normalized.matchScope !== 'name' && !foldersOnly
      ? normalized.maxFileBytes
      : DEFAULT_MAX_FILE_BYTES,
  };
}

export function canRunUnifiedSearch(query, filters = {}) {
  if (String(query || '').trim()) {
    return true;
  }

  const normalized = reconcileUnifiedSearchFiltersForScope(filters);

  if (normalized.matchScope === 'content') {
    return false;
  }

  return normalized.depth === 'direct';
}

function modifiedBounds(preset, now) {
  const current = new Date(now);
  const nowSeconds = Math.floor(current.getTime() / 1000);

  if (preset === 'today') {
    current.setHours(0, 0, 0, 0);
    return { modifiedAfter: Math.floor(current.getTime() / 1000), modifiedBefore: null };
  }

  const days = preset === 'week' ? 7 : preset === 'month' ? 30 : preset === 'year' ? 365 : 0;
  return {
    modifiedAfter: days > 0 ? nowSeconds - (days * 24 * 60 * 60) : null,
    modifiedBefore: null,
  };
}

function sizeBounds(preset) {
  const mebibyte = 1024 * 1024;
  const gibibyte = 1024 * mebibyte;

  switch (preset) {
    case 'small':
      return { minSize: null, maxSize: mebibyte - 1 };
    case 'medium':
      return { minSize: mebibyte, maxSize: (100 * mebibyte) - 1 };
    case 'large':
      return { minSize: 100 * mebibyte, maxSize: gibibyte - 1 };
    case 'huge':
      return { minSize: gibibyte, maxSize: null };
    default:
      return { minSize: null, maxSize: null };
  }
}

export function buildUnifiedSearchOptions(filters = {}, context = {}) {
  const normalized = reconcileUnifiedSearchFiltersForScope(filters);
  const categories = normalized.matchScope === 'content'
    ? normalized.categories.filter((category) => category !== 'directory')
    : normalized.categories;
  const modified = modifiedBounds(normalized.modified, context.now ?? Date.now());
  const size = sizeBounds(normalized.size);
  const hasDirectoryCategory = categories.includes('directory');
  const hasFileCategory = categories.some((category) => category !== 'directory');
  const hasCategories = categories.length > 0;
  const extensionsOnly = normalized.extensions.length > 0 && !hasDirectoryCategory;

  return {
    matchScope: normalized.matchScope,
    categories,
    extensions: normalized.extensions,
    ...size,
    ...modified,
    maxDepth: normalized.depth === 'direct' ? 1 : null,
    includeHidden: normalized.includeHidden,
    respectIgnore: normalized.respectIgnore,
    includeFiles: !hasCategories || hasFileCategory,
    includeDirectories: normalized.matchScope !== 'content'
      && !extensionsOnly
      && (!hasCategories || hasDirectoryCategory),
    followSymlinks: normalized.followSymlinks,
    caseSensitive: normalized.caseSensitive,
    regex: normalized.regex,
    maxFileBytes: normalized.maxFileBytes,
    limit: Math.max(1, Math.min(500, optionalNonNegativeInteger(context.limit) || 120)),
  };
}

function optionalString(value) {
  return typeof value === 'string' ? value : '';
}

function optionalNumber(value) {
  const number = finiteNumber(value);
  return number === null ? null : number;
}

export function normalizeUnifiedSearchResult(result) {
  if (!result || typeof result !== 'object' || Array.isArray(result)) {
    return null;
  }

  const name = optionalString(result.name).trim();
  const path = optionalString(result.path).trim();

  if (!name || !path) {
    return null;
  }

  const category = optionalString(result.category);
  const matchSource = optionalString(result.matchSource);

  return {
    name,
    path,
    parentPath: optionalString(result.parentPath),
    kind: optionalString(result.kind) || 'file',
    category: CATEGORY_SET.has(category)
      ? category
      : result.kind === 'directory' ? 'directory' : 'file',
    size: optionalNumber(result.size),
    modifiedAt: optionalNumber(result.modifiedAt),
    matchSource: MATCH_SOURCES.has(matchSource) ? matchSource : 'name',
    matchIndices: [...new Set(
      (Array.isArray(result.matchIndices) ? result.matchIndices : [])
        .map(optionalNonNegativeInteger)
        .filter((index) => index !== null),
    )].sort((left, right) => left - right),
    lineNumber: optionalNonNegativeInteger(result.lineNumber),
    lineText: optionalString(result.lineText),
    matchStart: optionalNonNegativeInteger(result.matchStart),
    matchEnd: optionalNonNegativeInteger(result.matchEnd),
    matchCount: optionalNonNegativeInteger(result.matchCount),
    score: optionalNumber(result.score) || 0,
  };
}

export function normalizeUnifiedSearchPayload(payload) {
  if (!payload || typeof payload !== 'object' || !Array.isArray(payload.results)) {
    return null;
  }

  return {
    results: payload.results.map(normalizeUnifiedSearchResult).filter(Boolean),
    scannedEntries: optionalNonNegativeInteger(payload.scannedEntries) || 0,
    matchedEntries: optionalNonNegativeInteger(payload.matchedEntries) || 0,
    done: Boolean(payload.done),
  };
}

export function contentSnippetSegments(result) {
  const characters = Array.from(optionalString(result?.lineText));
  const start = Math.max(0, Math.min(characters.length, optionalNonNegativeInteger(result?.matchStart) || 0));
  const requestedEnd = optionalNonNegativeInteger(result?.matchEnd);
  const end = Math.max(start, Math.min(characters.length, requestedEnd ?? start));

  return [
    { text: characters.slice(0, start).join(''), match: false },
    { text: characters.slice(start, end).join(''), match: end > start },
    { text: characters.slice(end).join(''), match: false },
  ].filter((segment) => segment.text);
}

export function formatSearchSize(value) {
  const bytes = optionalNumber(value);

  if (bytes === null || bytes < 0) return '';
  if (bytes >= 1024 ** 3) return `${(bytes / 1024 ** 3).toFixed(bytes >= 10 * 1024 ** 3 ? 0 : 1)} GB`;
  if (bytes >= 1024 ** 2) return `${(bytes / 1024 ** 2).toFixed(bytes >= 10 * 1024 ** 2 ? 0 : 1)} MB`;
  if (bytes >= 1024) return `${(bytes / 1024).toFixed(bytes >= 10 * 1024 ? 0 : 1)} KB`;
  return `${Math.round(bytes)} B`;
}

export function formatSearchModified(value, now = Date.now()) {
  const seconds = optionalNumber(value);

  if (seconds === null || seconds < 0) return '';
  const elapsedSeconds = Math.max(0, Math.floor(now / 1000) - seconds);
  if (elapsedSeconds < 60) return 'now';
  if (elapsedSeconds < 60 * 60) return `${Math.floor(elapsedSeconds / 60)}m`;
  if (elapsedSeconds < 24 * 60 * 60) return `${Math.floor(elapsedSeconds / (60 * 60))}h`;
  if (elapsedSeconds < 7 * 24 * 60 * 60) return `${Math.floor(elapsedSeconds / (24 * 60 * 60))}d`;

  return new Intl.DateTimeFormat(undefined, { month: 'short', day: 'numeric' })
    .format(new Date(seconds * 1000));
}
