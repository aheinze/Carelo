<script setup>
import { computed, nextTick, onBeforeUnmount, onMounted, ref, shallowRef, watch } from 'vue';
import * as pdfjs from 'pdfjs-dist/legacy/build/pdf.mjs';
import pdfWorkerUrl from 'pdfjs-dist/legacy/build/pdf.worker.mjs?url';
import AppIcon from './AppIcon.vue';
import {
  localFileAssetUrl,
  readMediaPreview,
} from '../composables/useFileOperations';

pdfjs.GlobalWorkerOptions.workerSrc = pdfWorkerUrl;

const props = defineProps({
  entry: {
    type: Object,
    required: true,
  },
});

const pdfCanvas = ref(null);
const pdfTextLayer = ref(null);
const pdfPageShell = ref(null);
const pdfStage = ref(null);
const pdfSearchInput = ref(null);
const pdfDocument = shallowRef(null);
const pageNumber = ref(1);
const pageCount = ref(0);
const scale = ref(0.68);
const fitMode = ref(true);
const loading = ref(false);
const errorMessage = ref('');
const searchQuery = ref('');
const searchVisible = ref(false);
const searchMatches = ref([]);
const activeMatchIndex = ref(-1);
const searchLoading = ref(false);

const pdfPreviewMaxBytes = 128 * 1024 * 1024;
const minScale = 0.3;
const maxScale = 2.4;
const scaleStep = 0.12;
let loadVersion = 0;
let renderVersion = 0;
let searchVersion = 0;
let renderTask = null;
let searchTimer = null;
let resizeObserver = null;
let destroyed = false;

const scaleLabel = computed(() => `${Math.round(scale.value * 100)}%`);
const matchLabel = computed(() => {
  if (searchLoading.value) return '...';
  if (!searchQuery.value.trim()) return '';
  if (searchMatches.value.length === 0) return '0/0';
  return `${activeMatchIndex.value + 1}/${searchMatches.value.length}`;
});
const hasPreviousPage = computed(() => pageNumber.value > 1);
const hasNextPage = computed(() => pageNumber.value < pageCount.value);
const hasMatches = computed(() => searchMatches.value.length > 0);

watch(
  () => props.entry?.path,
  () => {
    loadPdf();
  },
  { immediate: true },
);

watch(pageNumber, () => {
  if (fitMode.value) {
    fitToWidth();
    return;
  }

  renderCurrentPage();
});

watch(scale, () => {
  renderCurrentPage();
});

onMounted(() => {
  if (typeof ResizeObserver === 'undefined') {
    return;
  }

  resizeObserver = new ResizeObserver(() => {
    if (fitMode.value) {
      fitToWidth();
    }
  });

  nextTick(() => {
    if (pdfStage.value) {
      resizeObserver?.observe(pdfStage.value);
    }
  });
});

onBeforeUnmount(() => {
  destroyed = true;
  clearSearchTimer();
  cancelRenderTask();
  resizeObserver?.disconnect();
  pdfDocument.value?.destroy?.();
});

function mediaPreviewPayloadToBytes(payload) {
  if (payload instanceof ArrayBuffer) {
    return new Uint8Array(payload);
  }

  if (ArrayBuffer.isView(payload)) {
    return new Uint8Array(payload.buffer, payload.byteOffset, payload.byteLength);
  }

  if (Array.isArray(payload)) {
    return new Uint8Array(payload);
  }

  throw new Error('Unexpected PDF preview payload.');
}

async function pdfDocumentFromBytes(entry) {
  const payload = await readMediaPreview(entry.path, pdfPreviewMaxBytes);
  const bytes = mediaPreviewPayloadToBytes(payload);

  return pdfjs.getDocument({
    data: bytes,
    useSystemFonts: true,
  }).promise;
}

async function pdfDocumentFromUrl(entry) {
  const url = localFileAssetUrl(entry.path);

  if (!url) {
    throw new Error('PDF preview URL unavailable.');
  }

  return pdfjs.getDocument({
    url,
    useSystemFonts: true,
  }).promise;
}

async function loadPdf() {
  loadVersion += 1;
  const version = loadVersion;
  const entry = props.entry;
  loading.value = true;
  errorMessage.value = '';
  pageNumber.value = 1;
  pageCount.value = 0;
  searchQuery.value = '';
  searchVisible.value = false;
  searchMatches.value = [];
  activeMatchIndex.value = -1;
  cancelRenderTask();

  const previousDocument = pdfDocument.value;
  pdfDocument.value = null;
  previousDocument?.destroy?.();

  if (!entry?.path) {
    loading.value = false;
    return;
  }

  try {
    let document = null;

    try {
      document = await pdfDocumentFromUrl(entry);
    } catch {
      document = await pdfDocumentFromBytes(entry);
    }

    if (destroyed || version !== loadVersion) {
      document?.destroy?.();
      return;
    }

    pdfDocument.value = document;
    pageCount.value = document.numPages || 0;
    loading.value = false;
    await nextTick();
    fitMode.value = true;
    await fitToWidth();
  } catch (error) {
    if (destroyed || version !== loadVersion) {
      return;
    }

    errorMessage.value = error?.message || 'PDF preview unavailable.';
    loading.value = false;
  }
}

function cancelRenderTask() {
  if (renderTask) {
    renderTask.cancel?.();
    renderTask = null;
  }
}

async function renderCurrentPage() {
  const document = pdfDocument.value;
  const canvas = pdfCanvas.value;
  const textLayer = pdfTextLayer.value;
  const shell = pdfPageShell.value;

  if (!document || !canvas || !textLayer || !shell || loading.value) {
    return;
  }

  renderVersion += 1;
  const version = renderVersion;
  cancelRenderTask();

  try {
    const page = await document.getPage(pageNumber.value);

    if (version !== renderVersion) {
      return;
    }

    const viewport = page.getViewport({ scale: scale.value });
    const outputScale = window.devicePixelRatio || 1;
    const context = canvas.getContext('2d');

    canvas.width = Math.floor(viewport.width * outputScale);
    canvas.height = Math.floor(viewport.height * outputScale);
    canvas.style.width = `${viewport.width}px`;
    canvas.style.height = `${viewport.height}px`;
    shell.style.width = `${viewport.width}px`;
    shell.style.height = `${viewport.height}px`;
    textLayer.style.width = `${viewport.width}px`;
    textLayer.style.height = `${viewport.height}px`;
    textLayer.innerHTML = '';

    renderTask = page.render({
      canvasContext: context,
      viewport,
      transform: outputScale === 1 ? null : [outputScale, 0, 0, outputScale, 0, 0],
    });

    await renderTask.promise;

    if (version !== renderVersion) {
      return;
    }

    renderTask = null;
    await renderTextLayer(page, viewport, textLayer);
    markSearchText();
  } catch (error) {
    if (error?.name !== 'RenderingCancelledException') {
      errorMessage.value = error?.message || 'PDF page render failed.';
    }
  }
}

async function renderTextLayer(page, viewport, container) {
  try {
    const textContent = await page.getTextContent();
    const textLayer = new pdfjs.TextLayer({
      textContentSource: textContent,
      container,
      viewport,
    });

    await textLayer.render();
  } catch {
    container.innerHTML = '';
  }
}

function markSearchText() {
  const query = searchQuery.value.trim().toLowerCase();
  const textLayer = pdfTextLayer.value;

  if (!query || !textLayer) {
    return;
  }

  const currentPageHasMatch = searchMatches.value.some((match) => match.page === pageNumber.value);

  if (!currentPageHasMatch) {
    return;
  }

  textLayer.querySelectorAll('span').forEach((span) => {
    if (String(span.textContent || '').toLowerCase().includes(query)) {
      span.classList.add('pdf-text-highlight');
    }
  });
}

async function fitToWidth() {
  const document = pdfDocument.value;
  const stage = pdfStage.value;

  if (!document || !stage || pageCount.value === 0) {
    return;
  }

  try {
    const page = await document.getPage(pageNumber.value);
    const viewport = page.getViewport({ scale: 1 });
    const availableWidth = Math.max(160, stage.clientWidth - 30);
    const nextScale = Math.min(maxScale, Math.max(minScale, availableWidth / viewport.width));
    const roundedScale = Number(nextScale.toFixed(2));

    if (Math.abs(scale.value - roundedScale) > 0.005) {
      scale.value = roundedScale;
    } else {
      renderCurrentPage();
    }
  } catch (error) {
    errorMessage.value = error?.message || 'PDF fit failed.';
  }
}

function goToPage(nextPage) {
  const numericPage = Number(nextPage);

  if (!Number.isFinite(numericPage) || pageCount.value === 0) {
    return;
  }

  pageNumber.value = Math.min(pageCount.value, Math.max(1, Math.round(numericPage)));
}

function previousPage() {
  if (hasPreviousPage.value) {
    goToPage(pageNumber.value - 1);
  }
}

function nextPage() {
  if (hasNextPage.value) {
    goToPage(pageNumber.value + 1);
  }
}

function zoomOut() {
  fitMode.value = false;
  scale.value = Math.max(minScale, Number((scale.value - scaleStep).toFixed(2)));
}

function zoomIn() {
  fitMode.value = false;
  scale.value = Math.min(maxScale, Number((scale.value + scaleStep).toFixed(2)));
}

function toggleSearch() {
  searchVisible.value = !searchVisible.value;

  if (searchVisible.value) {
    nextTick(() => {
      pdfSearchInput.value?.focus();
      pdfSearchInput.value?.select?.();
    });
  }
}

function closeSearch() {
  searchVisible.value = false;
}

function clearSearchTimer() {
  if (searchTimer) {
    clearTimeout(searchTimer);
    searchTimer = null;
  }
}

function scheduleSearch() {
  clearSearchTimer();
  searchTimer = setTimeout(runSearch, 240);
}

async function runSearch() {
  clearSearchTimer();
  searchVersion += 1;
  const version = searchVersion;
  const query = searchQuery.value.trim().toLowerCase();
  const document = pdfDocument.value;
  searchMatches.value = [];
  activeMatchIndex.value = -1;

  if (!document || !query) {
    renderCurrentPage();
    return;
  }

  searchLoading.value = true;

  try {
    const matches = [];

    for (let page = 1; page <= document.numPages; page += 1) {
      if (version !== searchVersion) {
        return;
      }

      const pdfPage = await document.getPage(page);
      const content = await pdfPage.getTextContent();
      const text = content.items.map((item) => item.str || '').join(' ').toLowerCase();
      let index = text.indexOf(query);

      while (index >= 0) {
        matches.push({ page, index });
        index = text.indexOf(query, index + query.length);
      }
    }

    if (version !== searchVersion) {
      return;
    }

    searchMatches.value = matches;
    activeMatchIndex.value = matches.length > 0 ? 0 : -1;

    if (matches.length > 0) {
      const nextPage = matches[0].page;
      const pageChanged = nextPage !== pageNumber.value;
      goToPage(nextPage);

      if (!pageChanged) {
        renderCurrentPage();
      }
    } else {
      renderCurrentPage();
    }
  } finally {
    if (version === searchVersion) {
      searchLoading.value = false;
    }
  }
}

function goToMatch(offset) {
  if (!hasMatches.value) {
    return;
  }

  const count = searchMatches.value.length;
  activeMatchIndex.value = (activeMatchIndex.value + offset + count) % count;
  const nextPage = searchMatches.value[activeMatchIndex.value].page;
  const pageChanged = nextPage !== pageNumber.value;
  goToPage(nextPage);

  if (!pageChanged) {
    renderCurrentPage();
  }
}
</script>

<template>
  <section class="pdf-preview">
    <header v-if="!errorMessage" class="pdf-toolbar">
      <div class="pdf-toolbar-group">
        <button type="button" class="pdf-tool-button" :disabled="!hasPreviousPage" @click="previousPage">
          <AppIcon name="chevron-left" :size="15" :stroke-width="2.2" />
        </button>
        <button type="button" class="pdf-page-chip" :disabled="loading" @click="fitMode = true; fitToWidth()">
          {{ pageNumber }} / {{ pageCount || '-' }}
        </button>
        <button type="button" class="pdf-tool-button" :disabled="!hasNextPage" @click="nextPage">
          <AppIcon name="chevron-right" :size="15" :stroke-width="2.2" />
        </button>
      </div>

      <div class="pdf-toolbar-group">
        <button type="button" class="pdf-tool-button" :disabled="loading || scale <= minScale" @click="zoomOut">
          <AppIcon name="minus" :size="14" :stroke-width="2.2" />
        </button>
        <button type="button" class="pdf-zoom-chip" :disabled="loading" @click="fitMode = true; fitToWidth()">
          {{ scaleLabel }}
        </button>
        <button type="button" class="pdf-tool-button" :disabled="loading || scale >= maxScale" @click="zoomIn">
          <AppIcon name="plus" :size="14" :stroke-width="2.2" />
        </button>
        <span class="pdf-toolbar-divider" aria-hidden="true"></span>
        <button type="button" class="pdf-tool-button" :class="{ 'pdf-tool-button--active': searchVisible }" :disabled="loading" @click="toggleSearch">
          <AppIcon name="search" :size="14" :stroke-width="2" />
        </button>
      </div>
    </header>

    <form
      v-if="searchVisible"
      class="pdf-search-popover"
      @submit.prevent="runSearch"
      @keydown.escape.stop.prevent="closeSearch"
    >
      <AppIcon name="search" :size="14" :stroke-width="2" />
      <input
        ref="pdfSearchInput"
        v-model="searchQuery"
        type="search"
        placeholder="Find"
        @input="scheduleSearch"
      />
      <span v-if="matchLabel" class="pdf-match-count">{{ matchLabel }}</span>
      <button type="button" class="pdf-tool-button" :disabled="!hasMatches" @click="goToMatch(-1)">
        <AppIcon name="chevron-left" :size="14" :stroke-width="2.2" />
      </button>
      <button type="button" class="pdf-tool-button" :disabled="!hasMatches" @click="goToMatch(1)">
        <AppIcon name="chevron-right" :size="14" :stroke-width="2.2" />
      </button>
    </form>

    <div ref="pdfStage" class="pdf-stage">
      <span v-if="loading" class="pdf-status">Loading PDF...</span>
      <span v-else-if="errorMessage" class="pdf-status">{{ errorMessage }}</span>
      <div v-show="!loading && !errorMessage" ref="pdfPageShell" class="pdf-page-shell">
        <canvas ref="pdfCanvas" class="pdf-page-canvas"></canvas>
        <div ref="pdfTextLayer" class="pdf-text-layer"></div>
      </div>
    </div>
  </section>
</template>

<style scoped>
.pdf-preview {
  position: relative;
  display: flex;
  flex-direction: column;
  width: 100%;
  height: 100%;
  min-height: 0;
  overflow: hidden;
  border-radius: 7px;
  background:
    linear-gradient(180deg, color-mix(in srgb, var(--text) 5%, transparent), transparent),
    color-mix(in srgb, var(--text) 7%, transparent);
}

/* ── Fixed toolbar docked to the top of the card ──────────── */
.pdf-toolbar {
  position: relative;
  z-index: 4;
  display: flex;
  flex: 0 0 auto;
  align-items: center;
  justify-content: space-between;
  gap: 8px;
  padding: 5px 7px;
  border-bottom: 1px solid var(--hairline);
  background: var(--toolbar-bg);
  box-shadow: 0 1px 0 var(--separator);
}

.pdf-toolbar-group {
  display: inline-flex;
  min-width: 0;
  align-items: center;
  gap: 2px;
}

.pdf-toolbar-divider {
  flex: 0 0 auto;
  width: 1px;
  height: 16px;
  margin: 0 4px;
  background: var(--hairline);
}

.pdf-tool-button {
  display: grid;
  width: 24px;
  height: 24px;
  flex: 0 0 auto;
  place-items: center;
  border-radius: 7px;
  background: transparent;
  color: var(--icon);
  transition: background 100ms ease, color 100ms ease, opacity 100ms ease;
}

.pdf-tool-button:hover:not(:disabled) {
  background: var(--btn-hover);
  color: var(--text);
}

.pdf-tool-button:disabled {
  opacity: 0.38;
}

.pdf-tool-button--active {
  background: var(--accent-dim);
  color: var(--accent);
}

.pdf-page-chip,
.pdf-zoom-chip,
.pdf-match-count {
  display: inline-flex;
  align-items: center;
  height: 24px;
  border-radius: 7px;
  padding: 0 7px;
  background: transparent;
  color: var(--text-muted);
  font-size: 11.5px;
  font-weight: 650;
  white-space: nowrap;
}

.pdf-page-chip:hover:not(:disabled),
.pdf-zoom-chip:hover:not(:disabled) {
  background: var(--btn-hover);
  color: var(--text);
}

.pdf-page-chip:disabled,
.pdf-zoom-chip:disabled {
  opacity: 0.38;
  pointer-events: none;
}

.pdf-match-count {
  padding-inline: 3px;
  color: var(--text-faint);
}

.pdf-stage {
  position: relative;
  display: grid;
  flex: 1 1 auto;
  width: 100%;
  min-width: 0;
  min-height: 0;
  overflow: auto;
  place-items: start center;
  padding: 16px 15px;
  scrollbar-gutter: stable;
}

.pdf-search-popover {
  position: absolute;
  z-index: 5;
  top: 44px;
  right: 8px;
  display: inline-flex;
  min-width: 0;
  width: min(250px, calc(100% - 16px));
  align-items: center;
  gap: 5px;
  padding: 3px 5px;
  border: 1px solid var(--control-border);
  border-radius: 9px;
  background:
    linear-gradient(180deg, rgb(255 255 255 / 0.06), rgb(255 255 255 / 0.015)),
    color-mix(in srgb, var(--control-glass) 96%, transparent);
  color: var(--icon);
  box-shadow:
    var(--control-inset),
    0 12px 28px rgb(0 0 0 / 0.34);
  backdrop-filter: blur(20px) saturate(1.3);
  -webkit-backdrop-filter: blur(20px) saturate(1.3);
}

.pdf-search-popover input {
  min-width: 0;
  flex: 1 1 auto;
  border: 0;
  background: transparent;
  color: var(--text);
  font-size: 11.5px;
  outline: 0;
}

.pdf-search-popover input::placeholder {
  color: var(--text-faint);
}

.pdf-page-shell {
  position: relative;
  flex: 0 0 auto;
  overflow: hidden;
  border-radius: 4px;
  box-shadow:
    0 0 0 1px rgb(0 0 0 / 0.20),
    0 2px 6px rgb(0 0 0 / 0.18),
    0 16px 36px rgb(0 0 0 / 0.32);
}

.pdf-page-canvas {
  display: block;
  background: white;
}

.pdf-text-layer {
  position: absolute;
  inset: 0;
  overflow: hidden;
  color: transparent;
  line-height: 1;
  text-align: initial;
  transform-origin: 0 0;
}

.pdf-text-layer :deep(span) {
  position: absolute;
  color: transparent;
  cursor: text;
  transform-origin: 0 0;
  white-space: pre;
}

.pdf-text-layer :deep(.pdf-text-highlight) {
  border-radius: 2px;
  background: rgb(var(--accent-rgb) / 0.28);
  color: transparent;
}

.pdf-status {
  place-self: center;
  border: 1px solid var(--control-border);
  border-radius: 9px;
  padding: 8px 12px;
  background: var(--popover-bg);
  color: var(--text-muted);
  font-size: 11.5px;
  font-weight: 650;
  box-shadow: var(--shadow-overlay);
}

@media (max-width: 760px) {
  .pdf-zoom-chip {
    display: none;
  }
}
</style>
