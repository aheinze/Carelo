<script setup>
import { computed, onMounted, onUnmounted, ref, watch } from 'vue';
import AppIcon from './AppIcon.vue';
import { usePermissionsDialog } from '../composables/usePermissionsDialog';
import { getFileMetadata, setPermissions } from '../composables/useFileOperations';
import { useFileManagerStore } from '../stores/fileManagerStore';

const dialog = usePermissionsDialog();
const store = useFileManagerStore();

const loading = ref(false);
const applying = ref(false);
const ready = ref(false);
const error = ref('');
const mode = ref(0); // 0..0o7777
const ownerName = ref('');
const groupName = ref('');
const uid = ref(null);
const gid = ref(null);
const recursive = ref(false);
const octalInput = ref('0000');
let loadToken = 0;

// rows × columns of the rwx matrix.
const CLASSES = [
  { key: 'owner', label: 'Owner', shift: 6 },
  { key: 'group', label: 'Group', shift: 3 },
  { key: 'others', label: 'Others', shift: 0 },
];
const PERMS = [
  { key: 'read', label: 'Read', bit: 4 },
  { key: 'write', label: 'Write', bit: 2 },
  { key: 'execute', label: 'Execute', bit: 1 },
];
const SPECIAL = [
  { key: 'setuid', label: 'Setuid', bit: 0o4000 },
  { key: 'setgid', label: 'Setgid', bit: 0o2000 },
  { key: 'sticky', label: 'Sticky', bit: 0o1000 },
];

const target = computed(() => dialog.target.value);
const isDirectory = computed(() => Boolean(target.value?.isDirectory));
const isRemote = computed(() => Boolean(target.value?.isRemote));

function hasPerm(cls, perm) {
  return (mode.value & (perm.bit << cls.shift)) !== 0;
}

function togglePerm(cls, perm) {
  mode.value ^= perm.bit << cls.shift;
}

function hasSpecial(special) {
  return (mode.value & special.bit) !== 0;
}

function toggleSpecial(special) {
  mode.value ^= special.bit;
}

const octalString = computed(() => mode.value.toString(8).padStart(4, '0'));

// ls -l style preview, with s/S/t/T for the special bits.
const symbolic = computed(() => {
  const m = mode.value;
  const triad = (shift, specialBit, specialChar) => {
    const r = m & (4 << shift) ? 'r' : '-';
    const w = m & (2 << shift) ? 'w' : '-';
    const executable = (m & (1 << shift)) !== 0;
    let x = executable ? 'x' : '-';

    if (specialBit && m & specialBit) {
      x = executable ? specialChar.toLowerCase() : specialChar.toUpperCase();
    }

    return `${r}${w}${x}`;
  };

  const typeChar = isDirectory.value ? 'd' : '-';
  return typeChar + triad(6, 0o4000, 's') + triad(3, 0o2000, 's') + triad(0, 0o1000, 't');
});

// A directory with no owner-execute can't be entered — worth warning about.
const directoryNotTraversable = computed(
  () => isDirectory.value && (mode.value & 0o100) === 0,
);

const ownerLabel = computed(() => labelFor(ownerName.value, uid.value));
const groupLabel = computed(() => labelFor(groupName.value, gid.value));

function labelFor(name, id) {
  const trimmed = String(name || '').trim();

  if (trimmed && id !== null && id !== undefined) {
    return `${trimmed} (${id})`;
  }

  return trimmed || (id !== null && id !== undefined ? String(id) : '—');
}

function commitOctal() {
  const cleaned = octalInput.value.replace(/[^0-7]/g, '');
  const parsed = cleaned ? Number.parseInt(cleaned, 8) : 0;

  if (Number.isFinite(parsed)) {
    mode.value = parsed & 0o7777;
  }

  octalInput.value = octalString.value;
}

watch(mode, () => {
  octalInput.value = octalString.value;
});

async function load(path) {
  const token = ++loadToken;
  loading.value = true;
  ready.value = false;
  error.value = '';
  recursive.value = false;

  try {
    const meta = await getFileMetadata(path);

    if (token !== loadToken) {
      return;
    }

    const perms = meta?.permissions;

    if (!perms) {
      error.value = 'POSIX permissions are not available for this item.';
      return;
    }

    mode.value = (perms.mode ?? 0) & 0o7777;
    octalInput.value = octalString.value;
    ownerName.value = perms.ownerName || '';
    groupName.value = perms.groupName || '';
    uid.value = perms.uid ?? null;
    gid.value = perms.gid ?? null;
    ready.value = true;
  } catch (loadError) {
    if (token === loadToken) {
      error.value = loadError?.message || 'Unable to read permissions.';
    }
  } finally {
    if (token === loadToken) {
      loading.value = false;
    }
  }
}

async function apply() {
  const current = target.value;

  if (!current || applying.value || loading.value || !ready.value) {
    return;
  }

  applying.value = true;
  error.value = '';

  try {
    const applyRecursive = isDirectory.value && recursive.value && !isRemote.value;
    await setPermissions(current.path, mode.value, applyRecursive);

    // Refresh the containing directory so the new permissions are reflected.
    const parent =
      current.path.replace(/\/+$/, '').split('/').slice(0, -1).join('/') || '/';
    await store.reloadDirectoryInPanes(parent).catch(() => {});
    dialog.close();
  } catch (applyError) {
    error.value = applyError?.message || 'Unable to change permissions.';
  } finally {
    applying.value = false;
  }
}

function close() {
  if (!applying.value) {
    dialog.close();
  }
}

function onKeydown(event) {
  if (event.key === 'Escape') {
    event.stopPropagation();
    close();
    return;
  }

  if (event.key === 'Enter') {
    // Let the octal text field commit its typed value (via its own handler)
    // rather than applying the not-yet-committed mode.
    const el = event.target;
    if (el && el.tagName === 'INPUT' && el.type === 'text') {
      return;
    }

    if (ready.value && !applying.value && !loading.value) {
      event.stopPropagation();
      apply();
    }
  }
}

watch(
  () => dialog.visible.value,
  (visible) => {
    if (visible && target.value) {
      load(target.value.path);
    }
  },
  { immediate: true },
);

onMounted(() => window.addEventListener('keydown', onKeydown, true));
onUnmounted(() => window.removeEventListener('keydown', onKeydown, true));
</script>

<template>
  <Teleport to="body">
    <Transition name="perm-fade">
      <div
        v-if="dialog.visible.value"
        class="perm-overlay"
        role="dialog"
        aria-modal="true"
        aria-label="Edit permissions"
        @pointerdown.self="close"
      >
        <div class="perm-panel">
          <header class="perm-header">
            <div class="perm-title-group">
              <AppIcon name="lock" :size="17" :stroke-width="1.8" />
              <h2>Permissions</h2>
            </div>
            <button type="button" class="perm-close" aria-label="Close" @click="close">
              <AppIcon name="x" :size="14" :stroke-width="2" />
            </button>
          </header>

          <div class="perm-subject" :title="target?.path">
            <AppIcon :name="isDirectory ? 'folder' : 'file'" :size="15" :stroke-width="1.7" />
            <span class="perm-subject-name">{{ target?.name }}</span>
          </div>

          <div v-if="loading" class="perm-status">Loading permissions…</div>

          <div v-else-if="!ready" class="perm-error">
            {{ error || 'Permissions are not available for this item.' }}
          </div>

          <template v-else>
            <div class="perm-ownership">
              <div class="perm-owner-cell">
                <span class="perm-owner-label">Owner</span>
                <span class="perm-owner-value">{{ ownerLabel }}</span>
              </div>
              <div class="perm-owner-cell">
                <span class="perm-owner-label">Group</span>
                <span class="perm-owner-value">{{ groupLabel }}</span>
              </div>
            </div>

            <div class="perm-matrix" role="group" aria-label="Permission matrix">
              <div class="perm-matrix-head">
                <span></span>
                <span v-for="perm in PERMS" :key="perm.key">{{ perm.label }}</span>
              </div>
              <div v-for="cls in CLASSES" :key="cls.key" class="perm-matrix-row">
                <span class="perm-row-label">{{ cls.label }}</span>
                <div v-for="perm in PERMS" :key="perm.key" class="perm-cell">
                  <label class="perm-switch-wrap">
                    <input
                      type="checkbox"
                      class="perm-switch-input"
                      :checked="hasPerm(cls, perm)"
                      :aria-label="`${cls.label} ${perm.label}`"
                      @change="togglePerm(cls, perm)"
                    />
                    <span class="perm-switch" aria-hidden="true"></span>
                  </label>
                </div>
              </div>
            </div>

            <div class="perm-special">
              <span class="perm-special-label">Special</span>
              <div class="perm-special-row">
                <label v-for="special in SPECIAL" :key="special.key" class="perm-special-item">
                  <input
                    type="checkbox"
                    class="perm-switch-input"
                    :checked="hasSpecial(special)"
                    :aria-label="special.label"
                    @change="toggleSpecial(special)"
                  />
                  <span class="perm-switch" aria-hidden="true"></span>
                  <span class="perm-special-name">{{ special.label }}</span>
                </label>
              </div>
            </div>

            <div class="perm-readout">
              <code class="perm-symbolic">{{ symbolic }}</code>
              <div class="perm-octal-field">
                <span>Octal</span>
                <input
                  v-model="octalInput"
                  type="text"
                  inputmode="numeric"
                  spellcheck="false"
                  maxlength="4"
                  aria-label="Octal permissions"
                  @blur="commitOctal"
                  @keydown.enter.prevent="commitOctal"
                />
              </div>
            </div>

            <label v-if="isDirectory && !isRemote" class="perm-recursive">
              <input type="checkbox" v-model="recursive" />
              <span class="perm-recursive-copy">
                <strong>Apply to enclosed items</strong>
                <small>Folders keep their search bit so the tree stays browsable.</small>
              </span>
            </label>

            <div v-if="directoryNotTraversable" class="perm-warning">
              <AppIcon name="alert" :size="14" :stroke-width="2" />
              <span>Without owner Execute, this folder can't be opened or browsed.</span>
            </div>

            <div v-if="error" class="perm-error">{{ error }}</div>
          </template>

          <footer class="perm-footer">
            <button type="button" class="perm-btn" :disabled="applying" @click="close">
              Cancel
            </button>
            <button
              type="button"
              class="perm-btn perm-btn--primary"
              :disabled="!ready || applying"
              @click="apply"
            >
              {{ applying ? 'Applying…' : 'Apply' }}
            </button>
          </footer>
        </div>
      </div>
    </Transition>
  </Teleport>
</template>

<style scoped>
.perm-overlay {
  position: fixed;
  z-index: 5000;
  inset: 0;
  display: grid;
  place-items: center;
  padding: 32px;
  background: var(--overlay-bg);
}

.perm-panel {
  display: flex;
  flex-direction: column;
  gap: 14px;
  width: min(420px, calc(100vw - 48px));
  max-height: calc(100vh - 80px);
  overflow: hidden auto;
  border: 1px solid var(--control-border);
  border-radius: var(--radius-panel);
  padding: 20px 22px 18px;
  background: var(--modal-bg);
  box-shadow: var(--shadow-overlay);
}

.perm-header {
  display: flex;
  align-items: center;
  justify-content: space-between;
}

.perm-title-group {
  display: flex;
  align-items: center;
  gap: 9px;
  color: var(--text);
}

.perm-header h2 {
  margin: 0;
  font-size: 15px;
  font-weight: 700;
  letter-spacing: -0.01em;
}

.perm-close {
  display: grid;
  width: 26px;
  height: 26px;
  place-items: center;
  border-radius: 7px;
  background: transparent;
  color: var(--icon);
  transition: background 100ms ease, color 100ms ease;
}

.perm-close:hover {
  background: var(--btn-hover);
  color: var(--text);
}

.perm-subject {
  display: flex;
  align-items: center;
  gap: 8px;
  min-width: 0;
  color: var(--text);
}

.perm-subject-name {
  overflow: hidden;
  font-size: 13px;
  font-weight: 600;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.perm-status {
  padding: 18px 0;
  color: var(--text-faint);
  font-size: 13px;
  text-align: center;
}

.perm-ownership {
  display: grid;
  grid-template-columns: 1fr 1fr;
  gap: 8px;
}

.perm-owner-cell {
  display: flex;
  flex-direction: column;
  gap: 2px;
  padding: 8px 11px;
  border-radius: 9px;
  border: 1px solid var(--hairline);
  background: color-mix(in srgb, var(--text) 3.5%, transparent);
}

.perm-owner-label {
  color: var(--text-faint);
  font-size: 10px;
  font-weight: 700;
  letter-spacing: 0.04em;
  text-transform: uppercase;
}

.perm-owner-value {
  overflow: hidden;
  color: var(--text);
  font-size: 12.5px;
  font-weight: 600;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.perm-matrix {
  display: flex;
  flex-direction: column;
  gap: 6px;
}

.perm-matrix-head,
.perm-matrix-row {
  display: grid;
  grid-template-columns: 72px repeat(3, 1fr);
  align-items: center;
  gap: 8px;
}

.perm-matrix-head span {
  color: var(--text-faint);
  font-size: 10.5px;
  font-weight: 700;
  letter-spacing: 0.03em;
  text-align: center;
  text-transform: uppercase;
}

.perm-matrix-head span:first-child {
  text-align: left;
}

.perm-row-label {
  color: var(--text-muted);
  font-size: 12.5px;
  font-weight: 600;
}

.perm-cell {
  display: grid;
  place-items: center;
  height: 26px;
}

/* Compact toggle switch, matching the app's switch styling. */
.perm-switch-wrap {
  display: inline-flex;
  cursor: pointer;
}

.perm-switch-input {
  position: absolute;
  width: 1px;
  height: 1px;
  overflow: hidden;
  clip: rect(0 0 0 0);
  clip-path: inset(50%);
  white-space: nowrap;
}

.perm-switch {
  position: relative;
  display: block;
  width: 34px;
  height: 20px;
  flex: 0 0 34px;
  border: 1px solid var(--input-border);
  border-radius: 999px;
  background: color-mix(in srgb, var(--text) 9%, transparent);
  box-shadow: var(--input-shadow);
  cursor: pointer;
  transition: background 130ms ease, border-color 130ms ease;
}

.perm-switch::after {
  position: absolute;
  top: 2px;
  left: 2px;
  width: 14px;
  height: 14px;
  border-radius: 50%;
  background: color-mix(in srgb, var(--text) 70%, transparent);
  box-shadow: 0 1px 3px rgb(0 0 0 / 0.26);
  content: "";
  transition: transform 130ms ease, background 130ms ease;
}

.perm-switch-input:checked + .perm-switch {
  border-color: var(--accent-border);
  background: var(--accent);
}

.perm-switch-input:checked + .perm-switch::after {
  background: #ffffff;
  transform: translateX(16px);
}

.perm-switch-input:focus-visible + .perm-switch {
  box-shadow: var(--accent-focus-ring), var(--input-shadow);
}

.perm-special {
  display: flex;
  align-items: center;
  flex-wrap: wrap;
  gap: 10px 16px;
}

.perm-special-label {
  color: var(--text-faint);
  font-size: 10.5px;
  font-weight: 700;
  letter-spacing: 0.03em;
  text-transform: uppercase;
}

.perm-special-row {
  display: flex;
  align-items: center;
  flex-wrap: wrap;
  gap: 10px 16px;
}

.perm-special-item {
  display: inline-flex;
  align-items: center;
  gap: 8px;
  color: var(--text-muted);
  font-size: 12.5px;
  font-weight: 600;
  cursor: pointer;
}

.perm-special-name {
  user-select: none;
}

.perm-readout {
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: 12px;
  padding: 10px 12px;
  border-radius: 10px;
  border: 1px solid var(--hairline);
  background: color-mix(in srgb, var(--text) 3.5%, transparent);
}

.perm-symbolic {
  font-family: ui-monospace, SFMono-Regular, Menlo, monospace;
  font-size: 14px;
  font-weight: 600;
  letter-spacing: 0.06em;
  color: var(--text);
}

.perm-octal-field {
  display: flex;
  align-items: center;
  gap: 8px;
  color: var(--text-faint);
  font-size: 11px;
  font-weight: 700;
  letter-spacing: 0.03em;
  text-transform: uppercase;
}

.perm-octal-field input {
  width: 56px;
  padding: 5px 8px;
  border: 1px solid var(--input-border);
  border-radius: 8px;
  background: var(--input-bg);
  color: var(--text);
  font-family: ui-monospace, SFMono-Regular, Menlo, monospace;
  font-size: 13px;
  font-weight: 600;
  letter-spacing: 0.04em;
  text-align: center;
}

.perm-octal-field input:focus {
  outline: 0;
  border-color: var(--accent-border);
  box-shadow: var(--accent-focus-ring);
}

.perm-recursive {
  display: flex;
  align-items: center;
  gap: 10px;
  cursor: pointer;
}

.perm-recursive input {
  flex-shrink: 0;
  width: 16px;
  height: 16px;
  margin: 0;
  accent-color: var(--accent);
  cursor: pointer;
}

.perm-recursive-copy {
  display: flex;
  flex-direction: column;
  gap: 1px;
}

.perm-recursive-copy strong {
  color: var(--text);
  font-size: 12.5px;
  font-weight: 600;
}

.perm-recursive-copy small {
  color: var(--text-faint);
  font-size: 11px;
}

.perm-warning {
  display: flex;
  align-items: center;
  gap: 8px;
  padding: 9px 12px;
  border-radius: 9px;
  border: 1px solid color-mix(in srgb, var(--warning, #d29922) 45%, transparent);
  background: color-mix(in srgb, var(--warning, #d29922) 14%, transparent);
  color: var(--warning, #d29922);
  font-size: 12px;
  font-weight: 600;
}

.perm-error {
  padding: 9px 12px;
  border-radius: 9px;
  border: 1px solid rgb(var(--danger-rgb) / 0.4);
  background: color-mix(in srgb, var(--danger) 14%, transparent);
  color: var(--danger);
  font-size: 12px;
  font-weight: 600;
}

.perm-footer {
  display: flex;
  justify-content: flex-end;
  gap: 9px;
  margin-top: 2px;
}

.perm-btn {
  height: 32px;
  padding: 0 16px;
  border-radius: 8px;
  border: 1px solid var(--control-border);
  background: var(--control-bg);
  color: var(--text);
  font-size: 12.5px;
  font-weight: 600;
  cursor: pointer;
  transition: background 110ms ease, border-color 110ms ease;
}

.perm-btn:hover:not(:disabled) {
  background: var(--btn-hover);
}

.perm-btn:disabled {
  opacity: 0.55;
  cursor: default;
}

.perm-btn--primary {
  border-color: var(--accent-border);
  background: var(--accent);
  color: var(--accent-contrast, #fff);
}

.perm-btn--primary:hover:not(:disabled) {
  background: var(--accent);
  filter: brightness(1.05);
}

.perm-fade-enter-active {
  transition: opacity 180ms ease;
}
.perm-fade-leave-active {
  transition: opacity 140ms ease;
}
.perm-fade-enter-active .perm-panel {
  transition: transform 220ms cubic-bezier(0.2, 0, 0, 1), opacity 180ms ease;
}
.perm-fade-leave-active .perm-panel {
  transition: transform 140ms ease, opacity 120ms ease;
}
.perm-fade-enter-from,
.perm-fade-leave-to {
  opacity: 0;
}
.perm-fade-enter-from .perm-panel,
.perm-fade-leave-to .perm-panel {
  opacity: 0;
  transform: scale(0.97) translateY(8px);
}
</style>
