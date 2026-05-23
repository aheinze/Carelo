<script setup>
import { computed, nextTick, ref, watch } from 'vue';
import AppIcon from './AppIcon.vue';
import { canUseLocalFileAssets, searchContent, searchFiles } from '../composables/useFileOperations';
import { useFileManagerStore } from '../stores/fileManagerStore';
import { isArchivePath } from '../utils/archivePaths';

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
let searchTimer = null;
let searchVersion = 0;

function pluralize(count, singular, plural = `${singular}s`) {
  return `${count} ${count === 1 ? singular : plural}`;
}

const contentMatchedLineCount = computed(() => (
  results.value.reduce((total, result) => total + Math.max(Number(result.matchCount) || 1, 1), 0)
));
const currentMode = computed(() => store.fileSearchMode || 'files');
const activeRoot = computed(() => (
  store.effectiveDirectoryFor(store.activePaneId) || store.activeTabFor(store.activePaneId)?.currentPath || '~'
));
const canSearchRoot = computed(() => {
  const root = activeRoot.value;
  return canUseLocalFileAssets()
    && root
    && !isArchivePath(root);
});
const statusText = computed(() => {
  if (!canSearchRoot.value) {
    return 'Search unavailable';
  }

  if (!query.value.trim()) {
    return 'Ready';
  }

  if (loading.value) {
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
  currentMode.value === 'content'
    ? 'Search file contents in current folder'
    : 'Search files in current folder'
));
const emptyPlaceholder = computed(() => (
  currentMode.value === 'content'
    ? 'Type to search inside files in the current folder'
    : 'Type to fuzzy search the current folder'
));
const dialogLabel = computed(() => (
  currentMode.value === 'content' ? 'Content search' : 'Fuzzy file search'
));

function setMode(mode) {
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
  results.value = [];
  selectedIndex.value = 0;
  error.value = '';
  loading.value = false;
  searchVersion += 1;
  clearTimeout(searchTimer);
}

async function runSearch() {
  const version = ++searchVersion;

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

  try {
    const nextResults = currentMode.value === 'content'
      ? await searchContent(activeRoot.value, query.value, {
          limit: 120,
          includeHidden: store.showHiddenFiles,
          respectIgnore: true,
          caseSensitive: false,
          regex: false,
          maxFileBytes: CONTENT_SEARCH_MAX_FILE_BYTES,
        })
      : await searchFiles(activeRoot.value, query.value, {
          limit: SEARCH_LIMIT,
          includeHidden: store.showHiddenFiles,
          respectIgnore: true,
          includeFiles: true,
          includeDirectories: true,
          followSymlinks: false,
        });

    if (version !== searchVersion) {
      return;
    }

    results.value = Array.isArray(nextResults) ? nextResults : [];
    selectedIndex.value = Math.min(selectedIndex.value, Math.max(results.value.length - 1, 0));
  } catch (searchError) {
    if (version !== searchVersion) {
      return;
    }

    results.value = [];
    selectedIndex.value = 0;
    error.value = searchError?.message || 'Unable to search this folder.';
  } finally {
    if (version === searchVersion) {
      loading.value = false;
    }
  }
}

function scheduleSearch() {
  clearTimeout(searchTimer);
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
  }
}

function resultIcon(result) {
  return result?.kind === 'directory' ? 'folder' : 'file';
}

function resultKey(result) {
  return result.path;
}

function resultTitle(result) {
  return result.name;
}

function contentResultMeta(result) {
  const lineNumber = Math.max(Number(result?.lineNumber) || 1, 1);
  const matchCount = Math.max(Number(result?.matchCount) || 1, 1);
  const lineCountText = matchCount === 1 ? '1 line' : `${matchCount} lines`;

  return `Line ${lineNumber} / ${lineCountText}`;
}

function resultDetail(result) {
  return currentMode.value === 'content'
    ? result.lineText
    : result.parentPath;
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

watch(results, () => {
  resultButtons.value = [];
});

watch(selectedIndex, () => {
  if (store.fileSearchVisible) {
    scrollSelectedResultIntoView();
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
        <section class="command-palette" role="dialog" aria-modal="true" :aria-label="dialogLabel">
          <header class="command-palette__header">
            <div class="command-palette__title-group">
              <div class="command-palette__title-text">
                <h2>{{ dialogLabel }}</h2>
                <span class="command-palette__root" :title="activeRoot">{{ activeRoot }}</span>
              </div>
            </div>

            <button
              type="button"
              class="command-palette__close"
              aria-label="Close search"
              @click="close"
            >
              <AppIcon name="x" :size="14" :stroke-width="2" />
            </button>
          </header>

          <div class="command-palette__modes" role="tablist" aria-label="Search mode">
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
                  <span class="command-palette__name">{{ resultTitle(result) }}</span>
                  <span
                    v-if="currentMode === 'content'"
                    class="command-palette__match-meta"
                  >{{ contentResultMeta(result) }}</span>
                </span>
                <span
                  v-if="currentMode === 'content' && result.lineText"
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
                v-if="index === selectedIndex"
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
              v-else-if="!query.trim()"
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
  backdrop-filter: blur(14px) saturate(1.1);
  -webkit-backdrop-filter: blur(14px) saturate(1.1);
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
  border-bottom: 1px solid var(--hairline);
  flex-shrink: 0;
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
  min-width: 0;
  color: var(--text);
  font-size: 13px;
  font-weight: 650;
  line-height: 1.15;
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

.command-palette__enter-hint kbd {
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
  border-top: 1px solid var(--hairline);
  background: color-mix(in srgb, var(--text) 2%, transparent);
  flex-shrink: 0;
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
