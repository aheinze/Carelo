<script setup>
import { computed } from 'vue';
import { useFileManagerStore } from '../stores/fileManagerStore';

const store = useFileManagerStore();

const activeJob = computed(() =>
  store.queue.find((job) => ['running', 'paused', 'cancelling'].includes(job.status))
    || store.queue[0]
    || null,
);

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

function operationLabel(job) {
  if (!job) {
    return 'Idle';
  }

  if (job.status === 'paused') {
    return 'Paused';
  }

  if (job.status === 'completed') {
    return 'Done';
  }

  if (job.status === 'failed') {
    return 'Failed';
  }

  return job.label;
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

  if (job?.bytesPerSecond > 0) {
    metrics.push(`${formatBytes(job.bytesPerSecond)}/s`);
  }

  if (job?.etaSeconds !== null && job?.etaSeconds !== undefined) {
    metrics.push(`${formatDuration(job.etaSeconds)} left`);
  }

  return metrics.join(' · ');
}

function progressDetail(job) {
  if (!job) {
    return 'No active file operations';
  }

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

  return 'Preparing operation';
}
</script>

<template>
  <footer class="transfer-queue" aria-label="Transfer queue">
    <div class="queue-main">
      <strong>{{ operationLabel(activeJob) }}</strong>
      <span>{{ progressDetail(activeJob) }}</span>
    </div>

    <div
      v-if="activeJob"
      class="progress-track"
      :class="{ 'progress-track--indeterminate': activeJob.progress === null && ['running', 'cancelling'].includes(activeJob.status) }"
    >
      <span
        class="progress-fill"
        :style="{ width: activeJob.progress === null ? '36%' : `${Math.max(4, activeJob.progress * 100)}%` }"
      ></span>
      <span
        v-if="activeJob.currentProgress !== null"
        class="progress-fill progress-fill--current"
        :style="{ width: `${Math.max(4, activeJob.currentProgress * 100)}%` }"
      ></span>
    </div>

    <div class="status-strip">
      <span>{{ activeJob ? activeJob.status === 'paused' ? 'Paused' : formatPercent(activeJob) || 'Working' : 'Idle' }}</span>
      <span>{{ store.queue.length === 1 ? '1 job' : `${store.queue.length} jobs` }}</span>
    </div>
  </footer>
</template>

<style scoped>
.transfer-queue {
  display: flex;
  min-height: 30px;
  align-items: center;
  justify-content: space-between;
  gap: 16px;
  margin-top: 8px;
  border-top: none;
  border-radius: 10px;
  padding: 0 14px;
  background: var(--footer-bg);
  color: var(--text-muted);
  font-size: 11.5px;
  font-weight: 500;
}

.queue-main,
.status-strip {
  display: flex;
  align-items: center;
  gap: 9px;
}

.queue-main {
  min-width: 0;
  flex: 1 1 auto;
}

.queue-main span {
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

strong {
  color: var(--text);
  font-size: 11.5px;
  font-weight: 640;
  white-space: nowrap;
}

.progress-track {
  position: relative;
  flex: 0 1 220px;
  height: 5px;
  overflow: hidden;
  border-radius: 999px;
  background: var(--control-bg);
  box-shadow: inset 0 0 0 1px var(--hairline);
}

.progress-fill {
  position: absolute;
  top: 0;
  bottom: 0;
  left: 0;
  min-width: 18px;
  border-radius: inherit;
  background: var(--accent);
}

.progress-fill--current {
  top: auto;
  height: 2px;
  min-width: 12px;
  background: color-mix(in srgb, var(--accent) 58%, var(--text));
}

.progress-track--indeterminate .progress-fill:not(.progress-fill--current) {
  animation: queue-progress-slide 1s ease-in-out infinite;
}

.status-strip {
  flex: 0 0 auto;
  color: var(--text-faint);
}

.status-strip span + span::before {
  content: "·";
  margin-right: 9px;
  color: var(--text-faint);
  opacity: 0.5;
}

@keyframes queue-progress-slide {
  0% {
    transform: translateX(-100%);
  }

  100% {
    transform: translateX(290%);
  }
}
</style>
