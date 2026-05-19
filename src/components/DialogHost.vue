<script setup>
import { computed, nextTick, ref, watch } from 'vue';
import AppIcon from './AppIcon.vue';
import { useDialog } from '../composables/useDialog';

const dialog = useDialog();
const dialogPanel = ref(null);
const promptInput = ref(null);
const inputValue = ref('');
const checkboxValue = ref(false);

const activeDialog = computed(() => dialog.activeDialog.value);
const iconName = computed(() => {
  if (activeDialog.value?.icon) {
    return activeDialog.value.icon;
  }

  if (activeDialog.value?.variant === 'danger') {
    return 'alert';
  }

  return activeDialog.value?.type === 'prompt' ? 'file' : 'info';
});

watch(
  activeDialog,
  async (nextDialog) => {
    if (!nextDialog) {
      inputValue.value = '';
      return;
    }

    inputValue.value = nextDialog.inputValue || '';
    checkboxValue.value = Boolean(nextDialog.checkboxValue);
    await nextTick();

    if (nextDialog.type === 'prompt') {
      promptInput.value?.focus();
      promptInput.value?.select();
    } else {
      dialogPanel.value?.focus();
    }
  },
);

function confirmDialog() {
  const current = activeDialog.value;

  if (!current) {
    return;
  }

  if (current.type === 'choice') {
    const action = current.actions.find((item) => item.default);

    if (action) {
      resolveChoice(action);
    }

    return;
  }

  if (current.type === 'prompt') {
    const value = inputValue.value;

    if (current.inputRequired && !value.trim()) {
      promptInput.value?.focus();
      return;
    }

    dialog.resolve(value);
    return;
  }

  dialog.resolve(true);
}

function resolveChoice(action) {
  const current = activeDialog.value;

  if (!current) {
    return;
  }

  if (action.cancel) {
    dialog.resolve(null);
    return;
  }

  dialog.resolve({
    value: action.value,
    applyToAll: Boolean(current.checkboxLabel && checkboxValue.value),
  });
}

function cancelDialog() {
  const current = activeDialog.value;

  if (!current) {
    return;
  }

  dialog.resolve(current.type === 'prompt' || current.type === 'choice' ? null : false);
}

function handleKeydown(event) {
  if (event.key === 'Escape') {
    event.preventDefault();
    cancelDialog();
    return;
  }

  if (event.key === 'Enter' && (event.metaKey || event.ctrlKey || activeDialog.value?.type !== 'prompt')) {
    event.preventDefault();
    confirmDialog();
  }
}
</script>

<template>
  <Teleport to="body">
    <Transition name="dialog-fade">
      <div
        v-if="activeDialog"
        class="dialog-overlay"
        role="presentation"
        @pointerdown.self="cancelDialog"
        @keydown.stop="handleKeydown"
      >
        <section
          ref="dialogPanel"
          class="dialog-panel"
          :class="{
            'dialog-panel--danger': activeDialog.variant === 'danger',
            'dialog-panel--warning': activeDialog.variant === 'warning',
            'dialog-panel--wide': activeDialog.size === 'wide',
          }"
          role="dialog"
          aria-modal="true"
          :aria-labelledby="`dialog-title-${activeDialog.id}`"
          tabindex="-1"
        >
          <div class="dialog-content">
            <header class="dialog-header">
              <div class="dialog-title-row">
                <span class="dialog-icon" aria-hidden="true">
                  <AppIcon :name="iconName" :size="20" :stroke-width="1.9" />
                </span>
                <h2 :id="`dialog-title-${activeDialog.id}`">{{ activeDialog.title }}</h2>
              </div>
              <p v-if="activeDialog.message">{{ activeDialog.message }}</p>
              <small v-if="activeDialog.detail">{{ activeDialog.detail }}</small>
            </header>

            <label v-if="activeDialog.type === 'prompt'" class="dialog-input-row">
              <span v-if="activeDialog.inputLabel">{{ activeDialog.inputLabel }}</span>
              <input
                ref="promptInput"
                v-model="inputValue"
                :type="activeDialog.inputType || 'text'"
                :placeholder="activeDialog.inputPlaceholder"
                @keydown.enter.stop.prevent="confirmDialog"
              />
            </label>

            <dl v-if="activeDialog.facts.length > 0" class="dialog-facts">
              <div v-for="fact in activeDialog.facts" :key="fact.label">
                <dt :title="fact.label">{{ fact.label }}</dt>
                <dd
                  :class="{ 'dialog-fact--mono': fact.mono }"
                  :title="fact.value"
                >
                  {{ fact.value }}
                </dd>
              </div>
            </dl>

            <label v-if="activeDialog.checkboxLabel" class="dialog-checkbox">
              <input v-model="checkboxValue" type="checkbox" />
              <span>{{ activeDialog.checkboxLabel }}</span>
            </label>

            <footer
              v-if="activeDialog.type === 'choice'"
              class="dialog-actions dialog-actions--choice"
            >
              <button
                v-for="action in activeDialog.actions"
                :key="action.value || action.label"
                type="button"
                class="dialog-button"
                :class="{
                  'dialog-button--primary': action.primary,
                  'dialog-button--danger': action.destructive,
                }"
                @click="resolveChoice(action)"
              >
                {{ action.label }}
              </button>
            </footer>

            <footer v-else class="dialog-actions">
              <button
                v-if="activeDialog.showCancel"
                type="button"
                class="dialog-button"
                @click="cancelDialog"
              >
                {{ activeDialog.cancelLabel }}
              </button>
              <button
                type="button"
                class="dialog-button dialog-button--primary"
                :class="{ 'dialog-button--danger': activeDialog.destructive }"
                @click="confirmDialog"
              >
                {{ activeDialog.confirmLabel }}
              </button>
            </footer>
          </div>
        </section>
      </div>
    </Transition>
  </Teleport>
</template>

<style scoped>
/* ── Overlay ──────────────────────────────────────────────── */
.dialog-overlay {
  position: fixed;
  z-index: 5000;
  inset: 0;
  display: grid;
  place-items: center;
  padding: 28px;
  background: var(--overlay-bg);
  backdrop-filter: blur(6px);
  -webkit-backdrop-filter: blur(6px);
}

/* ── Panel ────────────────────────────────────────────────── */
.dialog-panel {
  display: block;
  width: min(380px, calc(100vw - 48px));
  border: 1px solid var(--control-border);
  border-radius: 11px;
  padding: 16px;
  background: var(--popover-bg);
  box-shadow: var(--shadow-overlay);
  color: var(--text);
  outline: 0;
}

.dialog-panel--wide {
  width: min(560px, calc(100vw - 48px));
}

/* ── Icon ─────────────────────────────────────────────────── */
.dialog-icon {
  display: grid;
  width: 24px;
  height: 24px;
  flex: 0 0 auto;
  place-items: center;
  border-radius: 0;
  background: transparent;
  color: var(--icon);
}

.dialog-panel--danger .dialog-icon {
  color: var(--danger);
}

.dialog-panel--warning .dialog-icon {
  color: var(--warning);
}

/* ── Content ──────────────────────────────────────────────── */
.dialog-content {
  display: grid;
  min-width: 0;
  gap: 16px;
}

.dialog-header {
  display: grid;
  gap: 6px;
}

.dialog-title-row {
  display: flex;
  min-width: 0;
  align-items: center;
  gap: 9px;
}

.dialog-header h2 {
  margin: 0;
  color: var(--text);
  font-size: 14.5px;
  font-weight: 700;
  letter-spacing: -0.01em;
}

.dialog-header p {
  margin: 0;
  color: var(--text-muted);
  font-size: 13px;
  font-weight: 480;
  line-height: 1.45;
  white-space: pre-line;
}

.dialog-header small {
  color: var(--text-faint);
  font-size: 11.5px;
  font-weight: 500;
  line-height: 1.4;
}

/* ── Prompt input ─────────────────────────────────────────── */
.dialog-input-row {
  display: grid;
  gap: 6px;
}

.dialog-input-row span {
  color: var(--text-muted);
  font-size: 11.5px;
  font-weight: 600;
  letter-spacing: 0.01em;
}

.dialog-input-row input {
  width: 100%;
  height: 34px;
  border: 1px solid var(--input-border);
  border-radius: 9px;
  padding: 0 11px;
  background: var(--input-bg);
  color: var(--text);
  font-size: 13px;
  outline: 0;
  box-shadow: var(--input-shadow);
  transition: border-color 120ms ease, box-shadow 120ms ease;
}

.dialog-input-row input:focus {
  border-color: var(--accent-border);
  box-shadow:
    var(--accent-focus-ring),
    var(--input-shadow);
}

/* ── Supporting details ───────────────────────────────────── */
.dialog-facts {
  display: grid;
  gap: 0;
  overflow: hidden;
  margin: 0;
  border: 1px solid var(--input-border);
  border-radius: 8px;
  background: var(--input-bg);
}

.dialog-facts div {
  display: grid;
  grid-template-columns: minmax(108px, 0.44fr) minmax(0, 1fr);
  gap: 10px;
  align-items: center;
  min-width: 0;
  padding: 7px 9px;
}

.dialog-facts div + div {
  border-top: 1px solid var(--input-border);
}

.dialog-facts dt,
.dialog-facts dd {
  min-width: 0;
  margin: 0;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
  font-size: 11.5px;
  line-height: 1.25;
}

.dialog-facts dt {
  color: var(--text-faint);
  font-weight: 700;
}

.dialog-facts dd {
  color: var(--text-muted);
  font-weight: 600;
  text-align: right;
}

.dialog-fact--mono {
  font-family: "SF Mono", ui-monospace, Menlo, Consolas, monospace;
  font-size: 11px;
  font-weight: 520;
}

.dialog-checkbox {
  display: inline-flex;
  align-items: center;
  gap: 8px;
  min-width: 0;
  color: var(--text-muted);
  font-size: 12px;
  font-weight: 600;
}

.dialog-checkbox input {
  width: 14px;
  height: 14px;
  margin: 0;
  accent-color: var(--accent);
}

/* ── Actions ──────────────────────────────────────────────── */
.dialog-actions {
  display: flex;
  justify-content: flex-end;
  gap: 10px;
}

.dialog-actions--choice {
  flex-wrap: wrap;
}

.dialog-button {
  min-width: 86px;
  height: 36px;
  border: 1px solid color-mix(in srgb, var(--text) 13%, transparent);
  border-radius: 999px;
  padding: 0 18px;
  background:
    linear-gradient(180deg, rgb(255 255 255 / 0.14), rgb(255 255 255 / 0.04)),
    color-mix(in srgb, var(--control-glass) 72%, transparent);
  color: var(--text);
  font-size: 13px;
  font-weight: 650;
  box-shadow:
    inset 0 1px 0 rgb(255 255 255 / 0.16),
    inset 0 -1px 0 rgb(0 0 0 / 0.22),
    0 1px 2px rgb(0 0 0 / 0.24);
  transition:
    background 100ms ease,
    border-color 100ms ease,
    box-shadow 100ms ease,
    transform 80ms ease;
}

.dialog-button:hover {
  border-color: color-mix(in srgb, var(--text) 20%, transparent);
  background:
    linear-gradient(180deg, rgb(255 255 255 / 0.18), rgb(255 255 255 / 0.06)),
    color-mix(in srgb, var(--control-glass) 82%, transparent);
}

.dialog-button:active {
  transform: translateY(1px);
  box-shadow:
    inset 0 1px 2px rgb(0 0 0 / 0.22),
    0 1px 1px rgb(0 0 0 / 0.18);
}

.dialog-button--primary {
  border-color: rgb(var(--accent-rgb) / 0.58);
  background:
    linear-gradient(180deg, rgb(72 176 255), rgb(0 113 242));
  color: rgb(255 255 255 / 0.96);
  box-shadow:
    inset 0 1px 0 rgb(255 255 255 / 0.34),
    inset 0 -1px 0 rgb(0 48 120 / 0.35),
    0 0 0 1px rgb(var(--accent-rgb) / 0.18),
    0 4px 14px rgb(var(--accent-rgb) / 0.32);
}

.dialog-button--primary:hover {
  background:
    linear-gradient(180deg, rgb(91 188 255), rgb(0 123 255));
}

.dialog-button--danger {
  border-color: rgb(var(--danger-rgb) / 0.45);
  background:
    linear-gradient(180deg, rgb(255 93 84), rgb(214 49 39));
  color: rgb(255 255 255 / 0.96);
  box-shadow:
    inset 0 1px 0 rgb(255 255 255 / 0.26),
    inset 0 -1px 0 rgb(95 0 0 / 0.30),
    0 4px 14px rgb(var(--danger-rgb) / 0.26);
}

.dialog-button--danger:hover {
  background:
    linear-gradient(180deg, rgb(255 105 96), rgb(226 58 47));
}

/* ── Enter/leave animation ────────────────────────────────── */
.dialog-fade-enter-active {
  transition: opacity 180ms ease;
}

.dialog-fade-leave-active {
  transition: opacity 140ms ease;
}

.dialog-fade-enter-active .dialog-panel {
  transition: transform 220ms cubic-bezier(0.2, 0, 0, 1), opacity 180ms ease;
}

.dialog-fade-leave-active .dialog-panel {
  transition: transform 140ms ease, opacity 120ms ease;
}

.dialog-fade-enter-from,
.dialog-fade-leave-to {
  opacity: 0;
}

.dialog-fade-enter-from .dialog-panel,
.dialog-fade-leave-to .dialog-panel {
  opacity: 0;
  transform: scale(0.97) translateY(6px);
}
</style>
