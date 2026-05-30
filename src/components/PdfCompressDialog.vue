<script setup>
import { computed, nextTick, onBeforeUnmount, onMounted, ref, watch } from 'vue';
import AppIcon from './AppIcon.vue';
import { extensionForName } from '../utils/fileTypes';

const DESKTOP_PREVIEW_ROW_HEIGHT = 34;
const MOBILE_PREVIEW_ROW_HEIGHT = 72;
const PREVIEW_OVERSCAN_ROWS = 12;

const PROFILES = [
  { value: 'balanced', label: 'Balanced', detail: 'Shareable', icon: 'archive' },
  { value: 'smallest', label: 'Smallest', detail: 'Screen', icon: 'download' },
  { value: 'print', label: 'Print', detail: 'Sharper', icon: 'file' },
  { value: 'prepress', label: 'Prepress', detail: 'Maximum', icon: 'shield' },
];

const CONFLICT_POLICIES = [
  { value: 'keepBoth', label: 'Keep Both', icon: 'copy' },
  { value: 'skip', label: 'Skip', icon: 'minus' },
  { value: 'replace', label: 'Replace', icon: 'refresh' },
];

const props = defineProps({
  visible: {
    type: Boolean,
    default: false,
  },
  entries: {
    type: Array,
    default: () => [],
  },
  existingNamesByDirectory: {
    type: Object,
    default: () => ({}),
  },
  otherPaneDirectory: {
    type: String,
    default: '',
  },
});

const emit = defineEmits(['cancel', 'compress']);

const panelRef = ref(null);
const previewListRef = ref(null);
const previewScrollTop = ref(0);
const previewViewportHeight = ref(0);
const previewRowHeight = ref(DESKTOP_PREVIEW_ROW_HEIGHT);
const profile = ref('balanced');
const conflictPolicy = ref('keepBoth');
const destinationMode = ref('sameFolder');
const keepOnlySmaller = ref(false);
let previewResizeObserver = null;

const selectedLabel = computed(() => {
  const count = props.entries.length;
  return `${count} ${count === 1 ? 'PDF' : 'PDFs'} selected`;
});
const selectedSize = computed(() => {
  const sizes = props.entries.map((entry) => Number(entry?.size));

  if (sizes.length === 0 || sizes.some((size) => !Number.isFinite(size))) {
    return '';
  }

  return formatBytes(sizes.reduce((total, size) => total + size, 0));
});
const canUseOtherPane = computed(() => Boolean(props.otherPaneDirectory));
const sourceDirectory = computed(() => {
  const directories = new Set(
    props.entries.map((entry) => entry?.directory || parentDirectoryForPath(entry?.path)),
  );

  return directories.size === 1 ? [...directories][0] : '';
});
const destinationLabel = computed(() => {
  const target = destinationMode.value === 'otherPane' && props.otherPaneDirectory
    ? props.otherPaneDirectory
    : sourceDirectory.value;

  return target ? directoryName(target) : 'Same folder';
});
const previewRows = computed(() => {
  const plannedByDirectory = new Map();

  return props.entries.map((entry, index) => {
    const directory = outputDirectoryFor(entry);
    const seedName = outputNameFor(entry?.name || `Document ${index + 1}`);
    const existingNames = existingNameSet(directory);
    const plannedNames = plannedByDirectory.get(directory) || new Set();
    plannedByDirectory.set(directory, plannedNames);

    const lowerSeed = seedName.toLocaleLowerCase();
    const exists = existingNames.has(lowerSeed);
    const planned = plannedNames.has(lowerSeed);
    let nextName = seedName;
    let status = 'ready';
    let message = 'Ready';

    if (planned) {
      nextName = uniqueOutputName(seedName, (candidate) =>
        existingNames.has(candidate.toLocaleLowerCase()) || plannedNames.has(candidate.toLocaleLowerCase()),
      );
      message = 'Renamed';
    } else if (exists) {
      if (conflictPolicy.value === 'skip') {
        status = 'skip';
        message = 'Exists';
      } else if (conflictPolicy.value === 'replace') {
        status = 'replace';
        message = 'Replace';
      } else {
        nextName = uniqueOutputName(seedName, (candidate) =>
          existingNames.has(candidate.toLocaleLowerCase()) || plannedNames.has(candidate.toLocaleLowerCase()),
        );
        message = 'Renamed';
      }
    }

    if (status !== 'skip') {
      plannedNames.add(nextName.toLocaleLowerCase());
    }

    return {
      id: entry?.path || `${entry?.name || 'pdf'}-${index}`,
      entry,
      index,
      directory,
      nextName,
      status,
      message,
    };
  });
});
const previewSummary = computed(() => {
  let ready = 0;
  let replace = 0;
  let skip = 0;

  for (const row of previewRows.value) {
    if (row.status === 'replace') {
      replace += 1;
    } else if (row.status === 'skip') {
      skip += 1;
    } else {
      ready += 1;
    }
  }

  return { ready, replace, skip };
});
const compressionCount = computed(() => previewSummary.value.ready + previewSummary.value.replace);
const canCompress = computed(() => compressionCount.value > 0);
const primaryLabel = computed(() => (
  compressionCount.value === 1 ? 'Compress 1 PDF' : `Compress ${compressionCount.value} PDFs`
));
const summaryText = computed(() => {
  if (compressionCount.value > 0) {
    return `${compressionCount.value} ${compressionCount.value === 1 ? 'output' : 'outputs'}`;
  }

  return 'No outputs';
});
const summaryTone = computed(() => (compressionCount.value > 0 ? 'ready' : 'idle'));
const virtualPreviewRange = computed(() => {
  const rowHeight = Math.max(1, previewRowHeight.value);
  const count = previewRows.value.length;
  const viewportHeight = Math.max(previewViewportHeight.value || rowHeight * 10, rowHeight);
  const start = Math.min(
    Math.max(0, count - 1),
    Math.max(0, Math.floor(previewScrollTop.value / rowHeight) - PREVIEW_OVERSCAN_ROWS),
  );
  const visibleCount = Math.ceil(viewportHeight / rowHeight) + (PREVIEW_OVERSCAN_ROWS * 2);
  const end = Math.min(count, start + visibleCount);

  return {
    start,
    end,
    paddingBefore: start * rowHeight,
    paddingAfter: Math.max(0, (count - end) * rowHeight),
  };
});
const virtualPreviewRows = computed(() => (
  previewRows.value
    .slice(virtualPreviewRange.value.start, virtualPreviewRange.value.end)
    .map((row, offset) => ({
      ...row,
      virtualIndex: virtualPreviewRange.value.start + offset,
    }))
));
const previewRowStyle = computed(() => ({
  '--pdf-compress-row-height': `${previewRowHeight.value}px`,
}));

function resetState() {
  profile.value = 'balanced';
  conflictPolicy.value = 'keepBoth';
  destinationMode.value = 'sameFolder';
  keepOnlySmaller.value = false;
}

function updatePreviewMetrics() {
  previewViewportHeight.value = previewListRef.value?.clientHeight || 0;
  previewRowHeight.value = typeof window !== 'undefined' && window.matchMedia?.('(max-width: 620px)').matches
    ? MOBILE_PREVIEW_ROW_HEIGHT
    : DESKTOP_PREVIEW_ROW_HEIGHT;
}

function observePreviewList() {
  nextTick(() => {
    previewResizeObserver?.disconnect?.();
    previewResizeObserver = null;
    updatePreviewMetrics();

    if (typeof ResizeObserver !== 'undefined' && previewListRef.value) {
      previewResizeObserver = new ResizeObserver(updatePreviewMetrics);
      previewResizeObserver.observe(previewListRef.value);
    }
  });
}

function handlePreviewScroll(event) {
  previewScrollTop.value = event.target?.scrollTop || 0;
}

function outputDirectoryFor(entry) {
  if (destinationMode.value === 'otherPane' && props.otherPaneDirectory) {
    return props.otherPaneDirectory;
  }

  return entry?.directory || parentDirectoryForPath(entry?.path);
}

function existingNameSet(directory) {
  return new Set(
    (props.existingNamesByDirectory[directory] || [])
      .map((name) => String(name || '').toLocaleLowerCase()),
  );
}

function outputNameFor(name) {
  const stem = fileStemForName(name);

  return stem.toLocaleLowerCase().endsWith(' compressed')
    ? `${stem} 2.pdf`
    : `${stem} compressed.pdf`;
}

function fileStemForName(name) {
  const cleanName = String(name || '')
    .replace(/[\\/]/g, ' ')
    .trim()
    .replace(/^\.+|\.+$/g, '');

  if (!cleanName) {
    return 'Document';
  }

  const extension = extensionForName(cleanName);

  if (!extension) {
    return cleanName;
  }

  return cleanName.slice(0, -(extension.length + 1)).trim() || cleanName;
}

function uniqueOutputName(seedName, exists) {
  if (!exists(seedName)) {
    return seedName;
  }

  const extension = extensionForName(seedName);
  const suffix = extension ? `.${extension}` : '';
  const stem = extension ? seedName.slice(0, -(extension.length + 1)) : seedName;

  for (let index = 2; index < 1000; index += 1) {
    const candidate = `${stem} ${index}${suffix}`;

    if (!exists(candidate)) {
      return candidate;
    }
  }

  return `${stem} copy${suffix}`;
}

function parentDirectoryForPath(path) {
  const value = String(path || '').replace(/\/+$/, '');

  if (!value || value === '/' || value === '~') {
    return value || '~';
  }

  if (value.startsWith('remote://')) {
    const rest = value.slice('remote://'.length);
    const slashIndex = rest.indexOf('/');
    const volumeId = slashIndex >= 0 ? rest.slice(0, slashIndex) : rest;
    const objectPath = slashIndex >= 0 ? rest.slice(slashIndex + 1).replace(/\/+$/, '') : '';

    if (!volumeId || !objectPath) {
      return `remote://${volumeId}/`;
    }

    const parentIndex = objectPath.lastIndexOf('/');
    return parentIndex < 0
      ? `remote://${volumeId}/`
      : `remote://${volumeId}/${objectPath.slice(0, parentIndex)}`;
  }

  const slashIndex = value.lastIndexOf('/');
  return slashIndex <= 0 ? '/' : value.slice(0, slashIndex);
}

function directoryName(path) {
  const value = String(path || '').replace(/\/+$/, '');

  if (!value || value === '/') {
    return 'Root';
  }

  if (value.startsWith('remote://')) {
    const parts = value.slice('remote://'.length).split('/').filter(Boolean);
    return parts.at(-1) || parts[0] || 'Remote';
  }

  return value.split('/').filter(Boolean).at(-1) || value;
}

function formatBytes(bytes) {
  const size = Number(bytes);

  if (!Number.isFinite(size)) {
    return '';
  }

  if (size < 1024) {
    return `${size} B`;
  }

  const units = [
    ['TB', 1024 ** 4],
    ['GB', 1024 ** 3],
    ['MB', 1024 ** 2],
    ['KB', 1024],
  ];
  const [unit, value] = units.find((candidate) => size >= candidate[1]) || units.at(-1);

  return `${new Intl.NumberFormat(undefined, {
    maximumFractionDigits: size / value >= 10 ? 0 : 1,
  }).format(size / value)} ${unit}`;
}

function cancel() {
  emit('cancel');
}

function compress() {
  if (!canCompress.value) {
    return;
  }

  emit('compress', {
    paths: props.entries.map((entry) => entry.path),
    options: {
      profile: profile.value,
      conflict: conflictPolicy.value,
      keepOnlySmaller: keepOnlySmaller.value,
      destinationDirectory: destinationMode.value === 'otherPane' ? props.otherPaneDirectory : null,
    },
  });
}

function handleKeydown(event) {
  if (event.key === 'Escape') {
    event.preventDefault();
    cancel();
    return;
  }

  if (event.key === 'Enter' && (event.metaKey || event.ctrlKey)) {
    event.preventDefault();
    compress();
  }
}

watch(() => props.visible, (visible) => {
  if (!visible) {
    return;
  }

  resetState();
  previewScrollTop.value = 0;
  nextTick(() => panelRef.value?.focus?.({ preventScroll: true }));
  observePreviewList();
});

watch([profile, conflictPolicy, destinationMode], () => {
  previewScrollTop.value = 0;
});

onMounted(() => {
  if (props.visible) {
    observePreviewList();
  }
});

onBeforeUnmount(() => {
  previewResizeObserver?.disconnect?.();
});
</script>

<template>
  <Teleport to="body">
    <Transition name="pdf-compress-dialog">
      <div
        v-if="visible"
        class="pdf-compress-overlay"
        role="presentation"
        @pointerdown.self="cancel"
        @keydown.stop="handleKeydown"
      >
        <section
          ref="panelRef"
          class="pdf-compress-panel"
          role="dialog"
          aria-modal="true"
          aria-labelledby="pdf-compress-title"
          tabindex="-1"
        >
          <header class="pdf-compress-header">
            <div class="pdf-compress-title-row">
              <span class="pdf-compress-icon" aria-hidden="true">
                <AppIcon name="archive" :size="20" :stroke-width="1.9" />
              </span>
              <div class="pdf-compress-title-copy">
                <h2 id="pdf-compress-title">Compress PDF</h2>
                <p>
                  <span>{{ selectedLabel }}</span>
                  <span v-if="selectedSize" class="pdf-compress-title-size">{{ selectedSize }}</span>
                </p>
              </div>
            </div>

            <div class="pdf-compress-summary" :class="`pdf-compress-summary--${summaryTone}`">
              <strong>{{ summaryText }}</strong>
              <span>{{ previewSummary.skip }} skipped</span>
            </div>
          </header>

          <div class="pdf-compress-layout">
            <section class="pdf-compress-controls" aria-label="Compression options">
              <div class="pdf-compress-section">
                <span class="pdf-compress-eyebrow">Profile</span>
                <div class="pdf-compress-profile-grid" role="radiogroup" aria-label="Compression profile">
                  <label
                    v-for="option in PROFILES"
                    :key="option.value"
                    class="pdf-compress-profile"
                    :class="{ 'pdf-compress-profile--active': profile === option.value }"
                  >
                    <input v-model="profile" type="radio" name="pdf-compress-profile" :value="option.value">
                    <span class="pdf-compress-profile-icon" aria-hidden="true">
                      <AppIcon :name="option.icon" :size="15" :stroke-width="1.9" />
                    </span>
                    <span class="pdf-compress-profile-copy">
                      <strong>{{ option.label }}</strong>
                      <small>{{ option.detail }}</small>
                    </span>
                    <span class="pdf-compress-profile-check" aria-hidden="true">
                      <AppIcon name="check" :size="12" :stroke-width="2.7" />
                    </span>
                  </label>
                </div>
              </div>

              <div class="pdf-compress-section">
                <span class="pdf-compress-eyebrow">Destination</span>
                <div class="pdf-compress-segments" role="group" aria-label="Output destination">
                  <button
                    type="button"
                    :class="{ active: destinationMode === 'sameFolder' }"
                    :aria-pressed="destinationMode === 'sameFolder'"
                    @click="destinationMode = 'sameFolder'"
                  >
                    <AppIcon name="folder" :size="14" :stroke-width="1.9" />
                    <span>Same Folder</span>
                  </button>
                  <button
                    v-if="canUseOtherPane"
                    type="button"
                    :class="{ active: destinationMode === 'otherPane' }"
                    :aria-pressed="destinationMode === 'otherPane'"
                    @click="destinationMode = 'otherPane'"
                  >
                    <AppIcon name="open-other-pane" :size="14" :stroke-width="1.9" />
                    <span>Other Pane</span>
                  </button>
                </div>
                <span class="pdf-compress-destination">{{ destinationLabel }}</span>
              </div>

              <div class="pdf-compress-section">
                <span class="pdf-compress-eyebrow">Conflicts</span>
                <div class="pdf-compress-conflicts" role="radiogroup" aria-label="Name conflicts">
                  <label
                    v-for="policy in CONFLICT_POLICIES"
                    :key="policy.value"
                    class="pdf-compress-conflict"
                    :class="{ 'pdf-compress-conflict--active': conflictPolicy === policy.value }"
                  >
                    <input v-model="conflictPolicy" type="radio" name="pdf-compress-conflict" :value="policy.value">
                    <AppIcon :name="policy.icon" :size="14" :stroke-width="1.9" />
                    <span>{{ policy.label }}</span>
                  </label>
                </div>
              </div>

              <div class="pdf-compress-section">
                <span class="pdf-compress-eyebrow">Output</span>
                <label class="pdf-compress-switch">
                  <span class="pdf-compress-switch-copy">
                    <strong>Only keep smaller results</strong>
                    <small>Discard a result that isn't smaller than the source.</small>
                  </span>
                  <input v-model="keepOnlySmaller" type="checkbox" class="pdf-compress-switch-input">
                  <span class="pdf-compress-switch-ui" aria-hidden="true"></span>
                </label>
              </div>
            </section>

            <section class="pdf-compress-preview" :style="previewRowStyle" aria-label="Compression preview">
              <div class="pdf-compress-preview-head">
                <span>Source</span>
                <span>Output</span>
                <span>Status</span>
              </div>

              <div ref="previewListRef" class="pdf-compress-preview-list" @scroll.passive="handlePreviewScroll">
                <div v-if="previewRows.length === 0" class="pdf-compress-preview-empty">
                  No PDFs selected
                </div>
                <template v-else>
                  <div
                    v-if="virtualPreviewRange.paddingBefore > 0"
                    :style="{ height: `${virtualPreviewRange.paddingBefore}px` }"
                    aria-hidden="true"
                  ></div>

                  <article
                    v-for="row in virtualPreviewRows"
                    :key="row.id"
                    class="pdf-compress-row"
                    :class="`pdf-compress-row--${row.status}`"
                  >
                    <span class="pdf-compress-row-source">{{ row.entry?.name }}</span>
                    <span class="pdf-compress-row-output">{{ row.nextName }}</span>
                    <span class="pdf-compress-row-status">{{ row.message }}</span>
                  </article>

                  <div
                    v-if="virtualPreviewRange.paddingAfter > 0"
                    :style="{ height: `${virtualPreviewRange.paddingAfter}px` }"
                    aria-hidden="true"
                  ></div>
                </template>
              </div>
            </section>
          </div>

          <footer class="pdf-compress-footer">
            <button type="button" class="app-button app-button--subtle" @click="cancel">Cancel</button>
            <button
              type="button"
              class="app-button app-button--primary"
              :disabled="!canCompress"
              @click="compress"
            >
              {{ primaryLabel }}
            </button>
          </footer>
        </section>
      </div>
    </Transition>
  </Teleport>
</template>

<style scoped>
.pdf-compress-overlay {
  position: fixed;
  z-index: 5100;
  inset: 0;
  display: grid;
  place-items: center;
  padding: 28px;
  background: var(--overlay-bg);
  backdrop-filter: blur(6px);
  -webkit-backdrop-filter: blur(6px);
}

.pdf-compress-panel {
  display: grid;
  width: min(980px, calc(100vw - 56px));
  max-height: min(760px, calc(100vh - 56px));
  grid-template-rows: auto minmax(0, 1fr) auto;
  overflow: hidden;
  border: 1px solid var(--control-border);
  border-radius: 11px;
  background: var(--popover-bg);
  box-shadow: var(--shadow-overlay);
  color: var(--text);
}

.pdf-compress-header {
  display: flex;
  min-width: 0;
  align-items: center;
  justify-content: space-between;
  gap: 18px;
  border-bottom: 1px solid var(--separator);
  padding: 16px 18px;
}

.pdf-compress-title-row {
  display: flex;
  min-width: 0;
  align-items: center;
  gap: 11px;
}

.pdf-compress-icon {
  display: inline-flex;
  width: 34px;
  height: 34px;
  flex: 0 0 auto;
  align-items: center;
  justify-content: center;
  border-radius: 9px;
  background: color-mix(in srgb, var(--accent) 14%, transparent);
  color: var(--accent);
}

.pdf-compress-title-copy {
  min-width: 0;
}

.pdf-compress-title-copy h2 {
  margin: 0;
  color: var(--text);
  font-size: 15px;
  font-weight: 650;
  letter-spacing: 0;
}

.pdf-compress-title-copy p {
  display: flex;
  gap: 8px;
  margin: 3px 0 0;
  color: var(--text-muted);
  font-size: 12px;
}

.pdf-compress-title-size {
  color: var(--text-faint);
}

.pdf-compress-summary {
  display: inline-grid;
  min-width: 112px;
  justify-items: end;
  gap: 2px;
  color: var(--text-muted);
  font-size: 11px;
}

.pdf-compress-summary strong {
  color: var(--text);
  font-size: 13px;
}

.pdf-compress-summary--ready strong {
  color: var(--success);
}

.pdf-compress-layout {
  display: grid;
  min-height: 0;
  grid-template-columns: minmax(260px, 330px) minmax(0, 1fr);
}

.pdf-compress-controls {
  min-width: 0;
  overflow: auto;
  border-right: 1px solid var(--separator);
  padding: 16px;
}

.pdf-compress-section + .pdf-compress-section {
  margin-top: 18px;
}

.pdf-compress-eyebrow {
  display: block;
  margin-bottom: 8px;
  color: var(--text-faint);
  font-size: 10px;
  font-weight: 700;
  letter-spacing: 0;
  text-transform: uppercase;
}

.pdf-compress-profile-grid {
  display: grid;
  grid-template-columns: repeat(2, minmax(0, 1fr));
  gap: 8px;
}

.pdf-compress-profile {
  position: relative;
  display: flex;
  min-width: 0;
  align-items: center;
  gap: 8px;
  border: 1px solid var(--control-border);
  border-radius: 8px;
  padding: 9px;
  background: color-mix(in srgb, var(--control-glass) 72%, transparent);
  box-shadow: var(--input-shadow);
  cursor: pointer;
}

.pdf-compress-profile input,
.pdf-compress-conflict input {
  position: absolute;
  width: 1px;
  height: 1px;
  margin: -1px;
  overflow: hidden;
  clip-path: inset(50%);
}

.pdf-compress-profile--active,
.pdf-compress-conflict--active {
  border-color: color-mix(in srgb, var(--accent) 52%, var(--control-border));
  background: color-mix(in srgb, var(--accent) 12%, var(--control-glass));
}

.pdf-compress-profile-icon {
  display: flex;
  flex: 0 0 auto;
  color: var(--accent);
}

.pdf-compress-profile-copy {
  display: grid;
  min-width: 0;
  gap: 2px;
}

.pdf-compress-profile-copy strong,
.pdf-compress-profile-copy small {
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.pdf-compress-profile-copy strong {
  font-size: 12px;
  font-weight: 650;
}

.pdf-compress-profile-copy small {
  color: var(--text-muted);
  font-size: 11px;
}

.pdf-compress-profile-check {
  margin-left: auto;
  color: var(--accent);
  opacity: 0;
}

.pdf-compress-profile--active .pdf-compress-profile-check {
  opacity: 1;
}

.pdf-compress-segments {
  display: inline-flex;
  width: 100%;
  min-width: 0;
  gap: 4px;
  border-radius: 9px;
  padding: 3px;
  background: var(--control-bg);
  box-shadow: var(--control-inset);
}

.pdf-compress-segments button {
  display: inline-flex;
  min-width: 0;
  flex: 1 1 0;
  align-items: center;
  justify-content: center;
  gap: 6px;
  border-radius: 7px;
  padding: 7px 8px;
  background: transparent;
  color: var(--text-muted);
  font-size: 12px;
  font-weight: 600;
}

.pdf-compress-segments button.active {
  background: color-mix(in srgb, var(--text) 9%, transparent);
  color: var(--text);
  box-shadow: inset 0 1px 0 rgb(255 255 255 / 0.18);
}

.pdf-compress-destination {
  display: block;
  min-width: 0;
  margin-top: 7px;
  overflow: hidden;
  color: var(--text-muted);
  font-size: 11px;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.pdf-compress-conflicts {
  display: grid;
  gap: 8px;
}

.pdf-compress-conflict {
  position: relative;
  display: flex;
  min-width: 0;
  align-items: center;
  gap: 8px;
  border: 1px solid var(--control-border);
  border-radius: 8px;
  padding: 8px 10px;
  background: color-mix(in srgb, var(--control-glass) 72%, transparent);
  box-shadow: var(--input-shadow);
  color: var(--text-muted);
  font-size: 12px;
  font-weight: 600;
  cursor: pointer;
}

.pdf-compress-conflict--active {
  color: var(--text);
}

.pdf-compress-switch {
  position: relative;
  display: grid;
  grid-template-columns: minmax(0, 1fr) auto;
  align-items: center;
  gap: 12px;
  border: 1px solid var(--control-border);
  border-radius: 9px;
  padding: 10px 11px;
  background: color-mix(in srgb, var(--control-glass) 72%, transparent);
  box-shadow: var(--input-shadow);
  cursor: pointer;
}

.pdf-compress-switch-copy {
  display: grid;
  min-width: 0;
  gap: 3px;
}

.pdf-compress-switch-copy strong {
  color: var(--text);
  font-size: 12px;
  font-weight: 650;
}

.pdf-compress-switch-copy small {
  color: var(--text-muted);
  font-size: 11px;
  line-height: 1.35;
}

.pdf-compress-switch-input {
  position: absolute;
  width: 1px;
  height: 1px;
  overflow: hidden;
  clip: rect(0 0 0 0);
  clip-path: inset(50%);
  white-space: nowrap;
}

.pdf-compress-switch-ui {
  position: relative;
  display: block;
  width: 42px;
  height: 24px;
  flex: 0 0 42px;
  border: 1px solid var(--input-border);
  border-radius: 999px;
  background: color-mix(in srgb, var(--text) 9%, transparent);
  box-shadow: var(--input-shadow);
  transition: background 120ms ease, border-color 120ms ease;
}

.pdf-compress-switch-ui::after {
  position: absolute;
  top: 2px;
  left: 2px;
  width: 18px;
  height: 18px;
  border-radius: 50%;
  background: color-mix(in srgb, var(--text) 78%, transparent);
  box-shadow: 0 1px 4px rgb(0 0 0 / 0.28);
  content: "";
  transition: transform 120ms ease, background 120ms ease;
}

.pdf-compress-switch-input:checked + .pdf-compress-switch-ui {
  border-color: var(--accent-border);
  background: var(--accent);
}

.pdf-compress-switch-input:checked + .pdf-compress-switch-ui::after {
  background: #fff;
  transform: translateX(18px);
}

.pdf-compress-switch-input:focus-visible + .pdf-compress-switch-ui {
  box-shadow:
    var(--accent-focus-ring),
    var(--input-shadow);
}

.pdf-compress-preview {
  display: grid;
  min-width: 0;
  min-height: 0;
  grid-template-rows: 32px minmax(0, 1fr);
}

.pdf-compress-preview-head,
.pdf-compress-row {
  display: grid;
  grid-template-columns: minmax(0, 1fr) minmax(0, 1fr) 82px;
  align-items: center;
  gap: 14px;
}

.pdf-compress-preview-head {
  border-bottom: 1px solid var(--separator);
  padding: 0 14px;
  color: var(--text-muted);
  font-size: 11px;
  font-weight: 650;
}

.pdf-compress-preview-list {
  min-height: 0;
  overflow: auto;
}

.pdf-compress-preview-empty {
  display: grid;
  height: 100%;
  place-items: center;
  color: var(--text-muted);
  font-size: 13px;
}

.pdf-compress-row {
  height: var(--pdf-compress-row-height);
  border-bottom: 1px solid color-mix(in srgb, var(--separator) 56%, transparent);
  padding: 0 14px;
  color: var(--text);
  font-size: 12px;
}

.pdf-compress-row:nth-child(even) {
  background: color-mix(in srgb, var(--text) 2.5%, transparent);
}

.pdf-compress-row-source,
.pdf-compress-row-output {
  min-width: 0;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.pdf-compress-row-output {
  color: var(--text);
  font-weight: 600;
}

.pdf-compress-row-status {
  justify-self: start;
  color: var(--text-muted);
  font-size: 11px;
}

.pdf-compress-row--replace .pdf-compress-row-status {
  color: var(--warning);
}

.pdf-compress-row--skip {
  color: var(--text-faint);
}

.pdf-compress-footer {
  display: flex;
  justify-content: flex-end;
  gap: 9px;
  border-top: 1px solid var(--separator);
  padding: 13px 16px;
}

.pdf-compress-dialog-enter-active,
.pdf-compress-dialog-leave-active {
  transition: opacity 180ms ease;
}

.pdf-compress-dialog-enter-from,
.pdf-compress-dialog-leave-to {
  opacity: 0;
}

.pdf-compress-dialog-enter-active .pdf-compress-panel,
.pdf-compress-dialog-leave-active .pdf-compress-panel {
  transition: transform 160ms cubic-bezier(0.2, 0, 0, 1), opacity 140ms ease;
}

.pdf-compress-dialog-enter-from .pdf-compress-panel,
.pdf-compress-dialog-leave-to .pdf-compress-panel {
  opacity: 0;
  transform: translateY(8px) scale(0.985);
}

@media (max-width: 760px) {
  .pdf-compress-overlay {
    padding: 12px;
  }

  .pdf-compress-panel {
    width: calc(100vw - 24px);
    max-height: calc(100vh - 24px);
  }

  .pdf-compress-layout {
    grid-template-columns: minmax(0, 1fr);
  }

  .pdf-compress-controls {
    border-right: 0;
    border-bottom: 1px solid var(--separator);
  }

  .pdf-compress-preview {
    min-height: 260px;
  }

  .pdf-compress-preview-head,
  .pdf-compress-row {
    grid-template-columns: minmax(0, 1fr) 88px;
  }

  .pdf-compress-preview-head span:nth-child(2),
  .pdf-compress-row-output {
    display: none;
  }
}
</style>
