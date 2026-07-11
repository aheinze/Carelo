<script setup>
import { computed, onMounted, onUnmounted, ref, watch } from 'vue';
import AppIcon from './AppIcon.vue';
import { useFolderCompare } from '../composables/useFolderCompare';
import { useDialog } from '../composables/useDialog';
import { useFileManagerStore } from '../stores/fileManagerStore';
import { compareDirectories, copyItems, deleteItems } from '../composables/useFileOperations';
import { formatFileDateTime } from '../utils/dateFormat';
import {
  extractFileOperationBatch,
  listCompletedFileOperationItems,
} from '../utils/fileOperationResults';

const panel = useFolderCompare();
const dialog = useDialog();
const store = useFileManagerStore();

const loading = ref(false);
const syncing = ref(false);
const error = ref('');
const result = ref(null);
const direction = ref('lr'); // 'lr' = left → right, 'rl' = right → left
const mirror = ref(false);
const filterText = ref('');
const overrides = ref({});
let compareToken = 0;

const STATUS_META = {
  only_left: { label: 'Only on left', icon: 'arrow-up', tone: 'add' },
  only_right: { label: 'Only on right', icon: 'arrow-up', tone: 'add' },
  left_newer: { label: 'Left is newer', icon: 'pencil', tone: 'change' },
  right_newer: { label: 'Right is newer', icon: 'pencil', tone: 'change' },
  differs: { label: 'Differs', icon: 'pencil', tone: 'change' },
  type_conflict: { label: 'Type conflict', icon: 'alert', tone: 'conflict' },
};

function joinRoot(root, relativePath) {
  const base = String(root || '').replace(/\/+$/, '');
  return `${base}/${relativePath}`;
}

function fmtSize(bytes) {
  const value = Number(bytes);

  if (!Number.isFinite(value)) {
    return '';
  }

  if (value >= 1024 ** 3) {
    return `${(value / 1024 ** 3).toFixed(1)} GB`;
  }

  if (value >= 1024 ** 2) {
    return `${(value / 1024 ** 2).toFixed(1)} MB`;
  }

  if (value >= 1024) {
    return `${(value / 1024).toFixed(1)} KB`;
  }

  return `${value} B`;
}

function sideSummary(side) {
  if (!side) {
    return '—';
  }

  if (side.isDir) {
    return 'folder';
  }

  const parts = [];
  const size = fmtSize(side.size);
  const date = side.modifiedAt
    ? formatFileDateTime(side.modifiedAt, store.appSettings.dateFormat, { fallback: '' })
    : '';

  if (size) {
    parts.push(size);
  }

  if (date) {
    parts.push(date);
  }

  return parts.join(' · ') || 'file';
}

// Resolve what a row would do given the current direction and mirror setting.
function rowAction(entry) {
  const isLR = direction.value === 'lr';
  const onlySource = isLR ? 'only_left' : 'only_right';
  const onlyDest = isLR ? 'only_right' : 'only_left';

  if (entry.status === 'type_conflict') {
    return { kind: 'conflict' };
  }

  if (entry.status === onlySource) {
    return { kind: 'copy', overwrite: false, toRight: isLR };
  }

  if (entry.status === onlyDest) {
    return mirror.value ? { kind: 'delete', onRight: isLR } : { kind: 'skip' };
  }

  // Exists on both but differs.
  return { kind: 'copy', overwrite: true, toRight: isLR };
}

// Rows that would overwrite a newer file on the destination start unchecked.
function overwritesNewer(entry) {
  return (
    (direction.value === 'lr' && entry.status === 'right_newer') ||
    (direction.value === 'rl' && entry.status === 'left_newer')
  );
}

function defaultIncluded(entry) {
  const action = rowAction(entry);

  if (action.kind === 'skip' || action.kind === 'conflict') {
    return false;
  }

  if (action.kind === 'copy' && overwritesNewer(entry)) {
    return false;
  }

  return true;
}

function isActionable(entry) {
  const action = rowAction(entry);
  return action.kind === 'copy' || action.kind === 'delete';
}

function isIncluded(entry) {
  if (!isActionable(entry)) {
    return false;
  }

  const override = overrides.value[entry.relativePath];
  return override === undefined ? defaultIncluded(entry) : override;
}

function toggleRow(entry) {
  if (!isActionable(entry)) {
    return;
  }

  overrides.value = {
    ...overrides.value,
    [entry.relativePath]: !isIncluded(entry),
  };
}

// The list is not virtualized; cap rendered rows so a huge diff stays
// responsive. Sync still operates on every included entry, not just the
// visible ones — the filter is there to find specific rows.
const RENDER_LIMIT = 1500;

const allEntries = computed(() => result.value?.entries || []);
const filteredEntries = computed(() => {
  const query = filterText.value.trim().toLowerCase();

  if (!query) {
    return allEntries.value;
  }

  return allEntries.value.filter((entry) => entry.relativePath.toLowerCase().includes(query));
});
const renderedEntries = computed(() => filteredEntries.value.slice(0, RENDER_LIMIT));
const hiddenRowCount = computed(() => filteredEntries.value.length - renderedEntries.value.length);

const plan = computed(() => {
  let copies = 0;
  let deletes = 0;

  for (const entry of allEntries.value) {
    if (!isIncluded(entry)) {
      continue;
    }

    if (rowAction(entry).kind === 'delete') {
      deletes += 1;
    } else {
      copies += 1;
    }
  }

  return { copies, deletes, total: copies + deletes };
});

const directionLabel = computed(() => (direction.value === 'lr' ? 'Left → Right' : 'Right → Left'));

async function runCompare() {
  const token = ++compareToken;
  loading.value = true;
  error.value = '';
  result.value = null;
  overrides.value = {};

  try {
    const data = await compareDirectories(panel.leftRoot.value, panel.rightRoot.value, {
      includeHidden: store.showHiddenFiles,
    });

    if (token !== compareToken) {
      return;
    }

    result.value = data;
  } catch (err) {
    if (token !== compareToken) {
      return;
    }

    error.value = err?.message || 'Unable to compare these folders.';
  } finally {
    if (token === compareToken) {
      loading.value = false;
    }
  }
}

async function runSync() {
  if (syncing.value || !result.value || plan.value.total === 0) {
    return;
  }

  const isLR = direction.value === 'lr';
  // Capture roots up front: closing the dialog mid-sync nulls `result`.
  const leftRootPath = result.value.leftRoot;
  const rightRootPath = result.value.rightRoot;
  const sourceRoot = isLR ? leftRootPath : rightRootPath;
  const destRoot = isLR ? rightRootPath : leftRootPath;
  const copies = [];
  const deletes = [];

  for (const entry of allEntries.value) {
    if (!isIncluded(entry)) {
      continue;
    }

    const action = rowAction(entry);

    if (action.kind === 'copy') {
      copies.push({
        from: joinRoot(sourceRoot, entry.relativePath),
        to: joinRoot(destRoot, entry.relativePath),
        overwrite: action.overwrite,
        symlinkMode: 'preserve',
      });
    } else if (action.kind === 'delete') {
      deletes.push(joinRoot(destRoot, entry.relativePath));
    }
  }

  if (deletes.length > 0) {
    const confirmed = await dialog.confirm({
      title: 'Mirror Delete',
      message: `${deletes.length} item${deletes.length === 1 ? '' : 's'} on the destination side will be moved to Trash.`,
      detail: 'These items do not exist on the source side. They are recoverable from Trash.',
      confirmLabel: 'Move to Trash',
      variant: 'danger',
    });

    if (!confirmed) {
      return;
    }
  }

  syncing.value = true;
  const jobId = store.startQueueJob({
    operation: 'sync',
    label: `Syncing ${plan.value.total} item${plan.value.total === 1 ? '' : 's'} (${directionLabel.value})`,
    detail: `To ${destRoot}`,
    remotePaths: [sourceRoot, destRoot],
  });

  const refreshAfterSync = async () => {
    await store.reloadDirectoriesInPanes([leftRootPath, rightRootPath]).catch(() => {});
    // Only re-run the diff if the dialog is still open.
    if (panel.visible.value) {
      await runCompare();
    }
  };

  try {
    if (copies.length > 0) {
      await copyItems(copies, jobId, store.transferMaxConcurrency());
    }

    if (deletes.length > 0) {
      let deleteBatch = null;
      let deleteError = null;

      try {
        deleteBatch = await deleteItems(deletes, 'trash');
      } catch (error) {
        deleteBatch = extractFileOperationBatch(error);
        deleteError = error;
      }

      const deletedPaths = listCompletedFileOperationItems(deleteBatch).map((item) => item.from);
      store.recordTrashDelete({
        paths: deletedPaths,
        directories: [destRoot],
        label: deletedPaths.length === 1 ? 'Deleted 1 sync item' : `Deleted ${deletedPaths.length} sync items`,
        deleteMode: 'trash',
      });

      if (deleteError) {
        throw deleteError;
      }
    }

    store.completeQueueJob(jobId, 'Folders synced');
    await refreshAfterSync();
  } catch (err) {
    if (err?.code === 'operation_cancelled') {
      store.cancelQueueJobDone(jobId);
      await refreshAfterSync();
    } else {
      store.failQueueJob(jobId, err?.message || 'Sync failed.');

      if (panel.visible.value) {
        await dialog.alert({
          title: 'Sync Failed',
          message: err?.message || 'Some items could not be synced.',
          variant: 'warning',
        });
      }
    }
  } finally {
    syncing.value = false;
  }
}

function close() {
  panel.close();
}

function onKeydown(event) {
  if (event.key === 'Escape') {
    event.stopPropagation();
    close();
  }
}

watch([direction, mirror], () => {
  overrides.value = {};
});

watch(
  () => panel.visible.value,
  (visible) => {
    if (visible) {
      direction.value = 'lr';
      mirror.value = false;
      filterText.value = '';
      runCompare();
    } else {
      compareToken += 1;
      result.value = null;
      error.value = '';
    }
  },
  { immediate: true },
);

onMounted(() => window.addEventListener('keydown', onKeydown, true));
onUnmounted(() => window.removeEventListener('keydown', onKeydown, true));
</script>

<template>
  <Teleport to="body">
    <Transition name="compare-fade">
      <div
        v-if="panel.visible.value"
        class="compare-overlay"
        role="dialog"
        aria-modal="true"
        aria-label="Compare folders"
        @pointerdown.self="close"
      >
        <div class="compare-panel">
          <header class="compare-header">
            <div class="compare-title-group">
              <AppIcon name="columns" :size="18" :stroke-width="1.8" />
              <h2>Compare &amp; Sync Folders</h2>
            </div>
            <button type="button" class="compare-close" aria-label="Close" @click="close">
              <AppIcon name="x" :size="14" :stroke-width="2" />
            </button>
          </header>

          <div class="compare-roots">
            <span class="compare-root" :title="panel.leftRoot.value">{{ panel.leftRoot.value }}</span>
            <span class="compare-root-sep" aria-hidden="true">vs</span>
            <span class="compare-root" :title="panel.rightRoot.value">{{ panel.rightRoot.value }}</span>
          </div>

          <div v-if="loading" class="compare-state">
            <AppIcon name="refresh" :size="22" :stroke-width="1.6" class="compare-spin" />
            <span>Comparing folders…</span>
          </div>

          <div v-else-if="error" class="compare-state compare-state--error">
            <AppIcon name="alert" :size="22" :stroke-width="1.6" />
            <span>{{ error }}</span>
          </div>

          <div v-else-if="result && allEntries.length === 0" class="compare-state compare-state--ok">
            <AppIcon name="check" :size="22" :stroke-width="2" />
            <span>These folders are already in sync.</span>
            <small>{{ result.identical }} identical item{{ result.identical === 1 ? '' : 's' }}.</small>
          </div>

          <template v-else-if="result">
            <div class="compare-summary">
              <span class="compare-chip compare-chip--add">{{ result.onlyLeft }} only left</span>
              <span class="compare-chip compare-chip--add">{{ result.onlyRight }} only right</span>
              <span class="compare-chip compare-chip--change">{{ result.differing }} differing</span>
              <span class="compare-chip">{{ result.identical }} identical</span>
              <span v-if="result.truncated" class="compare-chip compare-chip--warn">
                Truncated at 20,000 differences
              </span>
            </div>

            <div class="compare-controls">
              <div class="compare-direction" role="group" aria-label="Sync direction">
                <button
                  type="button"
                  :class="{ active: direction === 'lr' }"
                  @click="direction = 'lr'"
                >
                  Left → Right
                </button>
                <button
                  type="button"
                  :class="{ active: direction === 'rl' }"
                  @click="direction = 'rl'"
                >
                  Right → Left
                </button>
              </div>

              <label class="compare-mirror">
                <span class="compare-mirror-label">Mirror (delete extras on destination)</span>
                <input v-model="mirror" class="switch-input" type="checkbox" />
                <span class="settings-switch" aria-hidden="true"></span>
              </label>

              <label class="compare-filter">
                <AppIcon name="search" :size="13" :stroke-width="1.9" />
                <input
                  v-model="filterText"
                  type="search"
                  placeholder="Filter by path…"
                  aria-label="Filter differences by path"
                />
              </label>
            </div>

            <ul class="compare-list">
              <li
                v-for="entry in renderedEntries"
                :key="entry.relativePath"
                class="compare-row"
                :class="{ 'compare-row--inactive': !isActionable(entry) }"
              >
                <input
                  v-if="isActionable(entry)"
                  type="checkbox"
                  class="compare-row-check"
                  :checked="isIncluded(entry)"
                  :aria-label="`Include ${entry.relativePath}`"
                  @change="toggleRow(entry)"
                />
                <span v-else class="compare-row-check compare-row-check--spacer" aria-hidden="true"></span>

                <span
                  class="compare-status"
                  :class="`compare-status--${STATUS_META[entry.status]?.tone || 'change'}`"
                  :title="STATUS_META[entry.status]?.label"
                >
                  <AppIcon :name="entry.isDir ? 'folder' : (STATUS_META[entry.status]?.icon || 'file')" :size="13" :stroke-width="1.8" />
                </span>

                <span class="compare-path" :title="entry.relativePath">{{ entry.relativePath }}</span>

                <span class="compare-side compare-side--left">{{ sideSummary(entry.left) }}</span>
                <span class="compare-arrow" :class="`compare-arrow--${rowAction(entry).kind}`">
                  <template v-if="rowAction(entry).kind === 'copy'">{{ rowAction(entry).toRight ? '→' : '←' }}</template>
                  <AppIcon v-else-if="rowAction(entry).kind === 'delete'" name="trash" :size="12" :stroke-width="1.9" />
                  <template v-else-if="rowAction(entry).kind === 'conflict'">!</template>
                  <template v-else>–</template>
                </span>
                <span class="compare-side compare-side--right">{{ sideSummary(entry.right) }}</span>
              </li>
              <li v-if="hiddenRowCount > 0" class="compare-row compare-row--overflow">
                {{ hiddenRowCount }} more difference{{ hiddenRowCount === 1 ? '' : 's' }} hidden — use the filter to narrow. Sync still applies to all selected items.
              </li>
            </ul>
          </template>

          <footer v-if="result && allEntries.length > 0" class="compare-footer">
            <span class="compare-plan">
              <template v-if="plan.total === 0">Nothing selected to sync</template>
              <template v-else>
                Will copy {{ plan.copies }}<template v-if="plan.deletes > 0">, delete {{ plan.deletes }}</template>
              </template>
            </span>
            <div class="compare-footer-actions">
              <button type="button" class="compare-btn" :disabled="syncing" @click="runCompare">
                Re-compare
              </button>
              <button
                type="button"
                class="compare-btn compare-btn--primary"
                :disabled="syncing || plan.total === 0"
                @click="runSync"
              >
                {{ syncing ? 'Syncing…' : `Sync ${directionLabel}` }}
              </button>
            </div>
          </footer>
        </div>
      </div>
    </Transition>
  </Teleport>
</template>

<style scoped>
.compare-overlay {
  position: fixed;
  z-index: 5000;
  inset: 0;
  display: grid;
  place-items: center;
  padding: 32px;
  background: var(--overlay-bg);
}

.compare-panel {
  display: flex;
  flex-direction: column;
  gap: 14px;
  width: min(900px, calc(100vw - 48px));
  max-height: calc(100vh - 72px);
  overflow: hidden;
  border: 1px solid var(--control-border);
  border-radius: var(--radius-panel);
  padding: 20px 22px 18px;
  background: var(--modal-bg);
  box-shadow: var(--shadow-overlay);
}

.compare-header {
  display: flex;
  align-items: center;
  justify-content: space-between;
  flex-shrink: 0;
}

.compare-title-group {
  display: flex;
  align-items: center;
  gap: 10px;
  color: var(--text);
}

.compare-header h2 {
  margin: 0;
  font-size: 15px;
  font-weight: 700;
  letter-spacing: -0.01em;
}

.compare-close {
  display: grid;
  width: 26px;
  height: 26px;
  place-items: center;
  border-radius: 7px;
  background: transparent;
  color: var(--icon);
  transition: background 100ms ease, color 100ms ease;
}

.compare-close:hover {
  background: var(--btn-hover);
  color: var(--text);
}

.compare-roots {
  display: flex;
  align-items: center;
  gap: 10px;
  flex-shrink: 0;
  font-size: 12px;
  color: var(--text-muted);
}

.compare-root {
  flex: 1 1 0;
  min-width: 0;
  overflow: hidden;
  padding: 5px 9px;
  border-radius: 7px;
  background: color-mix(in srgb, var(--text) 4%, transparent);
  font-family: ui-monospace, SFMono-Regular, Menlo, monospace;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.compare-root-sep {
  flex-shrink: 0;
  color: var(--text-faint);
  font-weight: 600;
}

.compare-state {
  display: grid;
  place-items: center;
  align-content: center;
  gap: 10px;
  min-height: 200px;
  color: var(--text-muted);
  font-size: 13px;
  font-weight: 560;
}

.compare-state small {
  color: var(--text-faint);
  font-weight: 500;
}

.compare-state--error {
  color: var(--danger);
}

.compare-state--ok {
  color: var(--success);
}

.compare-spin {
  animation: compare-spin 900ms linear infinite;
}

@keyframes compare-spin {
  to {
    transform: rotate(360deg);
  }
}

.compare-summary {
  display: flex;
  flex-wrap: wrap;
  gap: 6px;
  flex-shrink: 0;
}

.compare-chip {
  padding: 3px 9px;
  border-radius: 999px;
  background: color-mix(in srgb, var(--text) 7%, transparent);
  color: var(--text-muted);
  font-size: 11px;
  font-weight: 650;
}

.compare-chip--add {
  background: var(--accent-dim);
  color: var(--accent);
}

.compare-chip--change {
  background: color-mix(in srgb, var(--warning) 18%, transparent);
  color: var(--warning);
}

.compare-chip--warn {
  background: color-mix(in srgb, var(--danger) 16%, transparent);
  color: var(--danger);
}

.compare-controls {
  display: flex;
  align-items: center;
  gap: 14px;
  flex-wrap: wrap;
  flex-shrink: 0;
}

.compare-direction {
  display: flex;
  border: 1px solid var(--control-border);
  border-radius: 8px;
  overflow: hidden;
}

.compare-direction button {
  padding: 6px 12px;
  background: transparent;
  color: var(--text-muted);
  font-size: 12px;
  font-weight: 600;
  cursor: pointer;
}

.compare-direction button.active {
  background: var(--accent);
  color: #fff;
}

.compare-mirror {
  display: flex;
  align-items: center;
  gap: 9px;
  color: var(--text-muted);
  font-size: 12px;
  font-weight: 540;
  cursor: pointer;
}

/* Switch — matches the app's settings/open-with toggles. */
.switch-input {
  position: absolute;
  width: 1px;
  height: 1px;
  overflow: hidden;
  clip: rect(0 0 0 0);
  clip-path: inset(50%);
  white-space: nowrap;
}

.settings-switch {
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

.settings-switch::after {
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

.switch-input:checked + .settings-switch {
  border-color: var(--accent-border);
  background: var(--accent);
}

.switch-input:checked + .settings-switch::after {
  background: #ffffff;
  transform: translateX(18px);
}

.switch-input:focus-visible + .settings-switch {
  box-shadow:
    var(--accent-focus-ring),
    var(--input-shadow);
}

.compare-filter {
  display: flex;
  align-items: center;
  gap: 7px;
  margin-left: auto;
  min-width: 180px;
  padding: 0 9px;
  height: 30px;
  border: 1px solid var(--input-border);
  border-radius: 8px;
  background: var(--input-bg);
  color: var(--text-faint);
}

.compare-filter:focus-within {
  border-color: var(--accent-border);
  box-shadow: var(--accent-focus-ring);
}

.compare-filter input {
  width: 100%;
  min-width: 0;
  border: 0;
  outline: 0;
  background: transparent;
  color: var(--text);
  font-size: 12px;
}

.compare-list {
  list-style: none;
  margin: 0;
  padding: 4px 0;
  overflow-y: auto;
  flex: 1 1 auto;
  border-top: 1px solid var(--hairline);
  border-bottom: 1px solid var(--hairline);
}

.compare-row {
  display: grid;
  grid-template-columns: 18px 20px minmax(0, 1fr) minmax(72px, auto) 24px minmax(72px, auto);
  align-items: center;
  gap: 8px;
  padding: 5px 4px;
  border-radius: 6px;
  font-size: 12px;
}

.compare-row:hover {
  background: color-mix(in srgb, var(--text) 4%, transparent);
}

.compare-row--inactive {
  opacity: 0.55;
}

.compare-row--overflow {
  display: block;
  padding: 10px 6px;
  color: var(--text-faint);
  font-size: 11px;
  font-style: italic;
}

.compare-row--overflow:hover {
  background: transparent;
}

.compare-row-check {
  width: 14px;
  height: 14px;
  margin: 0;
  cursor: pointer;
}

.compare-row-check--spacer {
  cursor: default;
}

.compare-status {
  display: grid;
  place-items: center;
  width: 20px;
  height: 20px;
  border-radius: 5px;
}

.compare-status--add {
  color: var(--accent);
  background: var(--accent-dim);
}

.compare-status--change {
  color: var(--warning);
  background: color-mix(in srgb, var(--warning) 16%, transparent);
}

.compare-status--conflict {
  color: var(--danger);
  background: color-mix(in srgb, var(--danger) 16%, transparent);
}

.compare-path {
  min-width: 0;
  overflow: hidden;
  color: var(--text);
  font-family: ui-monospace, SFMono-Regular, Menlo, monospace;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.compare-side {
  color: var(--text-faint);
  font-size: 11px;
  text-align: right;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.compare-side--left {
  text-align: right;
}

.compare-arrow {
  display: grid;
  place-items: center;
  color: var(--text-muted);
  font-size: 13px;
  font-weight: 700;
}

.compare-arrow--copy {
  color: var(--accent);
}

.compare-arrow--delete {
  color: var(--danger);
}

.compare-arrow--conflict {
  color: var(--danger);
}

.compare-footer {
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: 12px;
  flex-shrink: 0;
}

.compare-plan {
  color: var(--text-muted);
  font-size: 12px;
  font-weight: 560;
}

.compare-footer-actions {
  display: flex;
  gap: 8px;
}

.compare-btn {
  height: 32px;
  padding: 0 14px;
  border: 1px solid var(--control-border);
  border-radius: 8px;
  background: var(--control-bg);
  color: var(--text);
  font-size: 12px;
  font-weight: 620;
  cursor: pointer;
  transition: background 100ms ease, opacity 100ms ease;
}

.compare-btn:hover:not(:disabled) {
  background: var(--btn-hover);
}

.compare-btn:disabled {
  opacity: 0.45;
  cursor: default;
}

.compare-btn--primary {
  border-color: transparent;
  background: var(--accent);
  color: #fff;
}

.compare-btn--primary:hover:not(:disabled) {
  background: var(--accent);
  filter: brightness(1.08);
}

.compare-fade-enter-active {
  transition: opacity 180ms ease;
}
.compare-fade-leave-active {
  transition: opacity 140ms ease;
}
.compare-fade-enter-active .compare-panel {
  transition: transform 220ms cubic-bezier(0.2, 0, 0, 1), opacity 180ms ease;
}
.compare-fade-leave-active .compare-panel {
  transition: transform 140ms ease, opacity 120ms ease;
}
.compare-fade-enter-from,
.compare-fade-leave-to {
  opacity: 0;
}
.compare-fade-enter-from .compare-panel,
.compare-fade-leave-to .compare-panel {
  opacity: 0;
  transform: scale(0.97) translateY(8px);
}
</style>
