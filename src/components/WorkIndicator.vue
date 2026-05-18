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
const activeJobs = computed(() =>
  store.queue.filter((job) => ['running', 'paused', 'cancelling'].includes(job.status)),
);
const hasWork = computed(() => store.queue.length > 0);
const activeCount = computed(() => activeJobs.value.length);
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

  if (job.status === 'paused') {
    return 'Paused';
  }

  return formatPercent(job) || 'Working';
}

function formatDuration(seconds) {
  const value = Number(seconds);

  if (!Number.isFinite(value) || value < 0) {
    return '';
  }

  if (value < 60) {
    return `${Math.max(1, Math.round(value))}s`;
  }

  const minutes = Math.floor(value / 60);
  const remainingSeconds = Math.round(value % 60);

  if (minutes < 60) {
    return remainingSeconds > 0 ? `${minutes}m ${remainingSeconds}s` : `${minutes}m`;
  }

  const hours = Math.floor(minutes / 60);
  const remainingMinutes = minutes % 60;
  return remainingMinutes > 0 ? `${hours}h ${remainingMinutes}m` : `${hours}h`;
}

function metricDetail(job) {
  const metrics = [];

  if (job.bytesPerSecond > 0) {
    metrics.push(`${formatBytes(job.bytesPerSecond)}/s`);
  }

  if (job.etaSeconds !== null && job.etaSeconds !== undefined) {
    metrics.push(`${formatDuration(job.etaSeconds)} left`);
  }

  return metrics.join(' · ');
}

function jobDetail(job) {
  const metric = metricDetail(job);

  if (job.detail) {
    return metric ? `${job.detail} · ${metric}` : job.detail;
  }

  if (job.totalBytes > 0) {
    const progress = `${formatBytes(job.processedBytes)} of ${formatBytes(job.totalBytes)}`;
    return metric ? `${progress} · ${metric}` : progress;
  }

  if (job.totalEntries > 0) {
    return `${job.processedEntries} of ${job.totalEntries} entries`;
  }

  return job.currentPath || 'Preparing operation';
}

function jobMeta(job) {
  const current = currentFileDetail(job);

  if (current && !job.detail) {
    return current;
  }

  if (current && job.detail) {
    return `${current}`;
  }

  return '';
}

function fileName(path) {
  const value = String(path || '').replace(/\/+$/, '');
  return value.split('/').filter(Boolean).at(-1) || value || 'Current item';
}

function currentFileDetail(job) {
  if (!job.currentPath) {
    return '';
  }

  if (job.currentTotalBytes > 0) {
    return `${fileName(job.currentPath)} · ${formatBytes(job.currentBytes)} of ${formatBytes(job.currentTotalBytes)}`;
  }

  return fileName(job.currentPath);
}

function canCancel(job) {
  return job.cancelable && ['running', 'paused', 'cancelling'].includes(job.status) && !job.cancelRequested;
}

function canPause(job) {
  return job.pausable && job.status === 'running';
}

function canResume(job) {
  return job.status === 'paused';
}

function canRetry(job) {
  return job.status === 'failed' && typeof job.retryAction === 'function';
}

function canDismiss(job) {
  return ['completed', 'failed', 'cancelled'].includes(job.status);
}

function cancelJob(job) {
  store.cancelQueueJob(job.id);
}

function pauseJob(job) {
  store.pauseQueueJob(job.id);
}

function resumeJob(job) {
  store.resumeQueueJob(job.id);
}

function retryJob(job) {
  store.retryQueueJob(job.id);
}

function dismissJob(job) {
  store.removeQueueJob(job.id);
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
      <span class="work-ring" :class="{ 'work-ring--active': runningJobs.length > 0 }">
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
            <p v-if="jobMeta(job)" class="work-job-meta" :title="job.currentPath">
              {{ jobMeta(job) }}
            </p>
          </div>

          <div class="work-actions" aria-label="Job actions">
            <button
              v-if="canPause(job)"
              v-tooltip="'Pause'"
              type="button"
              class="work-action"
              :aria-label="`Pause ${job.label}`"
              @click="pauseJob(job)"
            >
              <AppIcon name="pause" :size="12" :stroke-width="2.4" />
            </button>
            <button
              v-if="canResume(job)"
              v-tooltip="'Resume'"
              type="button"
              class="work-action"
              :aria-label="`Resume ${job.label}`"
              @click="resumeJob(job)"
            >
              <AppIcon name="play" :size="12" :stroke-width="2.4" />
            </button>
            <button
              v-if="canRetry(job)"
              v-tooltip="'Retry'"
              type="button"
              class="work-action"
              :aria-label="`Retry ${job.label}`"
              @click="retryJob(job)"
            >
              <AppIcon name="refresh" :size="12" :stroke-width="2.1" />
            </button>
            <button
              v-if="canCancel(job)"
              v-tooltip="'Cancel'"
              type="button"
              class="work-action"
              :aria-label="`Cancel ${job.label}`"
              @click="cancelJob(job)"
            >
              <AppIcon name="x" :size="12" :stroke-width="2.4" />
            </button>
            <button
              v-if="canDismiss(job)"
              v-tooltip="'Dismiss'"
              type="button"
              class="work-action"
              :aria-label="`Dismiss ${job.label}`"
              @click="dismissJob(job)"
            >
              <AppIcon name="x" :size="12" :stroke-width="2.4" />
            </button>
          </div>

          <div
            class="work-job-progress"
            :class="{ 'work-job-progress--indeterminate': job.progress === null && ['running', 'cancelling'].includes(job.status) }"
          >
            <span
              :style="{ width: job.progress === null ? '38%' : `${Math.max(4, job.progress * 100)}%` }"
            ></span>
          </div>
          <div
            v-if="job.currentProgress !== null"
            class="work-job-current-progress"
            aria-hidden="true"
          >
            <span :style="{ width: `${Math.max(4, job.currentProgress * 100)}%` }"></span>
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
.work-action {
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
.work-action:hover:not(:disabled) {
  background: var(--btn-hover);
  color: var(--text-muted);
}

.work-close:active,
.work-action:active:not(:disabled) {
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
  grid-template-columns: minmax(0, 1fr) auto;
  gap: 8px;
  overflow: hidden;
  border: 1px solid var(--hairline);
  border-radius: 9px;
  padding: 9px 9px 16px;
  background: color-mix(in srgb, var(--control-glass) 74%, transparent);
}

.work-job--failed {
  border-color: rgb(var(--danger-rgb) / 0.32);
}

.work-job--cancelled,
.work-job--cancelling {
  border-color: rgb(var(--warning-rgb) / 0.28);
}

.work-job--paused {
  border-color: rgb(var(--warning-rgb) / 0.34);
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

.work-job .work-job-meta {
  color: var(--text-faint);
  font-size: 10.5px;
}

.work-actions {
  display: flex;
  align-items: flex-start;
  gap: 4px;
  align-self: start;
}

.work-action {
  width: 24px;
  height: 24px;
}

.work-action:disabled {
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

.work-job-current-progress {
  position: absolute;
  right: 8px;
  bottom: 2px;
  left: 8px;
  height: 2px;
  overflow: hidden;
  border-radius: 999px;
  background: color-mix(in srgb, var(--text-faint) 13%, transparent);
}

.work-job-current-progress span {
  position: absolute;
  inset: 0 auto 0 0;
  min-width: 12px;
  border-radius: inherit;
  background: color-mix(in srgb, var(--accent) 58%, var(--text));
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
