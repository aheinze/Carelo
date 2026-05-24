const ARCHIVE_PREFIX = 'archive://';
const ARCHIVE_SEPARATOR = '!/';

const ARCHIVE_EXTENSIONS = [
  '.zip',
  '.tar',
  '.tar.gz',
  '.tgz',
  '.tar.zst',
  '.tzst',
  '.7z',
];

export function isArchivePath(path) {
  return String(path || '').startsWith(ARCHIVE_PREFIX);
}

export function isArchiveName(name) {
  const value = String(name || '').toLowerCase();

  return ARCHIVE_EXTENSIONS.some((extension) => value.endsWith(extension));
}

export function isArchiveEntry(entry) {
  return entry?.kind === 'file' && isArchiveName(entry.name) && !isArchivePath(entry.path);
}

export function isBrowsableEntry(entry) {
  return entry?.kind === 'directory' || isArchiveEntry(entry);
}

export function archiveRootPath(path) {
  return `${ARCHIVE_PREFIX}${encodeArchiveContainerPath(path)}${ARCHIVE_SEPARATOR}`;
}

export function parseArchivePath(path) {
  const value = String(path || '');

  if (!value.startsWith(ARCHIVE_PREFIX)) {
    return null;
  }

  const rest = value.slice(ARCHIVE_PREFIX.length);
  const separatorIndex = rest.indexOf(ARCHIVE_SEPARATOR);

  if (separatorIndex < 0) {
    return null;
  }

  const archivePath = decodeArchiveContainerPath(rest.slice(0, separatorIndex));
  const innerPath = normalizeArchiveInnerPath(rest.slice(separatorIndex + ARCHIVE_SEPARATOR.length));

  return {
    archivePath,
    innerPath,
    rootPath: `${ARCHIVE_PREFIX}${rest.slice(0, separatorIndex)}${ARCHIVE_SEPARATOR}`,
  };
}

export function archiveDisplayName(path) {
  const parsed = parseArchivePath(path);

  if (!parsed) {
    return '';
  }

  return fileNameForPath(parsed.archivePath);
}

export function archiveParentPath(path) {
  const parsed = parseArchivePath(path);

  if (!parsed) {
    return '';
  }

  if (!parsed.innerPath) {
    return parentLocalPath(parsed.archivePath);
  }

  const parts = parsed.innerPath.split('/').filter(Boolean);

  if (parts.length <= 1) {
    return parsed.rootPath;
  }

  return `${parsed.rootPath}${parts.slice(0, -1).join('/')}`;
}

export function archiveBreadcrumbs(path) {
  const parsed = parseArchivePath(path);

  if (!parsed) {
    return [];
  }

  const crumbs = [
    {
      label: fileNameForPath(parsed.archivePath),
      path: parsed.rootPath,
    },
  ];
  const innerParts = parsed.innerPath.split('/').filter(Boolean);

  innerParts.forEach((part, index) => {
    crumbs.push({
      label: part,
      path: `${parsed.rootPath}${innerParts.slice(0, index + 1).join('/')}`,
    });
  });

  return crumbs;
}

export function joinArchiveAwarePath(directory, name) {
  const base = String(directory || '').replace(/\/+$/, '');

  if (isArchivePath(directory)) {
    return directory.endsWith(ARCHIVE_SEPARATOR)
      ? `${directory}${name}`
      : `${base}/${name}`;
  }

  if (!base || base === '/') {
    return `/${name}`;
  }

  return `${base}/${name}`;
}

function encodeArchiveContainerPath(path) {
  return Array.from(new TextEncoder().encode(String(path || '')))
    .map((byte) => {
      const character = String.fromCharCode(byte);

      return /[A-Za-z0-9_.~/-]/.test(character)
        ? character
        : `%${byte.toString(16).toUpperCase().padStart(2, '0')}`;
    })
    .join('');
}

function decodeArchiveContainerPath(path) {
  try {
    return decodeURIComponent(path);
  } catch {
    return path;
  }
}

function normalizeArchiveInnerPath(path) {
  return String(path || '')
    .replace(/\\/g, '/')
    .split('/')
    .filter((part) => part && part !== '.')
    .join('/');
}

function fileNameForPath(path) {
  const value = String(path || '').replace(/\/+$/, '');
  return value.split('/').filter(Boolean).at(-1) || value || 'Archive';
}

function parentLocalPath(path) {
  const cleanPath = String(path || '').replace(/\/+$/, '');

  if (!cleanPath || cleanPath === '/' || cleanPath === '~') {
    return cleanPath || '~';
  }

  const index = cleanPath.lastIndexOf('/');
  return index <= 0 ? '/' : cleanPath.slice(0, index);
}
