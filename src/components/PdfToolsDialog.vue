<script setup>
import { computed, nextTick, onBeforeUnmount, onMounted, ref, watch } from 'vue';
import AppIcon from './AppIcon.vue';
import { extensionForName } from '../utils/fileTypes';

const DESKTOP_PREVIEW_ROW_HEIGHT = 34;
const MOBILE_PREVIEW_ROW_HEIGHT = 72;
const PREVIEW_OVERSCAN_ROWS = 12;

const TOOLS = [
  { value: 'compress', label: 'Compress', icon: 'archive', min: 1, hint: 'Reduce file size. Choose how much to compress below.' },
  { value: 'merge', label: 'Merge', icon: 'copy', min: 2, hint: 'Combine the selected PDFs into one document, in the order selected.' },
  { value: 'extractPages', label: 'Extract', icon: 'extract', min: 1, hint: 'Save the chosen page range as a new PDF.' },
  { value: 'splitPages', label: 'Split', icon: 'columns', min: 1, hint: 'Save every page of each PDF as its own file.' },
  { value: 'rotatePages', label: 'Rotate', icon: 'refresh', min: 1, hint: 'Turn pages clockwise — every page, or just a range.' },
  { value: 'unlock', label: 'Unlock', icon: 'lock', min: 1, hint: 'Remove a known password so the PDF opens freely.' },
];

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

const ROTATIONS = [
  { value: 90, label: '90°' },
  { value: 180, label: '180°' },
  { value: 270, label: '270°' },
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
  initialTool: {
    type: String,
    default: 'compress',
  },
});

const emit = defineEmits(['cancel', 'run']);

const panelRef = ref(null);
const previewListRef = ref(null);
const previewScrollTop = ref(0);
const previewViewportHeight = ref(0);
const previewRowHeight = ref(DESKTOP_PREVIEW_ROW_HEIGHT);
const tool = ref(props.initialTool || 'compress');
const compressionProfile = ref('balanced');
const keepOnlySmaller = ref(false);
const pageRanges = ref('1');
const rotation = ref(90);
const password = ref('');
const conflictPolicy = ref('keepBoth');
const destinationMode = ref('sameFolder');
let previewResizeObserver = null;

const availableTools = computed(() => (
  TOOLS.filter((option) => props.entries.length >= option.min)
));
const currentTool = computed(() => (
  availableTools.value.find((option) => option.value === tool.value) || availableTools.value[0] || TOOLS[1]
));
const currentToolHint = computed(() => currentTool.value?.hint || '');
const selectedLabel = computed(() => {
  const count = props.entries.length;
  return `${count} ${count === 1 ? 'PDF' : 'PDFs'} selected`;
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
const rangeValue = computed(() => pageRanges.value.trim());
const needsRange = computed(() => tool.value === 'extractPages');
const showsRange = computed(() => tool.value === 'extractPages' || tool.value === 'rotatePages');
const rangePlaceholder = computed(() => (tool.value === 'rotatePages' ? 'All pages' : '1-3,5'));
const previewRows = computed(() => {
  if (tool.value === 'merge') {
    const firstEntry = props.entries[0] || {};
    const directory = outputDirectoryFor(firstEntry);
    const seedName = outputNameFor(firstEntry?.name || 'Merged.pdf', 'merged');

    return [previewRowFor({
      id: 'merge',
      entry: { name: props.entries.map((entry) => entry.name).join(', ') || 'Selected PDFs' },
      directory,
      seedName,
      index: 0,
    })];
  }

  const plannedByDirectory = new Map();

  return props.entries.map((entry, index) => {
    const directory = outputDirectoryFor(entry);
    const plannedNames = plannedByDirectory.get(directory) || new Set();
    plannedByDirectory.set(directory, plannedNames);

    if (tool.value === 'splitPages') {
      return {
        id: entry?.path || `${entry?.name || 'pdf'}-${index}`,
        entry,
        directory,
        nextName: `${fileStemForName(entry?.name || `Document ${index + 1}`)} page 01.pdf ...`,
        status: 'ready',
        message: 'Multiple outputs',
        virtualIndex: index,
      };
    }

    const seedName = outputNameFor(entry?.name || `Document ${index + 1}`, outputSuffixForTool());
    const row = previewRowFor({ id: entry?.path || `${entry?.name || 'pdf'}-${index}`, entry, directory, seedName, index }, plannedNames);

    if (row.status !== 'skip') {
      plannedNames.add(row.nextName.toLocaleLowerCase());
    }

    return row;
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
const outputCount = computed(() => {
  if (tool.value === 'splitPages') {
    return props.entries.length;
  }

  return previewSummary.value.ready + previewSummary.value.replace;
});
const canRun = computed(() => (
  outputCount.value > 0
  && (!needsRange.value || rangeValue.value.length > 0)
  && availableTools.value.some((option) => option.value === tool.value)
));
const primaryLabel = computed(() => {
  if (tool.value === 'compress') return outputCount.value === 1 ? 'Compress 1 PDF' : `Compress ${outputCount.value} PDFs`;
  if (tool.value === 'merge') return 'Merge PDFs';
  if (tool.value === 'extractPages') return outputCount.value === 1 ? 'Extract Pages' : 'Extract Pages';
  if (tool.value === 'splitPages') return outputCount.value === 1 ? 'Split 1 PDF' : `Split ${outputCount.value} PDFs`;
  if (tool.value === 'rotatePages') return outputCount.value === 1 ? 'Rotate 1 PDF' : `Rotate ${outputCount.value} PDFs`;
  return outputCount.value === 1 ? 'Unlock 1 PDF' : `Unlock ${outputCount.value} PDFs`;
});
const summaryText = computed(() => {
  if (tool.value === 'splitPages') {
    return outputCount.value > 0 ? 'Per-page outputs' : 'No outputs';
  }

  if (outputCount.value > 0) {
    return `${outputCount.value} ${outputCount.value === 1 ? 'output' : 'outputs'}`;
  }

  return 'No outputs';
});
const summaryTone = computed(() => (canRun.value ? 'ready' : 'idle'));
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
  '--pdf-tools-row-height': `${previewRowHeight.value}px`,
}));

function resetState() {
  tool.value = availableTools.value.some((option) => option.value === props.initialTool)
    ? props.initialTool
    : availableTools.value[0]?.value || 'extractPages';
  compressionProfile.value = 'balanced';
  keepOnlySmaller.value = false;
  pageRanges.value = tool.value === 'rotatePages' ? '' : '1';
  rotation.value = 90;
  password.value = '';
  conflictPolicy.value = 'keepBoth';
  destinationMode.value = 'sameFolder';
}

function previewRowFor(row, plannedNames = new Set()) {
  const existingNames = existingNameSet(row.directory);
  const lowerSeed = row.seedName.toLocaleLowerCase();
  const exists = existingNames.has(lowerSeed);
  const planned = plannedNames.has(lowerSeed);
  let nextName = row.seedName;
  let status = 'ready';
  let message = 'Ready';

  if (planned) {
    nextName = uniqueOutputName(row.seedName, (candidate) =>
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
      nextName = uniqueOutputName(row.seedName, (candidate) =>
        existingNames.has(candidate.toLocaleLowerCase()) || plannedNames.has(candidate.toLocaleLowerCase()),
      );
      message = 'Renamed';
    }
  }

  return {
    id: row.id,
    entry: row.entry,
    directory: row.directory,
    nextName,
    status,
    message,
    virtualIndex: row.index,
  };
}

function outputSuffixForTool() {
  if (tool.value === 'compress') return 'compressed';

  if (tool.value === 'extractPages') {
    return `pages ${rangeNameFragment(rangeValue.value) || 'selection'}`;
  }

  if (tool.value === 'rotatePages') return 'rotated';
  if (tool.value === 'unlock') return 'unlocked';

  return 'merged';
}

function outputNameFor(name, suffix) {
  const stem = fileStemForName(name);
  const safeSuffix = safeNameFragment(suffix);

  return stem.toLocaleLowerCase().endsWith(` ${safeSuffix.toLocaleLowerCase()}`)
    ? `${stem} 2.pdf`
    : `${stem} ${safeSuffix}.pdf`;
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

function rangeNameFragment(value) {
  return safeNameFragment(value);
}

function safeNameFragment(value) {
  return String(value || '')
    .replace(/[^a-z0-9-]+/gi, ' ')
    .replace(/\s+/g, ' ')
    .trim();
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

function cancel() {
  emit('cancel');
}

function run() {
  if (!canRun.value) {
    return;
  }

  emit('run', {
    paths: props.entries.map((entry) => entry.path),
    options: {
      tool: tool.value,
      conflict: conflictPolicy.value,
      destinationDirectory: destinationMode.value === 'otherPane' ? props.otherPaneDirectory : null,
      profile: tool.value === 'compress' ? compressionProfile.value : null,
      keepOnlySmaller: tool.value === 'compress' ? keepOnlySmaller.value : null,
      pageRanges: showsRange.value ? rangeValue.value : null,
      rotation: tool.value === 'rotatePages' ? Number(rotation.value) : null,
      password: tool.value === 'unlock' ? password.value : null,
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
    run();
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

watch(tool, (nextTool, previousTool) => {
  if (nextTool === 'rotatePages' && previousTool !== 'rotatePages' && pageRanges.value === '1') {
    pageRanges.value = '';
  } else if (nextTool === 'extractPages' && !pageRanges.value.trim()) {
    pageRanges.value = '1';
  }
});

watch([tool, pageRanges, conflictPolicy, destinationMode], () => {
  previewScrollTop.value = 0;
});

onMounted(() => {
  if (props.visible) {
    resetState();
    previewScrollTop.value = 0;
    observePreviewList();
    nextTick(() => panelRef.value?.focus?.({ preventScroll: true }));
  }

  window.addEventListener('resize', updatePreviewMetrics);
});

onBeforeUnmount(() => {
  previewResizeObserver?.disconnect?.();
  window.removeEventListener('resize', updatePreviewMetrics);
});
</script>

<template>
  <Teleport to="body">
    <div
      v-if="visible"
      class="pdf-tools-overlay"
      role="presentation"
      @pointerdown.self="cancel"
      @keydown.stop="handleKeydown"
    >
      <section
        ref="panelRef"
        class="pdf-tools-panel"
        role="dialog"
        aria-modal="true"
        aria-labelledby="pdf-tools-title"
        tabindex="-1"
      >
        <header class="pdf-tools-header">
          <div class="pdf-tools-title-row">
            <span class="pdf-tools-icon" aria-hidden="true">
              <AppIcon name="file-text" :size="20" :stroke-width="1.9" />
            </span>
            <div class="pdf-tools-title-copy">
              <h2 id="pdf-tools-title">PDF Tools</h2>
              <p>{{ selectedLabel }}</p>
            </div>
          </div>

          <div class="pdf-tools-summary" :class="`pdf-tools-summary--${summaryTone}`">
            <strong>{{ summaryText }}</strong>
            <span>{{ previewSummary.skip }} skipped</span>
          </div>
        </header>

          <div class="pdf-tools-tabs" role="radiogroup" aria-label="PDF tool">
            <label
              v-for="option in availableTools"
              :key="option.value"
              class="pdf-tools-tab"
              :class="{ 'pdf-tools-tab--active': tool === option.value }"
            >
              <input v-model="tool" type="radio" name="pdf-tool" :value="option.value">
              <AppIcon :name="option.icon" :size="16" :stroke-width="1.9" />
              <span>{{ option.label }}</span>
            </label>
          </div>

          <div class="pdf-tools-layout">
            <section class="pdf-tools-controls" aria-label="PDF tool options">
              <p class="pdf-tools-hint">{{ currentToolHint }}</p>

              <div v-if="tool === 'compress'" class="pdf-tools-section">
                <span class="pdf-tools-eyebrow">Quality</span>
                <div class="pdf-tools-grid" role="radiogroup" aria-label="Compression quality">
                  <label
                    v-for="option in PROFILES"
                    :key="option.value"
                    class="pdf-tools-tool"
                    :class="{ 'pdf-tools-tool--active': compressionProfile === option.value }"
                  >
                    <input v-model="compressionProfile" type="radio" name="pdf-compression-profile" :value="option.value">
                    <span class="pdf-tools-tool-icon" aria-hidden="true">
                      <AppIcon :name="option.icon" :size="15" :stroke-width="1.9" />
                    </span>
                    <span class="pdf-tools-tool-copy">
                      <strong>{{ option.label }}</strong>
                      <small>{{ option.detail }}</small>
                    </span>
                    <span class="pdf-tools-tool-check" aria-hidden="true">
                      <AppIcon name="check" :size="12" :stroke-width="2.7" />
                    </span>
                  </label>
                </div>
                <label class="pdf-tools-switch">
                  <span class="pdf-tools-switch-copy">
                    <strong>Only keep smaller results</strong>
                    <small>Discard a result that isn't smaller than the source.</small>
                  </span>
                  <input v-model="keepOnlySmaller" type="checkbox" class="pdf-tools-switch-input">
                  <span class="pdf-tools-switch-ui" aria-hidden="true"></span>
                </label>
              </div>

              <div v-if="tool === 'extractPages'" class="pdf-tools-section">
                <span class="pdf-tools-eyebrow">Pages to extract</span>
                <input
                  v-model="pageRanges"
                  class="pdf-tools-input"
                  type="text"
                  :placeholder="rangePlaceholder"
                  spellcheck="false"
                  aria-label="Pages to extract"
                >
                <span class="pdf-tools-help">Use ranges like 1-3, 5, 8-10.</span>
              </div>

              <template v-if="tool === 'rotatePages'">
                <div class="pdf-tools-section">
                  <span class="pdf-tools-eyebrow">Rotate by</span>
                  <div class="pdf-tools-segments" role="radiogroup" aria-label="Rotation">
                    <button
                      v-for="option in ROTATIONS"
                      :key="option.value"
                      type="button"
                      :class="{ active: rotation === option.value }"
                      :aria-pressed="rotation === option.value"
                      @click="rotation = option.value"
                    >
                      <span>{{ option.label }}</span>
                    </button>
                  </div>
                </div>
                <div class="pdf-tools-section">
                  <span class="pdf-tools-eyebrow">Pages</span>
                  <input
                    v-model="pageRanges"
                    class="pdf-tools-input"
                    type="text"
                    :placeholder="rangePlaceholder"
                    spellcheck="false"
                    aria-label="Pages to rotate"
                  >
                  <span class="pdf-tools-help">Leave blank to rotate every page.</span>
                </div>
              </template>

              <div v-if="tool === 'unlock'" class="pdf-tools-section">
                <span class="pdf-tools-eyebrow">Password</span>
                <input
                  v-model="password"
                  class="pdf-tools-input"
                  type="password"
                  autocomplete="off"
                  placeholder="Known password"
                  aria-label="Known password"
                >
                <span class="pdf-tools-help">An unlocked copy is saved — the original stays untouched.</span>
              </div>

              <div class="pdf-tools-section">
                <span class="pdf-tools-eyebrow">Save to</span>
                <div class="pdf-tools-segments" role="group" aria-label="Output destination">
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
                <span class="pdf-tools-destination">{{ destinationLabel }}</span>
              </div>

              <div class="pdf-tools-section">
                <span class="pdf-tools-eyebrow">If a name already exists</span>
                <div class="pdf-tools-segments" role="radiogroup" aria-label="Name conflicts">
                  <button
                    v-for="policy in CONFLICT_POLICIES"
                    :key="policy.value"
                    type="button"
                    :class="{ active: conflictPolicy === policy.value }"
                    :aria-pressed="conflictPolicy === policy.value"
                    @click="conflictPolicy = policy.value"
                  >
                    <span>{{ policy.label }}</span>
                  </button>
                </div>
              </div>
            </section>

            <section class="pdf-tools-preview" :style="previewRowStyle" aria-label="PDF tool preview">
              <div class="pdf-tools-preview-head">
                <span>Source</span>
                <span>Output</span>
                <span>Status</span>
              </div>

              <div
                ref="previewListRef"
                class="pdf-tools-preview-list"
                @scroll.passive="handlePreviewScroll"
              >
                <div
                  v-if="virtualPreviewRange.paddingBefore > 0"
                  :style="{ height: `${virtualPreviewRange.paddingBefore}px` }"
                  aria-hidden="true"
                ></div>

                <article
                  v-for="row in virtualPreviewRows"
                  :key="row.id"
                  class="pdf-tools-row"
                  :class="`pdf-tools-row--${row.status}`"
                >
                  <span class="pdf-tools-row-source">{{ row.entry?.name }}</span>
                  <span class="pdf-tools-row-output">{{ row.nextName }}</span>
                  <span class="pdf-tools-row-status">{{ row.message }}</span>
                </article>

                <div
                  v-if="virtualPreviewRange.paddingAfter > 0"
                  :style="{ height: `${virtualPreviewRange.paddingAfter}px` }"
                  aria-hidden="true"
                ></div>
              </div>
            </section>
          </div>

        <footer class="pdf-tools-footer">
          <button type="button" class="app-button app-button--subtle" @click="cancel">Cancel</button>
          <button
            type="button"
            class="app-button app-button--primary"
            :disabled="!canRun"
            @click="run"
          >
            {{ primaryLabel }}
          </button>
        </footer>
      </section>
    </div>
  </Teleport>
</template>

<style scoped>
.pdf-tools-overlay {
  position: fixed;
  z-index: 5100;
  inset: 0;
  display: grid;
  isolation: isolate;
  place-items: center;
  padding: 28px;
  background: var(--overlay-bg);
}

.pdf-tools-panel {
  position: relative;
  z-index: 1;
  display: grid;
  width: min(980px, calc(100vw - 56px));
  max-height: min(760px, calc(100vh - 56px));
  grid-template-rows: auto auto minmax(0, 1fr) auto;
  overflow: hidden;
  border: 1px solid var(--control-border);
  border-radius: 11px;
  background: var(--popover-bg);
  box-shadow: var(--shadow-overlay);
  color: var(--text);
}

.pdf-tools-header {
  display: flex;
  min-width: 0;
  align-items: center;
  justify-content: space-between;
  gap: 18px;
  border-bottom: 1px solid var(--separator);
  padding: 16px 18px;
}

.pdf-tools-title-row,
.pdf-tools-title-copy,
.pdf-tools-tool-copy {
  min-width: 0;
}

.pdf-tools-tabs {
  display: flex;
  flex-wrap: wrap;
  gap: 4px;
  border-bottom: 1px solid var(--separator);
  padding: 10px 14px;
}

.pdf-tools-tab {
  display: inline-flex;
  align-items: center;
  gap: 7px;
  min-height: 30px;
  border-radius: 999px;
  padding: 0 12px;
  background: transparent;
  color: var(--text-muted);
  font-size: 12.5px;
  font-weight: 600;
  cursor: pointer;
  transition: background 120ms ease, color 120ms ease;
}

.pdf-tools-tab :deep(svg) {
  flex: 0 0 auto;
}

.pdf-tools-tab span {
  white-space: nowrap;
}

.pdf-tools-tab:hover {
  background: color-mix(in srgb, var(--text) 6%, transparent);
  color: var(--text);
}

.pdf-tools-tab--active,
.pdf-tools-tab--active:hover {
  background: var(--accent);
  color: #fff;
}

.pdf-tools-hint {
  margin: 0;
  color: var(--text-muted);
  font-size: 12px;
  line-height: 1.4;
}

.pdf-tools-title-row {
  display: flex;
  align-items: center;
  gap: 11px;
}

.pdf-tools-icon {
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

.pdf-tools-title-copy h2 {
  margin: 0;
  color: var(--text);
  font-size: 15px;
  font-weight: 650;
  letter-spacing: 0;
}

.pdf-tools-title-copy p {
  margin: 3px 0 0;
  color: var(--text-muted);
  font-size: 12px;
}

.pdf-tools-summary {
  display: inline-grid;
  min-width: 112px;
  justify-items: end;
  gap: 2px;
  color: var(--text-muted);
  font-size: 11px;
}

.pdf-tools-summary strong {
  color: var(--text);
  font-size: 13px;
}

.pdf-tools-summary--ready strong {
  color: var(--success);
}

.pdf-tools-layout {
  display: grid;
  min-height: 0;
  grid-template-columns: minmax(260px, 330px) minmax(0, 1fr);
}

.pdf-tools-controls {
  display: flex;
  min-width: 0;
  min-height: 0;
  flex-direction: column;
  gap: 18px;
  overflow: auto;
  border-right: 1px solid var(--separator);
  padding: 16px;
}

.pdf-tools-section {
  display: grid;
  gap: 8px;
}

.pdf-tools-eyebrow {
  color: var(--text-faint);
  font-size: 10px;
  font-weight: 700;
  letter-spacing: 0;
  text-transform: uppercase;
}

.pdf-tools-grid {
  display: grid;
  grid-template-columns: repeat(2, minmax(0, 1fr));
  gap: 8px;
}

.pdf-tools-tool {
  position: relative;
  display: grid;
  grid-template-columns: auto minmax(0, 1fr) auto;
  align-items: center;
  gap: 9px;
  border: 1px solid var(--control-border);
  border-radius: 8px;
  padding: 9px 10px;
  background: color-mix(in srgb, var(--control-glass) 72%, transparent);
  box-shadow: var(--input-shadow);
  cursor: pointer;
}

.pdf-tools-tool input,
.pdf-tools-tab input {
  position: absolute;
  width: 1px;
  height: 1px;
  margin: -1px;
  overflow: hidden;
  clip-path: inset(50%);
}

.pdf-tools-tool--active {
  border-color: color-mix(in srgb, var(--accent) 52%, var(--control-border));
  background: color-mix(in srgb, var(--accent) 12%, var(--control-glass));
}

.pdf-tools-tool:focus-within {
  border-color: var(--accent-border);
  box-shadow:
    var(--accent-focus-ring),
    var(--input-shadow);
}

.pdf-tools-tab:focus-within {
  box-shadow: var(--accent-focus-ring);
}

.pdf-tools-tool-icon {
  display: flex;
  flex: 0 0 auto;
  color: var(--accent);
}

.pdf-tools-tool-copy {
  display: grid;
  min-width: 0;
  gap: 2px;
}

.pdf-tools-tool-copy strong {
  overflow: hidden;
  color: var(--text);
  font-size: 12px;
  font-weight: 650;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.pdf-tools-tool-copy small {
  overflow: hidden;
  color: var(--text-muted);
  font-size: 11px;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.pdf-tools-tool-check {
  display: flex;
  flex: 0 0 auto;
  color: var(--accent);
  opacity: 0;
}

.pdf-tools-tool--active .pdf-tools-tool-check {
  opacity: 1;
}

.pdf-tools-input {
  width: 100%;
  height: 34px;
  border: 1px solid var(--input-border);
  border-radius: 9px;
  padding: 0 11px;
  background: var(--input-bg);
  box-shadow: var(--input-shadow);
  color: var(--text);
  font-size: 13px;
  font-weight: 520;
  outline: 0;
}

.pdf-tools-input:focus {
  border-color: var(--accent-border);
  box-shadow:
    var(--accent-focus-ring),
    var(--input-shadow);
}

.pdf-tools-help {
  color: var(--text-faint);
  font-size: 11px;
  line-height: 1.35;
}

.pdf-tools-switch {
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

.pdf-tools-switch-copy {
  display: grid;
  min-width: 0;
  gap: 3px;
}

.pdf-tools-switch-copy strong {
  color: var(--text);
  font-size: 12px;
  font-weight: 650;
}

.pdf-tools-switch-copy small {
  color: var(--text-muted);
  font-size: 11px;
  line-height: 1.35;
}

.pdf-tools-switch-input {
  position: absolute;
  width: 1px;
  height: 1px;
  overflow: hidden;
  clip: rect(0 0 0 0);
  clip-path: inset(50%);
  white-space: nowrap;
}

.pdf-tools-switch-ui {
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

.pdf-tools-switch-ui::after {
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

.pdf-tools-switch-input:checked + .pdf-tools-switch-ui {
  border-color: var(--accent-border);
  background: var(--accent);
}

.pdf-tools-switch-input:checked + .pdf-tools-switch-ui::after {
  background: #fff;
  transform: translateX(18px);
}

.pdf-tools-switch-input:focus-visible + .pdf-tools-switch-ui {
  box-shadow:
    var(--accent-focus-ring),
    var(--input-shadow);
}

.pdf-tools-segments {
  display: inline-flex;
  width: 100%;
  min-width: 0;
  gap: 4px;
  border-radius: 9px;
  padding: 3px;
  background: var(--control-bg);
  box-shadow: var(--control-inset);
}

.pdf-tools-segments button {
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

.pdf-tools-segments button.active {
  background: color-mix(in srgb, var(--text) 9%, transparent);
  color: var(--text);
  box-shadow: inset 0 1px 0 rgb(255 255 255 / 0.18);
}

.pdf-tools-destination {
  display: block;
  min-width: 0;
  overflow: hidden;
  color: var(--text-muted);
  font-size: 11px;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.pdf-tools-preview {
  display: grid;
  min-width: 0;
  min-height: 0;
  grid-template-rows: 32px minmax(0, 1fr);
}

.pdf-tools-preview-head,
.pdf-tools-row {
  display: grid;
  grid-template-columns: minmax(0, 1fr) minmax(0, 1fr) 82px;
  align-items: center;
  gap: 14px;
}

.pdf-tools-preview-head {
  border-bottom: 1px solid var(--separator);
  padding: 0 14px;
  color: var(--text-muted);
  font-size: 11px;
  font-weight: 650;
}

.pdf-tools-preview-list {
  min-height: 0;
  overflow: auto;
}

.pdf-tools-row {
  height: var(--pdf-tools-row-height);
  border-bottom: 1px solid color-mix(in srgb, var(--separator) 56%, transparent);
  padding: 0 14px;
  color: var(--text);
  font-size: 12px;
}

.pdf-tools-row:nth-child(even) {
  background: color-mix(in srgb, var(--text) 2.5%, transparent);
}

.pdf-tools-row-source,
.pdf-tools-row-output {
  min-width: 0;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.pdf-tools-row-output {
  color: var(--text);
  font-weight: 600;
}

.pdf-tools-row-status {
  justify-self: start;
  color: var(--text-muted);
  font-size: 11px;
}

.pdf-tools-row--replace .pdf-tools-row-status {
  color: var(--warning);
}

.pdf-tools-row--skip {
  color: var(--text-faint);
}

.pdf-tools-footer {
  display: flex;
  justify-content: flex-end;
  gap: 9px;
  border-top: 1px solid var(--separator);
  padding: 13px 16px;
}

@media (max-width: 760px) {
  .pdf-tools-overlay {
    padding: 12px;
  }

  .pdf-tools-panel {
    width: calc(100vw - 24px);
    max-height: calc(100vh - 24px);
  }

  .pdf-tools-layout {
    grid-template-columns: minmax(0, 1fr);
    grid-template-rows: auto minmax(240px, 1fr);
  }

  .pdf-tools-controls {
    max-height: 44vh;
    border-right: 0;
    border-bottom: 1px solid var(--separator);
  }

  .pdf-tools-preview-head,
  .pdf-tools-row {
    grid-template-columns: minmax(0, 1fr) 88px;
  }

  .pdf-tools-preview-head span:nth-child(2),
  .pdf-tools-row-output {
    display: none;
  }
}
</style>
