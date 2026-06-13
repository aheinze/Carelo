<script setup>
import { computed, onMounted, onUnmounted, ref, watch } from 'vue';
import AppIcon from './AppIcon.vue';
import { useChecksumDialog } from '../composables/useChecksumDialog';
import { computeFileChecksum } from '../composables/useFileOperations';

const dialog = useChecksumDialog();

const results = ref([]);
const expected = ref('');
const copiedPath = ref('');
let runToken = 0;
let copiedTimer = null;

function fileName(path) {
  const clean = String(path || '').replace(/\/+$/, '');
  const segment = clean.split('/').filter(Boolean).at(-1);
  return segment || clean || 'Untitled';
}

function formatSize(bytes) {
  const value = Number(bytes);

  if (!Number.isFinite(value)) {
    return '';
  }

  if (value >= 1024 ** 3) {
    return `${(value / 1024 ** 3).toFixed(2)} GB`;
  }

  if (value >= 1024 ** 2) {
    return `${(value / 1024 ** 2).toFixed(2)} MB`;
  }

  if (value >= 1024) {
    return `${(value / 1024).toFixed(1)} KB`;
  }

  return `${value} B`;
}

function normalizeHash(value) {
  return String(value || '').trim().toLowerCase().replace(/[\s:]/g, '');
}

const isSingle = computed(() => results.value.length === 1);
const allDone = computed(() =>
  results.value.length > 0 && results.value.every((entry) => entry.status === 'done'),
);

const expectedNormalized = computed(() => normalizeHash(expected.value));
const expectedState = computed(() => {
  if (!isSingle.value || !expectedNormalized.value) {
    return 'idle';
  }

  const entry = results.value[0];

  if (entry?.status !== 'done') {
    return 'idle';
  }

  return normalizeHash(entry.hash) === expectedNormalized.value ? 'match' : 'mismatch';
});

// When exactly two files are checked, report whether they are identical.
const pairVerdict = computed(() => {
  if (results.value.length !== 2 || !allDone.value) {
    return 'idle';
  }

  return normalizeHash(results.value[0].hash) === normalizeHash(results.value[1].hash)
    ? 'identical'
    : 'different';
});

async function runChecksums(paths) {
  const token = ++runToken;
  results.value = paths.map((path) => ({
    path,
    name: fileName(path),
    status: 'pending',
    hash: '',
    bytes: null,
    algorithm: 'SHA-256',
    error: '',
  }));

  for (let index = 0; index < paths.length; index += 1) {
    try {
      const checksum = await computeFileChecksum(paths[index]);

      if (token !== runToken) {
        return;
      }

      results.value = results.value.map((entry, position) =>
        position === index
          ? {
              ...entry,
              status: 'done',
              hash: checksum.hash,
              bytes: checksum.bytes,
              algorithm: checksum.algorithm || 'SHA-256',
            }
          : entry,
      );
    } catch (error) {
      if (token !== runToken) {
        return;
      }

      results.value = results.value.map((entry, position) =>
        position === index
          ? { ...entry, status: 'error', error: error?.message || 'Unable to read this file.' }
          : entry,
      );
    }
  }
}

async function copyHash(entry) {
  if (entry.status !== 'done' || !navigator.clipboard?.writeText) {
    return;
  }

  try {
    await navigator.clipboard.writeText(entry.hash);
    copiedPath.value = entry.path;
    window.clearTimeout(copiedTimer);
    copiedTimer = window.setTimeout(() => {
      copiedPath.value = '';
    }, 1500);
  } catch {
    // Clipboard access can be denied; nothing actionable to surface here.
  }
}

function close() {
  dialog.close();
}

function onKeydown(event) {
  if (event.key === 'Escape') {
    event.stopPropagation();
    close();
  }
}

watch(
  () => dialog.visible.value,
  (visible) => {
    if (visible) {
      expected.value = '';
      copiedPath.value = '';
      runChecksums([...dialog.paths.value]);
    } else {
      runToken += 1;
      results.value = [];
    }
  },
  { immediate: true },
);

onMounted(() => window.addEventListener('keydown', onKeydown, true));
onUnmounted(() => {
  window.removeEventListener('keydown', onKeydown, true);
  window.clearTimeout(copiedTimer);
});
</script>

<template>
  <Teleport to="body">
    <Transition name="checksum-fade">
      <div
        v-if="dialog.visible.value"
        class="checksum-overlay"
        role="dialog"
        aria-modal="true"
        aria-label="Verify checksum"
        @pointerdown.self="close"
      >
        <div class="checksum-panel">
          <header class="checksum-header">
            <div class="checksum-title-group">
              <AppIcon name="shield" :size="18" :stroke-width="1.8" />
              <h2>Verify Checksum</h2>
              <span class="checksum-algo">SHA-256</span>
            </div>
            <button type="button" class="checksum-close" aria-label="Close" @click="close">
              <AppIcon name="x" :size="14" :stroke-width="2" />
            </button>
          </header>

          <div
            v-if="pairVerdict !== 'idle'"
            class="checksum-verdict"
            :class="`checksum-verdict--${pairVerdict === 'identical' ? 'match' : 'mismatch'}`"
          >
            <AppIcon :name="pairVerdict === 'identical' ? 'check' : 'x'" :size="15" :stroke-width="2.2" />
            <span>{{ pairVerdict === 'identical' ? 'Files are identical' : 'Files are different' }}</span>
          </div>

          <ul class="checksum-list">
            <li v-for="entry in results" :key="entry.path" class="checksum-row">
              <div class="checksum-row-head">
                <AppIcon name="file" :size="15" :stroke-width="1.7" class="checksum-file-icon" />
                <span class="checksum-name" :title="entry.path">{{ entry.name }}</span>
                <span v-if="entry.status === 'done' && entry.bytes !== null" class="checksum-size">
                  {{ formatSize(entry.bytes) }}
                </span>
              </div>

              <div v-if="entry.status === 'pending'" class="checksum-hash checksum-hash--pending">
                Computing…
              </div>
              <div v-else-if="entry.status === 'error'" class="checksum-hash checksum-hash--error">
                {{ entry.error }}
              </div>
              <div v-else class="checksum-hash-row">
                <code class="checksum-hash">{{ entry.hash }}</code>
                <button
                  type="button"
                  class="checksum-copy"
                  :aria-label="`Copy checksum for ${entry.name}`"
                  @click="copyHash(entry)"
                >
                  <AppIcon :name="copiedPath === entry.path ? 'check' : 'copy'" :size="14" :stroke-width="1.9" />
                  <span>{{ copiedPath === entry.path ? 'Copied' : 'Copy' }}</span>
                </button>
              </div>
            </li>
          </ul>

          <div v-if="isSingle" class="checksum-verify">
            <label class="checksum-verify-label" for="checksum-expected">
              Compare against an expected checksum
            </label>
            <div
              class="checksum-verify-field"
              :class="{
                'checksum-verify-field--match': expectedState === 'match',
                'checksum-verify-field--mismatch': expectedState === 'mismatch',
              }"
            >
              <input
                id="checksum-expected"
                v-model="expected"
                type="text"
                autocomplete="off"
                spellcheck="false"
                placeholder="Paste a published SHA-256 hash…"
              />
              <span
                v-if="expectedState === 'match'"
                class="checksum-verify-badge checksum-verify-badge--match"
              >
                <AppIcon name="check" :size="13" :stroke-width="2.4" /> Match
              </span>
              <span
                v-else-if="expectedState === 'mismatch'"
                class="checksum-verify-badge checksum-verify-badge--mismatch"
              >
                <AppIcon name="x" :size="13" :stroke-width="2.4" /> No match
              </span>
            </div>
          </div>
        </div>
      </div>
    </Transition>
  </Teleport>
</template>

<style scoped>
.checksum-overlay {
  position: fixed;
  z-index: 5000;
  inset: 0;
  display: grid;
  place-items: center;
  padding: 32px;
  background: var(--overlay-bg);
}

.checksum-panel {
  display: flex;
  flex-direction: column;
  gap: 16px;
  width: min(640px, calc(100vw - 48px));
  max-height: calc(100vh - 80px);
  overflow: hidden auto;
  border: 1px solid var(--control-border);
  border-radius: var(--radius-panel);
  padding: 22px 24px 24px;
  background: var(--modal-bg);
  box-shadow: var(--shadow-overlay);
}

.checksum-header {
  display: flex;
  align-items: center;
  justify-content: space-between;
  flex-shrink: 0;
}

.checksum-title-group {
  display: flex;
  align-items: center;
  gap: 10px;
  color: var(--text);
}

.checksum-header h2 {
  margin: 0;
  font-size: 15px;
  font-weight: 700;
  letter-spacing: -0.01em;
}

.checksum-algo {
  padding: 2px 8px;
  border: 1px solid rgb(var(--accent-rgb) / 0.4);
  border-radius: 6px;
  background: var(--accent-dim);
  color: var(--accent);
  font-size: 10px;
  font-weight: 700;
  letter-spacing: 0.04em;
}

.checksum-close {
  display: grid;
  width: 26px;
  height: 26px;
  place-items: center;
  border-radius: 7px;
  background: transparent;
  color: var(--icon);
  transition: background 100ms ease, color 100ms ease;
}

.checksum-close:hover {
  background: var(--btn-hover);
  color: var(--text);
}

.checksum-verdict {
  display: flex;
  align-items: center;
  gap: 8px;
  padding: 10px 14px;
  border-radius: 10px;
  font-size: 13px;
  font-weight: 650;
}

.checksum-verdict--match {
  border: 1px solid color-mix(in srgb, var(--success) 45%, transparent);
  background: color-mix(in srgb, var(--success) 16%, transparent);
  color: var(--success);
}

.checksum-verdict--mismatch {
  border: 1px solid rgb(var(--danger-rgb) / 0.4);
  background: color-mix(in srgb, var(--danger) 16%, transparent);
  color: var(--danger);
}

.checksum-list {
  list-style: none;
  margin: 0;
  padding: 0;
  display: flex;
  flex-direction: column;
  gap: 10px;
}

.checksum-row {
  display: flex;
  flex-direction: column;
  gap: 8px;
  padding: 12px 14px;
  border-radius: 12px;
  border: 1px solid var(--hairline);
  background: color-mix(in srgb, var(--text) 3.5%, transparent);
}

.checksum-row-head {
  display: flex;
  align-items: center;
  gap: 8px;
  min-width: 0;
}

.checksum-file-icon {
  color: var(--icon);
  flex-shrink: 0;
}

.checksum-name {
  flex: 1;
  min-width: 0;
  overflow: hidden;
  color: var(--text);
  font-size: 13px;
  font-weight: 600;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.checksum-size {
  flex-shrink: 0;
  color: var(--text-faint);
  font-size: 11px;
  font-weight: 600;
}

.checksum-hash-row {
  display: flex;
  align-items: center;
  gap: 8px;
}

.checksum-hash {
  flex: 1;
  min-width: 0;
  font-family: ui-monospace, SFMono-Regular, Menlo, monospace;
  font-size: 12px;
  line-height: 1.5;
  letter-spacing: 0;
  color: var(--text-muted);
  word-break: break-all;
  user-select: all;
}

.checksum-hash--pending {
  color: var(--text-faint);
  font-family: inherit;
  font-style: italic;
}

.checksum-hash--error {
  color: var(--danger);
  font-family: inherit;
  font-size: 12px;
}

.checksum-copy {
  display: inline-flex;
  align-items: center;
  gap: 5px;
  flex-shrink: 0;
  align-self: flex-start;
  height: 26px;
  padding: 0 10px;
  border-radius: 7px;
  border: 1px solid var(--control-border);
  background: var(--control-bg);
  color: var(--text-muted);
  font-size: 11px;
  font-weight: 600;
  transition: background 100ms ease, color 100ms ease;
}

.checksum-copy:hover {
  background: var(--btn-hover);
  color: var(--text);
}

.checksum-verify {
  display: flex;
  flex-direction: column;
  gap: 8px;
}

.checksum-verify-label {
  color: var(--text-muted);
  font-size: 12px;
  font-weight: 600;
}

.checksum-verify-field {
  display: flex;
  align-items: center;
  gap: 9px;
  min-height: 38px;
  padding: 0 10px 0 12px;
  border: 1px solid var(--input-border);
  border-radius: 10px;
  background: var(--input-bg);
  box-shadow: var(--input-shadow);
  transition: border-color 120ms ease, box-shadow 120ms ease;
}

.checksum-verify-field:focus-within {
  border-color: var(--accent-border);
  box-shadow: var(--input-shadow), var(--accent-focus-ring);
}

.checksum-verify-field--match {
  border-color: color-mix(in srgb, var(--success) 60%, transparent);
}

.checksum-verify-field--mismatch {
  border-color: rgb(var(--danger-rgb) / 0.6);
}

.checksum-verify-field input {
  width: 100%;
  min-width: 0;
  border: 0;
  outline: 0;
  background: transparent;
  color: var(--text);
  font-family: ui-monospace, SFMono-Regular, Menlo, monospace;
  font-size: 12px;
  font-weight: 500;
}

.checksum-verify-field input::placeholder {
  color: var(--text-faint);
  font-family: inherit;
}

.checksum-verify-badge {
  display: inline-flex;
  align-items: center;
  gap: 4px;
  flex-shrink: 0;
  font-size: 11px;
  font-weight: 700;
  letter-spacing: 0.01em;
}

.checksum-verify-badge--match {
  color: var(--success);
}

.checksum-verify-badge--mismatch {
  color: var(--danger);
}

.checksum-fade-enter-active {
  transition: opacity 180ms ease;
}
.checksum-fade-leave-active {
  transition: opacity 140ms ease;
}
.checksum-fade-enter-active .checksum-panel {
  transition: transform 220ms cubic-bezier(0.2, 0, 0, 1), opacity 180ms ease;
}
.checksum-fade-leave-active .checksum-panel {
  transition: transform 140ms ease, opacity 120ms ease;
}
.checksum-fade-enter-from,
.checksum-fade-leave-to {
  opacity: 0;
}
.checksum-fade-enter-from .checksum-panel,
.checksum-fade-leave-to .checksum-panel {
  opacity: 0;
  transform: scale(0.97) translateY(8px);
}
</style>
