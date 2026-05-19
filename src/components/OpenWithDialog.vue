<script setup>
import { computed, nextTick, ref, watch } from 'vue';
import AppIcon from './AppIcon.vue';

const props = defineProps({
  visible: {
    type: Boolean,
    default: false,
  },
  entry: {
    type: Object,
    default: null,
  },
  context: {
    type: Object,
    default: null,
  },
  loading: {
    type: Boolean,
    default: false,
  },
  error: {
    type: String,
    default: '',
  },
});

const emit = defineEmits(['cancel', 'open', 'reveal']);

const panelRef = ref(null);
const selectedAppId = ref('');
const remember = ref(false);

const apps = computed(() => props.context?.apps || []);
const fileTypeLabel = computed(() => props.context?.fileType?.label || 'this file type');
const selectedApp = computed(() => apps.value.find((app) => app.id === selectedAppId.value) || null);
const canOpen = computed(() => Boolean(selectedAppId.value && !props.loading));

watch(
  () => props.visible,
  async (visible) => {
    if (!visible) {
      selectedAppId.value = '';
      remember.value = false;
      return;
    }

    await nextTick();
    panelRef.value?.focus();
  },
);

watch(
  apps,
  (nextApps) => {
    if (!props.visible) {
      return;
    }

    const remembered = nextApps.find((app) => app.isRememberedDefault);
    const systemDefault = nextApps.find((app) => app.isSystemDefault);
    const selected = remembered || systemDefault || nextApps[0];

    selectedAppId.value = selected?.id || '';
    remember.value = Boolean(remembered);
  },
  { immediate: true },
);

function cancel() {
  emit('cancel');
}

function reveal() {
  emit('reveal');
}

function open() {
  if (!canOpen.value) {
    return;
  }

  emit('open', {
    appId: selectedAppId.value,
    remember: Boolean(remember.value),
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
    open();
  }
}
</script>

<template>
  <Teleport to="body">
    <Transition name="open-with-dialog">
      <div
        v-if="visible"
        class="open-with-overlay"
        role="presentation"
        @pointerdown.self="cancel"
        @keydown.stop="handleKeydown"
      >
        <section
          ref="panelRef"
          class="open-with-panel"
          role="dialog"
          aria-modal="true"
          aria-labelledby="open-with-title"
          tabindex="-1"
        >
          <div class="open-with-content">
            <header class="open-with-header">
              <div class="open-with-title-row">
                <span class="open-with-icon" aria-hidden="true">
                  <AppIcon name="app" :size="20" :stroke-width="1.9" />
                </span>
                <h2 id="open-with-title">Open With</h2>
              </div>
              <p v-if="entry">{{ entry.name }}</p>
              <small>{{ fileTypeLabel }}</small>
            </header>

            <div v-if="loading" class="open-with-loading" aria-live="polite">
              <span v-for="index in 4" :key="index"></span>
            </div>

            <p v-else-if="error" class="open-with-error" role="alert">
              {{ error }}
            </p>

            <div v-else-if="apps.length > 0" class="open-with-apps" role="radiogroup" aria-label="Applications">
              <label
                v-for="app in apps"
                :key="app.id"
                class="open-with-app"
                :class="{ 'open-with-app--selected': selectedAppId === app.id }"
              >
                <input v-model="selectedAppId" type="radio" :value="app.id">
                <span class="open-with-app-icon" aria-hidden="true">
                  <AppIcon name="app" :size="17" :stroke-width="1.8" />
                </span>
                <span class="open-with-app-copy">
                  <strong>{{ app.name }}</strong>
                  <span>{{ app.description }}</span>
                </span>
                <span v-if="selectedAppId === app.id" class="open-with-app-check" aria-hidden="true">
                  <AppIcon name="check" :size="13" :stroke-width="2.6" />
                </span>
              </label>
            </div>

            <p v-else class="open-with-empty">No compatible apps were found.</p>

            <label class="open-with-switch" :class="{ 'open-with-switch--disabled': !selectedApp }">
              <span class="open-with-switch-copy">
                <strong>Always use for {{ fileTypeLabel }}</strong>
                <span v-if="selectedApp">Use {{ selectedApp.name }} when opening this file type.</span>
                <span v-else>Choose an app before saving a default.</span>
              </span>
              <input v-model="remember" class="switch-input" type="checkbox" :disabled="!selectedApp">
              <span class="settings-switch" aria-hidden="true"></span>
            </label>

            <footer class="open-with-actions">
              <button type="button" class="open-with-button open-with-button--subtle" @click="reveal">
                Reveal
              </button>
              <span class="open-with-action-spacer"></span>
              <button type="button" class="open-with-button" @click="cancel">
                Cancel
              </button>
              <button
                type="button"
                class="open-with-button open-with-button--primary"
                :disabled="!canOpen"
                @click="open"
              >
                Open
              </button>
            </footer>
          </div>
        </section>
      </div>
    </Transition>
  </Teleport>
</template>

<style scoped>
.open-with-overlay {
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

.open-with-panel {
  display: block;
  width: min(520px, calc(100vw - 48px));
  max-height: calc(100vh - 56px);
  overflow: hidden;
  border: 1px solid var(--control-border);
  border-radius: 11px;
  padding: 16px;
  background: var(--popover-bg);
  box-shadow: var(--shadow-overlay);
  color: var(--text);
  outline: 0;
}

.open-with-icon {
  display: grid;
  width: 24px;
  height: 24px;
  flex: 0 0 auto;
  place-items: center;
  color: var(--icon);
}

.open-with-content {
  display: grid;
  min-width: 0;
  min-height: 0;
  gap: 14px;
}

.open-with-header {
  display: grid;
  gap: 4px;
  min-width: 0;
}

.open-with-title-row {
  display: flex;
  min-width: 0;
  align-items: center;
  gap: 9px;
}

.open-with-header h2 {
  margin: 0;
  color: var(--text);
  font-size: 14.5px;
  font-weight: 700;
  letter-spacing: 0;
}

.open-with-header p,
.open-with-header small {
  overflow: hidden;
  margin: 0;
  color: var(--text-muted);
  font-size: 12.5px;
  font-weight: 560;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.open-with-header small {
  color: var(--text-faint);
  font-size: 11.5px;
}

.open-with-loading {
  display: grid;
  gap: 8px;
}

.open-with-loading span {
  height: 42px;
  border-radius: 8px;
  background: linear-gradient(90deg, transparent, rgb(255 255 255 / 0.08), transparent);
  background-color: var(--input-bg);
  background-size: 220% 100%;
  animation: open-with-loading 1100ms linear infinite;
}

@keyframes open-with-loading {
  from { background-position: 120% 0; }
  to { background-position: -120% 0; }
}

.open-with-error,
.open-with-empty {
  margin: 0;
  border: 1px solid var(--input-border);
  border-radius: 8px;
  padding: 12px;
  background: var(--input-bg);
  color: var(--text-muted);
  font-size: 12.5px;
  font-weight: 600;
  line-height: 1.4;
}

.open-with-apps {
  display: grid;
  max-height: min(320px, calc(100vh - 350px));
  min-height: 0;
  gap: 5px;
  overflow-y: auto;
  padding-right: 2px;
}

.open-with-app {
  display: grid;
  grid-template-columns: 30px minmax(0, 1fr) 16px;
  align-items: center;
  gap: 10px;
  min-width: 0;
  min-height: 44px;
  border: 1px solid transparent;
  border-radius: 8px;
  padding: 7px 8px;
  color: var(--text);
  cursor: pointer;
}

.open-with-app:hover {
  background: var(--btn-hover);
}

.open-with-app--selected {
  border-color: rgb(var(--accent-rgb) / 0.22);
  background: rgb(var(--accent-rgb) / 0.08);
}

.open-with-app input {
  position: absolute;
  opacity: 0;
  pointer-events: none;
}

.open-with-app-icon {
  display: grid;
  width: 30px;
  height: 30px;
  place-items: center;
  border-radius: 8px;
  background: rgb(var(--accent-rgb) / 0.10);
  color: var(--accent);
}

.open-with-app-copy {
  display: grid;
  min-width: 0;
  gap: 2px;
}

.open-with-app-copy strong,
.open-with-app-copy span {
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.open-with-app-copy strong {
  font-size: 13px;
  font-weight: 650;
}

.open-with-app-copy span {
  color: var(--text-faint);
  font-size: 11px;
  font-weight: 560;
}

.open-with-app-check {
  display: flex;
  justify-content: flex-end;
  color: var(--accent);
}

.open-with-switch {
  display: grid;
  grid-template-columns: minmax(0, 1fr) auto;
  align-items: center;
  gap: 12px;
  min-width: 0;
  border: 1px solid var(--input-border);
  border-radius: 9px;
  padding: 9px 10px;
  background: var(--input-bg);
  box-shadow: var(--input-shadow);
}

.open-with-switch--disabled {
  opacity: 0.58;
}

.open-with-switch-copy {
  display: grid;
  min-width: 0;
  gap: 3px;
}

.open-with-switch-copy strong,
.open-with-switch-copy span {
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.open-with-switch-copy strong {
  color: var(--text-muted);
  font-size: 12px;
  font-weight: 700;
}

.open-with-switch-copy span {
  color: var(--text-faint);
  font-size: 11px;
  font-weight: 560;
}

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

.open-with-actions {
  display: flex;
  align-items: center;
  gap: 10px;
}

.open-with-action-spacer {
  flex: 1;
}

.open-with-button {
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

.open-with-button:hover:not(:disabled) {
  border-color: color-mix(in srgb, var(--text) 20%, transparent);
  background:
    linear-gradient(180deg, rgb(255 255 255 / 0.18), rgb(255 255 255 / 0.06)),
    color-mix(in srgb, var(--control-glass) 82%, transparent);
}

.open-with-button:active:not(:disabled) {
  transform: translateY(1px);
  box-shadow:
    inset 0 1px 2px rgb(0 0 0 / 0.22),
    0 1px 1px rgb(0 0 0 / 0.18);
}

.open-with-button:disabled {
  cursor: default;
  opacity: 0.55;
}

.open-with-button--subtle {
  min-width: 78px;
  color: var(--text-muted);
}

.open-with-button--primary {
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

.open-with-button--primary:hover:not(:disabled) {
  background:
    linear-gradient(180deg, rgb(91 188 255), rgb(0 123 255));
}

.open-with-dialog-enter-active,
.open-with-dialog-leave-active {
  transition: opacity 120ms ease;
}

.open-with-dialog-enter-active .open-with-panel,
.open-with-dialog-leave-active .open-with-panel {
  transition: transform 120ms ease, opacity 120ms ease;
}

.open-with-dialog-enter-from,
.open-with-dialog-leave-to {
  opacity: 0;
}

.open-with-dialog-enter-from .open-with-panel,
.open-with-dialog-leave-to .open-with-panel {
  opacity: 0;
  transform: translateY(6px) scale(0.985);
}

@media (max-width: 560px) {
  .open-with-panel {
    padding: 14px;
  }

  .open-with-actions {
    flex-wrap: wrap;
  }

  .open-with-action-spacer {
    display: none;
  }
}

@media (prefers-reduced-motion: reduce) {
  .open-with-loading span,
  .open-with-dialog-enter-active,
  .open-with-dialog-leave-active,
  .open-with-dialog-enter-active .open-with-panel,
  .open-with-dialog-leave-active .open-with-panel {
    animation: none;
    transition: none;
  }
}
</style>
