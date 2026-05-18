<script setup>
import { computed, onMounted, onUnmounted, ref } from 'vue';
import AppIcon from './AppIcon.vue';
import { useFileManagerStore } from '../stores/fileManagerStore';

const store = useFileManagerStore();
const rootRef = ref(null);
const open = ref(false);

const runningJobs = computed(() =>
  store.queue.filter((job) => ['running', 'cancelling'].includes(job.status)),
);
const hasWork = computed(() => store.queue.length > 0);
const activeCount = computed(() => runningJobs.value.length);
const aggregateProgress = computed(() => {
  const measurableJobs = store.queue.filter((job) => typeof job.progress === 'number');

  if (measurableJobs.length === 0) {
    return null;
  }

  const total = measurableJobs.reduce((sum, job) => sum + job.progress, 0);
  return Math.max(0, Math.min(1, total / measurableJobs.length));
});

function toggleOpen() {
  open.value = !open.value;
}

function close() {
  open.value = false;
}

function handleWindowPointerDown(event) {
  if (!rootRef.value?.contains(event.target)) {
    close();
  }
}

function handleKeydown(event) {
  if (event.key === 'Escape') {
    close();
  }
}

function formatPercent(job) {
  if (typeof job?.progress !== 'number') {
    return '';
  }

  return `${Math.round(job.progress * 100)}%`;
}

function formatBytes(value) {
  const bytes = Number(value || 0);

  if (bytes >= 1024 ** 3) {
    return `${(bytes / 1024 ** 3).toFixed(1)} GB`;
  }

  if (bytes >= 1024 ** 2) {
    return `${(bytes / 1024 ** 2).toFixed(1)} MB`;
  }

  if (bytes >= 1024) {
    return `${(bytes / 1024).toFixed(1)} KB`;
  }

  return `${bytes} B`;
}

function jobStatus(job) {
  if (job.status === 'completed') {
    return 'Done';
  }

  if (job.status === 'failed') {
    return 'Failed';
  }

  if (job.status === 'cancelled') {
    return 'Cancelled';
  }

  if (job.status === 'cancelling') {
    return 'Cancelling';
  }

  return formatPercent(job) || 'Working';
}

function jobDetail(job) {
  if (job.detail) {
    return job.detail;
  }

  if (job.totalBytes > 0) {
    return `${formatBytes(job.processedBytes)} of ${formatBytes(job.totalBytes)}`;
  }

  if (job.totalEntries > 0) {
    return `${job.processedEntries} of ${job.totalEntries} entries`;
  }

  return job.currentPath || 'Preparing operation';
}

function canCancel(job) {
  return job.cancelable && ['running', 'cancelling'].includes(job.status) && !job.cancelRequested;
}

function cancelJob(job) {
  store.cancelQueueJob(job.id);
}

onMounted(() => {
  window.addEventListener('pointerdown', handleWindowPointerDown);
  window.addEventListener('keydown', handleKeydown);
});

onUnmounted(() => {
  window.removeEventListener('pointerdown', handleWindowPointerDown);
  window.removeEventListener('keydown', handleKeydown);
});
</script>

<template>
  <div ref="rootRef" class="work-indicator" @mousedown.stop @dblclick.stop>
    <button
      v-tooltip="hasWork ? 'Show current work' : 'No current work'"
      type="button"
      class="icon-btn work-button"
      :class="{ active: hasWork, 'work-button--open': open }"
      aria-label="Current work"
      :aria-expanded="open"
      @click="toggleOpen"
    >
      <span class="work-ring" :class="{ 'work-ring--active': activeCount > 0 }">
        <AppIcon name="work-queue" :size="18" :stroke-width="1.85" />
      </span>
      <span v-if="hasWork" class="work-badge">{{ activeCount || store.queue.length }}</span>
      <span
        v-if="aggregateProgress !== null"
        class="work-progress"
        :style="{ transform: `scaleX(${Math.max(0.06, aggregateProgress)})` }"
      ></span>
    </button>

    <div
      v-if="open"
      class="work-popover"
      role="dialog"
      aria-label="Current work"
      @mousedown.stop
      @dblclick.stop
    >
      <header class="work-popover-header">
        <div>
          <strong>Current Work</strong>
          <span>{{ hasWork ? `${store.queue.length} task${store.queue.length === 1 ? '' : 's'}` : 'Idle' }}</span>
        </div>
        <button type="button" class="work-close" aria-label="Close current work" @click="close">
          <AppIcon name="x" :size="13" :stroke-width="2.2" />
        </button>
      </header>

      <div v-if="hasWork" class="work-list">
        <article
          v-for="job in store.queue"
          :key="job.id"
          class="work-job"
          :class="`work-job--${job.status}`"
        >
          <div class="work-job-main">
            <div class="work-job-title">
              <span>{{ job.label }}</span>
              <small>{{ jobStatus(job) }}</small>
            </div>
            <p>{{ jobDetail(job) }}</p>
          </div>

          <button
            type="button"
            class="work-cancel"
            :disabled="!canCancel(job)"
            :aria-label="`Cancel ${job.label}`"
            @click="cancelJob(job)"
          >
            <AppIcon name="x" :size="12" :stroke-width="2.4" />
          </button>

          <div
            class="work-job-progress"
            :class="{ 'work-job-progress--indeterminate': job.progress === null && ['running', 'cancelling'].includes(job.status) }"
          >
            <span
              :style="{ width: job.progress === null ? '38%' : `${Math.max(4, job.progress * 100)}%` }"
            ></span>
          </div>
        </article>
      </div>

      <div v-else class="work-empty">
        <AppIcon name="work-queue" :size="18" :stroke-width="1.9" />
        <span>No file operations are running.</span>
      </div>
    </div>
  </div>
</template>

<style scoped>
.work-indicator {
  position: relative;
  display: inline-flex;
  align-items: center;
  z-index: 20;
}

.work-button {
  position: relative;
  display: inline-flex;
  width: 31px;
  height: 34px;
  align-items: center;
  justify-content: center;
  overflow: hidden;
  border-radius: 6px;
  background: transparent;
  color: var(--icon);
  cursor: pointer;
  transition: background 80ms ease, color 80ms ease;
}

.work-button:hover,
.work-button--open {
  background: var(--btn-hover);
  color: var(--text-muted);
}

.work-button:active {
  background: var(--btn-active-bg);
}

.work-button.active {
  color: var(--accent);
  background: rgb(var(--accent-rgb) / 0.13);
}

.work-ring {
  position: relative;
  z-index: 1;
  display: grid;
  place-items: center;
}

.work-ring--active {
  animation: work-spin 1.2s linear infinite;
}

.work-badge {
  position: absolute;
  z-index: 2;
  top: 2px;
  right: 2px;
  min-width: 13px;
  height: 13px;
  border-radius: 999px;
  padding: 0 4px;
  background: var(--accent);
  color: #fff;
  font-size: 9px;
  font-weight: 740;
  line-height: 13px;
}

.work-progress {
  position: absolute;
  right: 0;
  bottom: 0;
  left: 0;
  height: 2px;
  background: var(--accent);
  transform-origin: left center;
}

.work-popover {
  position: absolute;
  z-index: 2400;
  top: calc(100% + 10px);
  left: 50%;
  display: grid;
  width: min(380px, calc(100vw - 24px));
  max-height: min(460px, calc(100vh - 76px));
  overflow: hidden;
  border: 1px solid var(--control-border);
  border-radius: 12px;
  background: var(--popover-bg);
  box-shadow: var(--shadow-overlay);
  color: var(--text);
  transform: translateX(-50%);
}

.work-popover-header {
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: 12px;
  border-bottom: 1px solid var(--hairline);
  padding: 11px 12px 10px;
}

.work-popover-header div {
  display: grid;
  gap: 2px;
  min-width: 0;
}

.work-popover-header strong {
  font-size: 13px;
  font-weight: 700;
}

.work-popover-header span {
  color: var(--text-faint);
  font-size: 11px;
  font-weight: 560;
}

.work-close,
.work-cancel {
  display: inline-flex;
  width: 24px;
  height: 24px;
  align-items: center;
  justify-content: center;
  border-radius: 6px;
  background: transparent;
  color: var(--icon);
  cursor: pointer;
  transition: background 80ms ease, color 80ms ease;
}

.work-close:hover,
.work-cancel:hover:not(:disabled) {
  background: var(--btn-hover);
  color: var(--text-muted);
}

.work-close:active,
.work-cancel:active:not(:disabled) {
  background: var(--btn-active-bg);
}

.work-list {
  display: grid;
  gap: 8px;
  overflow: auto;
  padding: 10px;
}

.work-job {
  position: relative;
  display: grid;
  grid-template-columns: minmax(0, 1fr) 24px;
  gap: 8px;
  overflow: hidden;
  border: 1px solid var(--hairline);
  border-radius: 9px;
  padding: 9px 9px 12px;
  background: color-mix(in srgb, var(--control-glass) 74%, transparent);
}

.work-job--failed {
  border-color: rgb(var(--danger-rgb) / 0.32);
}

.work-job--cancelled,
.work-job--cancelling {
  border-color: rgb(var(--warning-rgb) / 0.28);
}

.work-job-main {
  display: grid;
  gap: 4px;
  min-width: 0;
}

.work-job-title {
  display: flex;
  min-width: 0;
  align-items: center;
  justify-content: space-between;
  gap: 10px;
}

.work-job-title span,
.work-job p {
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.work-job-title span {
  font-size: 12.5px;
  font-weight: 670;
}

.work-job-title small {
  flex: 0 0 auto;
  color: var(--text-faint);
  font-size: 10.5px;
  font-weight: 680;
  text-transform: uppercase;
}

.work-job p {
  margin: 0;
  color: var(--text-muted);
  font-size: 11px;
  font-weight: 520;
}

.work-cancel {
  width: 24px;
  height: 24px;
  align-self: start;
}

.work-cancel:disabled {
  cursor: default;
  opacity: 0.35;
}

.work-job-progress {
  position: absolute;
  right: 8px;
  bottom: 6px;
  left: 8px;
  height: 4px;
  overflow: hidden;
  border-radius: 999px;
  background: color-mix(in srgb, var(--text-faint) 20%, transparent);
}

.work-job-progress span {
  position: absolute;
  inset: 0 auto 0 0;
  min-width: 18px;
  border-radius: inherit;
  background: var(--accent);
}

.work-job-progress--indeterminate span {
  animation: work-progress-slide 1s ease-in-out infinite;
}

.work-empty {
  display: flex;
  align-items: center;
  gap: 9px;
  padding: 16px 14px;
  color: var(--text-muted);
  font-size: 12px;
  font-weight: 560;
}

@keyframes work-spin {
  to {
    transform: rotate(360deg);
  }
}

@keyframes work-progress-slide {
  0% {
    transform: translateX(-100%);
  }

  100% {
    transform: translateX(270%);
  }
}
</style>
