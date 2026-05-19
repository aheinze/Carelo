import { isArchiveEntry } from './archivePaths';
import {
  extensionForName,
  isAudioEntry,
  isImageEntry,
  isPdfEntry,
  isVideoEntry,
} from './fileTypes';

const CODE_EXTENSIONS = new Set([
  'bash',
  'c',
  'cc',
  'cpp',
  'cs',
  'css',
  'go',
  'h',
  'hpp',
  'htm',
  'html',
  'java',
  'js',
  'jsx',
  'lua',
  'php',
  'pl',
  'py',
  'rb',
  'rs',
  'scss',
  'sh',
  'sql',
  'svelte',
  'swift',
  'ts',
  'tsx',
  'vue',
  'zsh',
]);

const CONFIG_EXTENSIONS = new Set([
  'cfg',
  'conf',
  'env',
  'gitignore',
  'ini',
  'json',
  'lock',
  'properties',
  'toml',
  'xml',
  'yaml',
  'yml',
]);

const CONFIG_NAMES = new Set([
  '.dockerignore',
  '.editorconfig',
  '.env',
  '.gitattributes',
  '.gitignore',
  'dockerfile',
  'makefile',
]);

const DOCUMENT_EXTENSIONS = new Set(['doc', 'docx', 'epub', 'md', 'odt', 'pages', 'rtf', 'txt']);
const SPREADSHEET_EXTENSIONS = new Set(['csv', 'ods', 'numbers', 'xls', 'xlsx']);
const PRESENTATION_EXTENSIONS = new Set(['key', 'odp', 'ppt', 'pptx']);

export function fileTypeIconKind(entry) {
  if (!entry || entry.kind !== 'file') {
    return entry?.kind === 'directory' ? 'directory' : 'file';
  }

  const name = String(entry.name || '').toLowerCase();
  const extension = extensionForName(entry.name);

  if (isArchiveEntry(entry)) return 'archive';
  if (isImageEntry(entry)) return 'image';
  if (isVideoEntry(entry)) return 'video';
  if (isAudioEntry(entry)) return 'audio';
  if (CONFIG_NAMES.has(name) || CONFIG_EXTENSIONS.has(extension)) return 'config';
  if (CODE_EXTENSIONS.has(extension)) return 'code';
  if (SPREADSHEET_EXTENSIONS.has(extension)) return 'spreadsheet';
  if (PRESENTATION_EXTENSIONS.has(extension)) return 'presentation';
  if (DOCUMENT_EXTENSIONS.has(extension) || isPdfEntry(entry)) return 'document';

  return 'file';
}

export function fileTypeIconName(entry) {
  const icons = {
    archive: 'archive',
    audio: 'music',
    code: 'file-code',
    config: 'file-config',
    directory: 'folder',
    document: 'file-text',
    image: 'image',
    spreadsheet: 'file-spreadsheet',
    presentation: 'file-presentation',
    video: 'video',
    file: 'file',
  };

  return icons[fileTypeIconKind(entry)] || 'file';
}
