<script setup>
import { computed, onBeforeUnmount, ref, watch } from 'vue';
import AppIcon from './AppIcon.vue';
import {
  cancelFileOperation,
  createMediaStreamUrl,
  getFileMetadata,
  isRemotePath,
  localFileAssetUrl,
  measureItemsSize,
  readMediaPreview,
  readTextPreview,
} from '../composables/useFileOperations';
import { useFileManagerStore } from '../stores/fileManagerStore';
import { archiveParentPath, isArchivePath } from '../utils/archivePaths';
import { formatFileDateTime } from '../utils/dateFormat';
import {
  audioTypeLabel,
  audioMimeType,
  extensionForName,
  imageTypeLabel,
  isAudioEntry,
  isImageEntry,
  isPdfEntry,
  isTextEntry,
  isVideoEntry,
  documentTypeLabel,
  videoMimeType,
  videoTypeLabel,
} from '../utils/fileTypes';
import { fileTypeIconKind, fileTypeIconName } from '../utils/fileTypeIcons';

const store = useFileManagerStore();
const fallbackPaneId = computed(() => (store.activePaneId === 'left' ? 'right' : 'left'));
const previewSelectionEntries = computed(() => {
  const activeSelection = store.selectedEntriesFor(store.activePaneId);

  if (activeSelection.length > 0) {
    return activeSelection;
  }

  const activeEntry = store.selectedEntryFor(store.activePaneId);

  if (activeEntry) {
    return [activeEntry];
  }

  const fallbackSelection = store.selectedEntriesFor(fallbackPaneId.value);

  if (fallbackSelection.length > 0) {
    return fallbackSelection;
  }

  const fallbackEntry = store.selectedEntryFor(fallbackPaneId.value);
  return fallbackEntry ? [fallbackEntry] : [];
});
const hasMultipleSelection = computed(() => previewSelectionEntries.value.length > 1);
const selectedEntry = computed(() => (
  hasMultipleSelection.value ? null : previewSelectionEntries.value[0] || null
));
const folderSizeMeasurement = ref(null);
const folderSizeMeasurementLoading = ref(false);
const folderSizeMeasurementError = ref('');
let folderSizeMeasureVersion = 0;
let folderSizeMeasureJobId = null;
let folderSizeMeasureSequence = 0;
const measurableFolderEntries = computed(() =>
  previewSelectionEntries.value.filter((entry) => isMeasurableFolderEntry(entry)),
);
const measurableFolderPaths = computed(() =>
  measurableFolderEntries.value.map((entry) => entry.path),
);
const measurableFolderPathSet = computed(() => new Set(measurableFolderPaths.value));
const directKnownSelectionSize = computed(() =>
  previewSelectionEntries.value.reduce((total, entry) => (
    entry.kind !== 'directory' && hasKnownSize(entry) ? total + Number(entry.size) : total
  ), 0),
);
const measuredFolderSize = computed(() => Number(folderSizeMeasurement.value?.logicalBytes || 0));
const measuredFolderEntryCount = computed(() => {
  const measurement = folderSizeMeasurement.value;

  return Number(measurement?.files || 0) +
    Number(measurement?.directories || 0) +
    Number(measurement?.symlinks || 0);
});
const selectionKnownSize = computed(() => directKnownSelectionSize.value + measuredFolderSize.value);
const selectionUnknownSizeCount = computed(() =>
  previewSelectionEntries.value.reduce((count, entry) => {
    if (entry.kind === 'directory') {
      if (!measurableFolderPathSet.value.has(entry.path)) {
        return count + 1;
      }

      if (!folderSizeMeasurement.value) {
        return count + 1;
      }

      return measuredFolderEntryCount.value > 0 ? count : count + 1;
    }

    return hasKnownSize(entry) ? count : count + 1;
  }, 0),
);
const selectionTypeSummary = computed(() => {
  const counts = previewSelectionEntries.value.reduce((summary, entry) => {
    if (entry.kind === 'directory') {
      summary.folders += 1;
    } else if (entry.kind === 'file') {
      summary.files += 1;
    } else {
      summary.other += 1;
    }

    return summary;
  }, { files: 0, folders: 0, other: 0 });
  const parts = [
    countLabel(counts.files, 'file'),
    countLabel(counts.folders, 'folder'),
    countLabel(counts.other, 'item'),
  ].filter(Boolean);

  return parts.join(', ');
});
const selectionSizeLabel = computed(() =>
  folderSizeMeasurementLoading.value && measurableFolderPaths.value.length > 0
    ? 'Calculating size'
    : selectionUnknownSizeCount.value > 0
      ? 'Known size'
      : 'Overall size',
);
const selectionSizeValue = computed(() => {
  if (selectionKnownSize.value > 0) {
    return formatBytes(selectionKnownSize.value);
  }

  if (folderSizeMeasurementLoading.value && measurableFolderPaths.value.length > 0) {
    return 'Calculating...';
  }

  if (folderSizeMeasurement.value && selectionUnknownSizeCount.value > 0) {
    return 'Size unavailable';
  }

  return selectionUnknownSizeCount.value > 0 ? 'Size unavailable' : '0 B';
});
const selectionSizeDetail = computed(() => {
  const details = [];
  const missing = selectionUnknownSizeCount.value;

  if (folderSizeMeasurementLoading.value && measurableFolderPaths.value.length > 0) {
    details.push(`Scanning ${countLabel(measurableFolderPaths.value.length, 'folder')}`);
  }

  if (folderSizeMeasurement.value?.skipped) {
    details.push(`${countLabel(folderSizeMeasurement.value.skipped, 'item')} skipped`);
  }

  if (folderSizeMeasurementError.value) {
    details.push(folderSizeMeasurementError.value);
  }

  if (missing > 0 && !folderSizeMeasurementLoading.value) {
    details.push(`${countLabel(missing, 'item')} not counted`);
  }

  return details.join(' · ');
});
const selectionCommonLocation = computed(() => {
  const parents = [...new Set(previewSelectionEntries.value.map((entry) => parentPathFor(entry.path)))];

  return parents.length === 1 ? parents[0] : 'Multiple locations';
});
const imageFailed = ref(false);
const audioFailed = ref(false);
const audioLoading = ref(false);
const audioReady = ref(false);
const audioPreviewUrl = ref('');
const audioPreviewMimeType = ref('');
const videoFailed = ref(false);
const videoLoading = ref(false);
const videoReady = ref(false);
const videoElementRef = ref(null);
const videoPreviewUrl = ref('');
const textPreview = ref('');
const textPreviewLoading = ref(false);
const textPreviewError = ref('');
const textPreviewTruncated = ref(false);
const fileMetadata = ref(null);
const metadataLoading = ref(false);
const metadataError = ref('');
const activeInspectorSection = ref('info');
const mediaPreviewFallbackMaxBytes = 128 * 1024 * 1024;
const mediaPreviewFallbackDelayMs = 1800;
const videoHaveMetadataReadyState = 1;
const videoReadyPollIntervalMs = 350;
const videoReadyPollMaxAttempts = 20;
let metadataLoadVersion = 0;
let audioPreviewLoadVersion = 0;
let videoPreviewLoadVersion = 0;
let audioPreviewFallbackTimer = null;
let videoPreviewFallbackTimer = null;
let videoReadyPollTimer = null;
let videoReadyPollAttempts = 0;

const inspectorSections = [
  { id: 'info', label: 'Info', icon: 'info', size: 17, strokeWidth: 2 },
  { id: 'work', label: 'Current work', icon: 'work-queue', size: 17, strokeWidth: 1.85 },
  { id: 'log', label: 'Log', icon: 'list', size: 17, strokeWidth: 1.9 },
];

const inspectedEntry = computed(() => {
  if (!selectedEntry.value) {
    return null;
  }

  return {
    ...selectedEntry.value,
    ...(fileMetadata.value || {}),
  };
});

const permissions = computed(() => fileMetadata.value?.permissions || null);
const runningJobs = computed(() =>
  store.queue.filter((job) => ['running', 'paused', 'cancelling'].includes(job.status)),
);
const currentWorkSummary = computed(() => {
  const count = store.queue.length;
  const activeCount = runningJobs.value.length;

  if (count === 0) {
    return 'Idle';
  }

  if (activeCount > 0) {
    return `${activeCount} active`;
  }

  return `${count} recent`;
});
const logSummary = computed(() => {
  const count = store.operationLog.length;

  if (count === 0) {
    return 'Empty';
  }

  return `${count} event${count === 1 ? '' : 's'}`;
});

watch(
  () => selectedEntry.value?.path,
  async (path) => {
    imageFailed.value = false;
    audioFailed.value = false;
    videoFailed.value = false;
    fileMetadata.value = null;
    metadataError.value = '';
    metadataLoadVersion += 1;
    const loadVersion = metadataLoadVersion;

    if (!path) {
      metadataLoading.value = false;
      return;
    }

    metadataLoading.value = true;

    try {
      const metadata = await getFileMetadata(path);

      if (metadataLoadVersion === loadVersion) {
        fileMetadata.value = metadata;
      }
    } catch (error) {
      if (metadataLoadVersion === loadVersion) {
        metadataError.value = error?.message || 'Unable to load metadata.';
      }
    } finally {
      if (metadataLoadVersion === loadVersion) {
        metadataLoading.value = false;
      }
    }
  },
  { immediate: true },
);

watch(
  () => [store.previewPanelVisible, measurableFolderPaths.value.join('\0')],
  () => {
    startFolderSizeMeasurement();
  },
  { immediate: true },
);

watch(
  () => [inspectedEntry.value?.path, inspectedEntry.value?.size, inspectedEntry.value?.name],
  () => {
    audioPreviewLoadVersion += 1;
    const loadVersion = audioPreviewLoadVersion;
    revokeAudioPreviewUrl();
    audioFailed.value = false;
    audioReady.value = false;
    audioLoading.value = false;
    const entry = inspectedEntry.value;

    if (!entry || !isAudioEntry(entry)) {
      return;
    }

    if (!canPreviewLocalMedia(entry)) {
      audioFailed.value = true;
      audioLoading.value = false;
      return;
    }

    const assetUrl = localFileAssetUrl(entry.path);

    if (!assetUrl) {
      audioFailed.value = true;
      audioLoading.value = false;
      return;
    }

    audioLoading.value = true;
    audioPreviewMimeType.value = audioMimeType(entry.name) || 'application/octet-stream';
    audioPreviewUrl.value = assetUrl;
    scheduleAudioBlobFallback(loadVersion);
  },
  { immediate: true },
);

watch(
  () => [inspectedEntry.value?.path, inspectedEntry.value?.size, inspectedEntry.value?.name],
  async () => {
    textPreview.value = '';
    textPreviewError.value = '';
    textPreviewTruncated.value = false;
    textPreviewLoading.value = false;
    const entry = inspectedEntry.value;

    if (!entry || !shouldShowTextPreview(entry)) {
      return;
    }

    textPreviewLoading.value = true;

    try {
      const preview = await readTextPreview(entry.path, 96 * 1024);
      textPreview.value = preview?.text || '';
      textPreviewTruncated.value = Boolean(preview?.truncated);
    } catch (error) {
      textPreviewError.value = error?.message || 'Text preview unavailable.';
    } finally {
      textPreviewLoading.value = false;
    }
  },
  { immediate: true },
);

watch(
  () => [inspectedEntry.value?.path, inspectedEntry.value?.size, inspectedEntry.value?.name],
  async () => {
    videoPreviewLoadVersion += 1;
    const loadVersion = videoPreviewLoadVersion;
    revokeVideoPreviewUrl();
    videoFailed.value = false;
    videoReady.value = false;
    videoLoading.value = false;
    const entry = inspectedEntry.value;

    if (!entry || !isVideoEntry(entry)) {
      return;
    }

    if (!canPreviewLocalMedia(entry)) {
      videoFailed.value = true;
      videoLoading.value = false;
      return;
    }

    videoLoading.value = true;

    try {
      const streamUrl = await createMediaStreamUrl(entry.path);

      if (videoPreviewLoadVersion !== loadVersion) {
        return;
      }

      videoPreviewUrl.value = streamUrl || localFileAssetUrl(entry.path);
    } catch (error) {
      if (videoPreviewLoadVersion !== loadVersion) {
        return;
      }

      videoPreviewUrl.value = localFileAssetUrl(entry.path);
    }

    if (!videoPreviewUrl.value) {
      videoFailed.value = true;
      videoLoading.value = false;
      return;
    }

    scheduleVideoBlobFallback(loadVersion);
    startVideoReadyPolling();
  },
  { immediate: true },
);

onBeforeUnmount(() => {
  cancelActiveFolderSizeMeasurement();
  revokeAudioPreviewUrl();
  revokeVideoPreviewUrl();
});

function countLabel(count, singular, plural = `${singular}s`) {
  if (!count) {
    return '';
  }

  return `${count} ${count === 1 ? singular : plural}`;
}

function hasKnownSize(entry) {
  return entry?.size !== null && entry?.size !== undefined && Number.isFinite(Number(entry.size));
}

function isMeasurableFolderEntry(entry) {
  return (
    entry?.kind === 'directory' &&
    Boolean(entry.path) &&
    !isArchivePath(entry.path) &&
    !isRemotePath(entry.path)
  );
}

function isOperationCancelled(error) {
  return (
    error?.code === 'operation_cancelled' ||
    /cancelled/i.test(String(error?.message || error || ''))
  );
}

function folderSizeSkippedLabel() {
  const skipped = Number(folderSizeMeasurement.value?.skipped || 0);

  return skipped > 0 ? `${countLabel(skipped, 'item')} skipped while calculating size.` : '';
}

function displaySizeForEntry(entry) {
  if (entry?.kind === 'directory' && isMeasurableFolderEntry(entry)) {
    if (folderSizeMeasurement.value) {
      if (measuredFolderEntryCount.value === 0 && folderSizeMeasurement.value.skipped > 0) {
        return 'Size unavailable';
      }

      return formatBytes(folderSizeMeasurement.value.logicalBytes);
    }

    if (folderSizeMeasurementLoading.value) {
      return 'Calculating...';
    }
  }

  return formatSize(entry?.size);
}

function hasDisplaySize(entry) {
  return displaySizeForEntry(entry) !== '--';
}

function cancelActiveFolderSizeMeasurement() {
  if (!folderSizeMeasureJobId) {
    return;
  }

  const jobId = folderSizeMeasureJobId;
  folderSizeMeasureJobId = null;
  cancelFileOperation(jobId).catch(() => {});
}

async function startFolderSizeMeasurement() {
  folderSizeMeasureVersion += 1;
  const measureVersion = folderSizeMeasureVersion;

  cancelActiveFolderSizeMeasurement();
  folderSizeMeasurement.value = null;
  folderSizeMeasurementError.value = '';

  if (!store.previewPanelVisible) {
    folderSizeMeasurementLoading.value = false;
    return;
  }

  const paths = measurableFolderPaths.value;

  if (paths.length === 0) {
    folderSizeMeasurementLoading.value = false;
    return;
  }

  folderSizeMeasureSequence += 1;
  const jobId = `preview-folder-size-${Date.now()}-${folderSizeMeasureSequence}`;

  folderSizeMeasureJobId = jobId;
  folderSizeMeasurementLoading.value = true;

  try {
    const result = await measureItemsSize(paths, jobId);

    if (folderSizeMeasureVersion === measureVersion) {
      folderSizeMeasurement.value = result;
      folderSizeMeasurementError.value = '';
    }
  } catch (error) {
    if (!isOperationCancelled(error)) {
      const message = error?.message || 'Unable to calculate folder size.';

      if (folderSizeMeasureVersion === measureVersion) {
        folderSizeMeasurementError.value = message;
      }
    }
  } finally {
    if (folderSizeMeasureVersion === measureVersion) {
      folderSizeMeasureJobId = null;
      folderSizeMeasurementLoading.value = false;
    }
  }
}

function formatSize(size) {
  if (size === null || size === undefined) return '--';
  return compactSize(size);
}

function compactSize(size) {
  if (size === null || size === undefined) return '--';
  if (size < 1024) return `${size} B`;
  if (size < 1024 * 1024) return `${(size / 1024).toFixed(0)} KB`;
  return `${(size / (1024 * 1024)).toFixed(1).replace('.', ',')} MB`;
}

function formatModified(modifiedAt) {
  return formatFileDateTime(modifiedAt, store.appSettings.dateFormat);
}

function extensionFor(name) {
  return extensionForName(name);
}

function displayTypeFor(name) {
  return imageTypeLabel(name) || videoTypeLabel(name) || audioTypeLabel(name) || documentTypeLabel(name) || extensionFor(name).toUpperCase();
}

function typeLabel(entry) {
  if (entry.kind === 'directory') return 'Folder';
  const ext = extensionFor(entry.name);
  return ext ? ext.toUpperCase() : 'File';
}

function previewFallbackClass(entry) {
  return `preview-file--${fileTypeIconKind(entry)}`;
}

function shouldShowImage(entry) {
  return isImageEntry(entry) && !isArchivePath(entry.path) && !imageFailed.value;
}

function shouldShowPdfPreview(entry) {
  return isPdfEntry(entry) && !isArchivePath(entry.path) && !isRemotePath(entry.path);
}

function pdfPreviewUrl(entry) {
  return appendUrlFragment(localFileAssetUrl(entry?.path), 'toolbar=0');
}

function appendUrlFragment(url, fragment) {
  if (!url) {
    return '';
  }

  return `${url}${url.includes('#') ? '&' : '#'}${fragment}`;
}

function shouldShowTextPreview(entry) {
  return isTextEntry(entry) && !isArchivePath(entry.path) && !isRemotePath(entry.path);
}

function canPreviewLocalMedia(entry) {
  return entry?.path && !isArchivePath(entry.path) && !isRemotePath(entry.path);
}

function canUseMediaBlobFallback(entry) {
  return (
    canPreviewLocalMedia(entry) &&
    (!hasKnownSize(entry) || Number(entry.size) <= mediaPreviewFallbackMaxBytes)
  );
}

async function createMediaPreviewObjectUrl(entry, mimeType) {
  if (!canUseMediaBlobFallback(entry)) {
    throw new Error('Media preview size limit exceeded.');
  }

  const payload = await readMediaPreview(entry.path, mediaPreviewFallbackMaxBytes);
  const bytes = mediaPreviewPayloadToBytes(payload);
  const blob = new Blob([bytes], { type: mimeType || 'application/octet-stream' });

  return URL.createObjectURL(blob);
}

function mediaPreviewPayloadToBytes(payload) {
  if (payload instanceof ArrayBuffer || ArrayBuffer.isView(payload)) {
    return payload;
  }

  if (Array.isArray(payload)) {
    return new Uint8Array(payload);
  }

  throw new Error('Unexpected media preview payload.');
}

function revokeObjectUrl(url) {
  if (typeof url === 'string' && url.startsWith('blob:')) {
    URL.revokeObjectURL(url);
  }
}

function clearAudioPreviewFallbackTimer() {
  if (audioPreviewFallbackTimer) {
    clearTimeout(audioPreviewFallbackTimer);
    audioPreviewFallbackTimer = null;
  }
}

function clearVideoPreviewFallbackTimer() {
  if (videoPreviewFallbackTimer) {
    clearTimeout(videoPreviewFallbackTimer);
    videoPreviewFallbackTimer = null;
  }
}

function clearVideoReadyPolling() {
  if (videoReadyPollTimer) {
    clearTimeout(videoReadyPollTimer);
    videoReadyPollTimer = null;
  }

  videoReadyPollAttempts = 0;
}

function scheduleAudioBlobFallback(loadVersion) {
  clearAudioPreviewFallbackTimer();
  audioPreviewFallbackTimer = window.setTimeout(() => {
    audioPreviewFallbackTimer = null;

    if (
      audioPreviewLoadVersion === loadVersion &&
      audioLoading.value &&
      !audioReady.value &&
      !audioFailed.value &&
      audioPreviewUrl.value &&
      !audioPreviewUrl.value.startsWith('blob:') &&
      canUseMediaBlobFallback(inspectedEntry.value)
    ) {
      loadAudioBlobFallback();
    }
  }, mediaPreviewFallbackDelayMs);
}

function scheduleVideoBlobFallback(loadVersion) {
  clearVideoPreviewFallbackTimer();
  videoPreviewFallbackTimer = window.setTimeout(() => {
    videoPreviewFallbackTimer = null;

    if (
      videoPreviewLoadVersion === loadVersion &&
      videoLoading.value &&
      !videoReady.value &&
      !videoFailed.value &&
      videoPreviewUrl.value &&
      !videoPreviewUrl.value.startsWith('blob:') &&
      canUseMediaBlobFallback(inspectedEntry.value)
    ) {
      loadVideoBlobFallback();
    }
  }, mediaPreviewFallbackDelayMs);
}

function revokeVideoPreviewUrl() {
  clearVideoPreviewFallbackTimer();
  clearVideoReadyPolling();
  revokeObjectUrl(videoPreviewUrl.value);
  videoPreviewUrl.value = '';
}

function revokeAudioPreviewUrl() {
  clearAudioPreviewFallbackTimer();
  revokeObjectUrl(audioPreviewUrl.value);
  audioPreviewUrl.value = '';
  audioPreviewMimeType.value = '';
}

async function loadAudioBlobFallback() {
  clearAudioPreviewFallbackTimer();
  const entry = inspectedEntry.value;

  if (!entry || !isAudioEntry(entry) || !canUseMediaBlobFallback(entry)) {
    audioFailed.value = true;
    audioLoading.value = false;
    return;
  }

  audioPreviewLoadVersion += 1;
  const loadVersion = audioPreviewLoadVersion;
  const mimeType = audioMimeType(entry.name) || 'application/octet-stream';
  audioLoading.value = true;
  audioFailed.value = false;
  audioReady.value = false;

  try {
    const url = await createMediaPreviewObjectUrl(entry, mimeType);

    if (audioPreviewLoadVersion !== loadVersion) {
      revokeObjectUrl(url);
      return;
    }

    revokeAudioPreviewUrl();
    audioPreviewMimeType.value = mimeType;
    audioPreviewUrl.value = url;
  } catch (error) {
    if (audioPreviewLoadVersion !== loadVersion) {
      return;
    }

    audioFailed.value = true;
    audioLoading.value = false;
  }
}

async function loadVideoBlobFallback() {
  clearVideoPreviewFallbackTimer();
  const entry = inspectedEntry.value;

  if (!entry || !isVideoEntry(entry) || !canUseMediaBlobFallback(entry)) {
    videoFailed.value = true;
    videoLoading.value = false;
    return;
  }

  videoPreviewLoadVersion += 1;
  const loadVersion = videoPreviewLoadVersion;
  const mimeType = videoMimeType(entry.name) || 'application/octet-stream';
  videoLoading.value = true;
  videoFailed.value = false;
  videoReady.value = false;

  try {
    const url = await createMediaPreviewObjectUrl(entry, mimeType);

    if (videoPreviewLoadVersion !== loadVersion) {
      revokeObjectUrl(url);
      return;
    }

    revokeVideoPreviewUrl();
    videoPreviewUrl.value = url;
    startVideoReadyPolling();
  } catch (error) {
    if (videoPreviewLoadVersion !== loadVersion) {
      return;
    }

    videoFailed.value = true;
    videoLoading.value = false;
  }
}

function handleAudioReady() {
  clearAudioPreviewFallbackTimer();
  audioReady.value = true;
  audioFailed.value = false;
  audioLoading.value = false;
}

async function handleAudioError(event) {
  const audio = event.currentTarget;

  if (audio?.readyState > 0) {
    handleAudioReady();
    return;
  }

  if (!audioPreviewUrl.value.startsWith('blob:')) {
    await loadAudioBlobFallback();
    return;
  }

  audioFailed.value = true;
  audioLoading.value = false;
}

function handleVideoReady() {
  clearVideoPreviewFallbackTimer();
  clearVideoReadyPolling();
  videoReady.value = true;
  videoFailed.value = false;
  videoLoading.value = false;
}

function hasLoadedVideoMetadata(video) {
  return Boolean(
    video &&
    (
      video.readyState >= videoHaveMetadataReadyState ||
      video.videoWidth > 0 ||
      video.videoHeight > 0 ||
      Number.isFinite(video.duration) ||
      video.seekable?.length > 0
    ),
  );
}

function checkVideoReadyState(event) {
  const video = event?.currentTarget || videoElementRef.value;

  if (hasLoadedVideoMetadata(video)) {
    handleVideoReady();
    return true;
  }

  return false;
}

function startVideoReadyPolling() {
  clearVideoReadyPolling();

  videoReadyPollTimer = window.setTimeout(pollVideoReadyState, videoReadyPollIntervalMs);
}

function pollVideoReadyState() {
  videoReadyPollTimer = null;

  if (!videoLoading.value || videoReady.value || videoFailed.value || !videoPreviewUrl.value) {
    clearVideoReadyPolling();
    return;
  }

  if (checkVideoReadyState()) {
    return;
  }

  videoReadyPollAttempts += 1;

  if (videoReadyPollAttempts < videoReadyPollMaxAttempts) {
    videoReadyPollTimer = window.setTimeout(pollVideoReadyState, videoReadyPollIntervalMs);
  }
}

async function handleVideoError(event) {
  const video = event.currentTarget;

  if (video?.readyState > 0) {
    handleVideoReady();
    return;
  }

  if (!videoPreviewUrl.value.startsWith('blob:')) {
    await loadVideoBlobFallback();
    return;
  }

  videoFailed.value = true;
  videoLoading.value = false;
}

function parentPathFor(path) {
  if (isArchivePath(path)) {
    return archiveParentPath(path);
  }

  const value = String(path || '');
  const trimmed = value.endsWith('/') && value.length > 1 ? value.slice(0, -1) : value;
  const index = trimmed.lastIndexOf('/');

  if (index < 0) return trimmed;
  if (index === 0) return '/';
  return trimmed.slice(0, index);
}

function permissionClass(set, key) {
  return {
    'checkbox--checked': Boolean(set?.[key]),
    'checkbox--unavailable': !set,
  };
}

function permissionName(value, fallbackId) {
  if (value) return value;
  if (fallbackId !== null && fallbackId !== undefined) return String(fallbackId);
  return '--';
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
  if (job.status === 'completed') return 'Done';
  if (job.status === 'failed') return 'Failed';
  if (job.status === 'cancelled') return 'Cancelled';
  if (job.status === 'cancelling') return 'Cancelling';
  if (job.status === 'paused') return 'Paused';
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

function canCancelJob(job) {
  return job.cancelable && ['running', 'paused', 'cancelling'].includes(job.status) && !job.cancelRequested;
}

function canPauseJob(job) {
  return job.pausable && job.status === 'running';
}

function canResumeJob(job) {
  return job.status === 'paused';
}

function canRetryJob(job) {
  return job.status === 'failed' && typeof job.retryAction === 'function';
}

function canDismissJob(job) {
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

function statusLabel(status) {
  if (status === 'completed') return 'Done';
  if (status === 'failed') return 'Failed';
  if (status === 'cancelled') return 'Cancelled';
  if (status === 'cancelling') return 'Cancelling';
  if (status === 'paused') return 'Paused';
  if (status === 'running') return 'Started';
  return 'Info';
}

function formatLogTime(timestamp) {
  if (!timestamp) {
    return '--';
  }

  return new Intl.DateTimeFormat(undefined, {
    hour: '2-digit',
    minute: '2-digit',
    second: '2-digit',
  }).format(new Date(timestamp));
}

function logDetail(entry) {
  return entry.detail || entry.path || statusLabel(entry.status);
}
</script>

<template>
  <aside
    class="preview-panel"
    :class="{ 'preview-panel--hidden': !store.previewPanelVisible }"
    aria-label="Inspector"
  >

    <!-- ── Tab row ─────────────────────────────────── -->
    <div class="inspector-tabs" aria-label="Inspector tabs">
      <button
        v-for="section in inspectorSections"
        :key="section.id"
        v-tooltip="section.label"
        type="button"
        class="inspector-tab"
        :class="{ 'inspector-tab--active': activeInspectorSection === section.id }"
        :aria-label="section.label"
        :aria-selected="activeInspectorSection === section.id"
        @click="activeInspectorSection = section.id"
      >
        <AppIcon :name="section.icon" :size="section.size" :stroke-width="section.strokeWidth" />
      </button>
    </div>

    <div class="inspector-content">
    <template v-if="activeInspectorSection === 'info'">
      <template v-if="hasMultipleSelection">
        <section class="selection-overview" aria-label="Selection summary">
          <div class="selection-stack-art" aria-hidden="true">
            <span class="selection-stack-card selection-stack-card--back"></span>
            <span class="selection-stack-card selection-stack-card--middle"></span>
            <span class="selection-stack-card selection-stack-card--front">
              <AppIcon name="file" :size="38" :stroke-width="1.35" />
            </span>
          </div>

          <div class="selection-identity">
            <h2>{{ previewSelectionEntries.length }} items selected</h2>
            <p>{{ selectionTypeSummary }}</p>
          </div>

          <div class="selection-metrics">
            <div class="selection-metric">
              <span>Items</span>
              <strong>{{ previewSelectionEntries.length }}</strong>
            </div>
            <div class="selection-metric">
              <span>{{ selectionSizeLabel }}</span>
              <strong>{{ selectionSizeValue }}</strong>
              <small v-if="selectionSizeDetail">{{ selectionSizeDetail }}</small>
            </div>
          </div>

          <dl class="selection-general">
            <div>
              <dt>Location</dt>
              <dd :title="selectionCommonLocation">{{ selectionCommonLocation }}</dd>
            </div>
            <div>
              <dt>Selection</dt>
              <dd>{{ selectionTypeSummary }}</dd>
            </div>
          </dl>
        </section>

      </template>

      <template v-else-if="inspectedEntry">

        <!-- ── Hero ───────────────────────────────────── -->
        <div class="preview-hero">
          <span
            v-if="inspectedEntry.kind === 'directory'"
            class="preview-folder"
          >
            <AppIcon name="folder" :size="80" :stroke-width="1.3" />
          </span>
          <img
            v-else-if="shouldShowImage(inspectedEntry)"
            class="preview-image"
            :src="localFileAssetUrl(inspectedEntry.path)"
            :alt="inspectedEntry.name"
            decoding="async"
            @error="imageFailed = true"
          />
          <span
            v-else-if="isVideoEntry(inspectedEntry)"
            class="preview-video-shell"
          >
            <video
              v-if="videoPreviewUrl"
              ref="videoElementRef"
              :key="videoPreviewUrl"
              :src="videoPreviewUrl"
              class="preview-video"
              controls
              playsinline
              preload="metadata"
              @loadedmetadata="handleVideoReady"
              @loadeddata="handleVideoReady"
              @canplay="handleVideoReady"
              @canplaythrough="handleVideoReady"
              @durationchange="checkVideoReadyState"
              @progress="checkVideoReadyState"
              @suspend="checkVideoReadyState"
              @play="handleVideoReady"
              @playing="handleVideoReady"
              @error="handleVideoError"
            >
              Your system webview cannot play this video.
            </video>
            <span v-if="videoLoading && !videoReady && !videoFailed" class="preview-media-status">
              Loading video...
            </span>
            <span v-else-if="videoFailed && !videoReady" class="preview-media-status">
              Video preview unavailable
            </span>
          </span>
          <span
            v-else-if="isAudioEntry(inspectedEntry)"
            class="preview-audio-shell"
          >
            <span class="preview-audio-art">
              <AppIcon name="music" :size="52" :stroke-width="1.5" />
            </span>
            <audio
              v-if="audioPreviewUrl"
              :key="audioPreviewUrl"
              class="preview-audio"
              controls
              preload="metadata"
              @loadedmetadata="handleAudioReady"
              @canplay="handleAudioReady"
              @error="handleAudioError"
            >
              <source :src="audioPreviewUrl" :type="audioPreviewMimeType" />
              Your system webview cannot play this audio file.
            </audio>
            <span v-if="audioLoading && !audioReady && !audioFailed" class="preview-media-status">
              Loading audio...
            </span>
            <span v-else-if="audioFailed && !audioReady" class="preview-media-status">
              Audio preview unavailable
            </span>
          </span>
          <span
            v-else-if="shouldShowPdfPreview(inspectedEntry)"
            class="preview-pdf-shell"
          >
            <iframe
              class="preview-pdf"
              :src="pdfPreviewUrl(inspectedEntry)"
              :title="`Preview of ${inspectedEntry.name}`"
            ></iframe>
          </span>
          <span
            v-else-if="shouldShowTextPreview(inspectedEntry)"
            class="preview-text-shell"
          >
            <span v-if="textPreviewLoading" class="preview-media-status">
              Loading text...
            </span>
            <span v-else-if="textPreviewError" class="preview-media-status">
              {{ textPreviewError }}
            </span>
            <pre v-else class="preview-text"><code>{{ textPreview }}</code></pre>
            <span v-if="textPreviewTruncated" class="preview-text-truncated">
              Preview truncated
            </span>
          </span>
          <span v-else class="preview-file" :class="previewFallbackClass(inspectedEntry)">
            <span class="preview-file-icon">
              <AppIcon :name="fileTypeIconName(inspectedEntry)" :size="76" :stroke-width="1.28" />
              <span class="preview-ext">{{ extensionFor(inspectedEntry.name).toUpperCase() || '?' }}</span>
            </span>
          </span>
        </div>

        <!-- ── Identity ────────────────────────────────── -->
        <div class="file-identity">
          <h2 :title="inspectedEntry.name">{{ inspectedEntry.name }}</h2>
          <div class="file-meta-row">
            <span>{{ displayTypeFor(inspectedEntry.name) || typeLabel(inspectedEntry) }}</span>
            <span v-if="hasDisplaySize(inspectedEntry)">
              - {{ displaySizeForEntry(inspectedEntry) }}
            </span>
          </div>
        </div>

        <!-- ── General ─────────────────────────────────── -->
        <section class="inspector-section">
          <h3 class="section-title">
            <span>General</span>
            <AppIcon name="chevron-down" :size="18" :stroke-width="1.9" />
          </h3>
          <dl>
            <div>
              <dt>Path</dt>
              <dd :title="parentPathFor(inspectedEntry.path)">
                {{ parentPathFor(inspectedEntry.path) }}
              </dd>
            </div>
            <div>
              <dt>Size</dt>
              <dd>{{ displaySizeForEntry(inspectedEntry) }}</dd>
            </div>
            <div>
              <dt>Modified</dt>
              <dd>{{ formatModified(inspectedEntry.modifiedAt) }}</dd>
            </div>
            <div>
              <dt>Created</dt>
              <dd>{{ formatModified(inspectedEntry.createdAt) }}</dd>
            </div>
            <div>
              <dt>Accessed</dt>
              <dd>{{ formatModified(inspectedEntry.accessedAt) }}</dd>
            </div>
            <div>
              <dt>Extension</dt>
              <dd>{{ extensionFor(inspectedEntry.name) || '--' }}</dd>
            </div>
            <div>
              <dt>Hidden</dt>
              <dd>
                <span class="checkbox" :class="{ 'checkbox--checked': inspectedEntry.isHidden }"></span>
              </dd>
            </div>
            <div>
              <dt>Read Only</dt>
              <dd>
                <span class="checkbox" :class="{ 'checkbox--checked': inspectedEntry.isReadonly }"></span>
              </dd>
            </div>
          </dl>
          <p v-if="folderSizeMeasurementLoading && isMeasurableFolderEntry(inspectedEntry)" class="metadata-note">
            Calculating folder size...
          </p>
          <p v-else-if="folderSizeMeasurementError && isMeasurableFolderEntry(inspectedEntry)" class="metadata-note metadata-note--warning">
            {{ folderSizeMeasurementError }}
          </p>
          <p v-else-if="folderSizeSkippedLabel() && isMeasurableFolderEntry(inspectedEntry)" class="metadata-note">
            {{ folderSizeSkippedLabel() }}
          </p>
          <p v-if="metadataLoading" class="metadata-note">Loading file metadata...</p>
          <p v-else-if="metadataError" class="metadata-note metadata-note--warning">
            {{ metadataError }}
          </p>
        </section>

        <!-- ── Permissions ─────────────────────────────── -->
        <section class="inspector-section">
          <h3 class="section-title">
            <span>Permissions</span>
            <AppIcon name="chevron-down" :size="18" :stroke-width="1.9" />
          </h3>
          <div v-if="permissions" class="permissions-block">
            <div class="perm-bits">
              <span></span>
              <span class="perm-col-head">R</span>
              <span class="perm-col-head">W</span>
              <span class="perm-col-head">X</span>

              <span class="perm-row-head">Owner</span>
              <span class="perm-dot" :class="{ 'perm-dot--on': permissions.owner?.read, 'perm-dot--na': !permissions.owner }"></span>
              <span class="perm-dot" :class="{ 'perm-dot--on': permissions.owner?.write, 'perm-dot--na': !permissions.owner }"></span>
              <span class="perm-dot" :class="{ 'perm-dot--on': permissions.owner?.execute, 'perm-dot--na': !permissions.owner }"></span>

              <span class="perm-row-head">Group</span>
              <span class="perm-dot" :class="{ 'perm-dot--on': permissions.group?.read, 'perm-dot--na': !permissions.group }"></span>
              <span class="perm-dot" :class="{ 'perm-dot--on': permissions.group?.write, 'perm-dot--na': !permissions.group }"></span>
              <span class="perm-dot" :class="{ 'perm-dot--on': permissions.group?.execute, 'perm-dot--na': !permissions.group }"></span>

              <span class="perm-row-head">Others</span>
              <span class="perm-dot" :class="{ 'perm-dot--on': permissions.others?.read, 'perm-dot--na': !permissions.others }"></span>
              <span class="perm-dot" :class="{ 'perm-dot--on': permissions.others?.write, 'perm-dot--na': !permissions.others }"></span>
              <span class="perm-dot" :class="{ 'perm-dot--on': permissions.others?.execute, 'perm-dot--na': !permissions.others }"></span>
            </div>

            <dl>
              <div>
                <dt>Mode</dt>
                <dd class="perm-mono">{{ permissions.symbolic }}</dd>
              </div>
              <div>
                <dt>Octal</dt>
                <dd class="perm-mono">{{ permissions.octal }}</dd>
              </div>
              <div>
                <dt>Owner</dt>
                <dd>{{ permissionName(permissions.ownerName, permissions.uid) }}</dd>
              </div>
              <div>
                <dt>Group</dt>
                <dd>{{ permissionName(permissions.groupName, permissions.gid) }}</dd>
              </div>
              <div>
                <dt>Locked</dt>
                <dd>
                  <span class="checkbox" :class="{ 'checkbox--checked': inspectedEntry.isReadonly }"></span>
                </dd>
              </div>
            </dl>
          </div>
          <div v-else class="permissions-unavailable">
            {{ metadataLoading ? 'Loading permissions...' : 'Permissions unavailable' }}
          </div>
        </section>

      </template>

      <!-- ── Empty ───────────────────────────────────── -->
      <div v-else class="empty-state">
        <AppIcon name="file" :size="36" :stroke-width="1.4" />
        <p>Select a file to inspect</p>
      </div>
    </template>

    <template v-else-if="activeInspectorSection === 'work'">
      <section class="inspector-section inspector-section--fill">
        <h3 class="section-title">
          <span>Current Work</span>
          <span class="section-pill">{{ currentWorkSummary }}</span>
        </h3>

        <div v-if="store.queue.length" class="inspector-work-list">
          <article
            v-for="job in store.queue"
            :key="job.id"
            class="inspector-work-job"
            :class="`inspector-work-job--${job.status}`"
          >
            <div class="inspector-work-main">
              <div class="inspector-work-title">
                <span>{{ job.label }}</span>
                <small>{{ jobStatus(job) }}</small>
              </div>
              <p :title="jobDetail(job)">{{ jobDetail(job) }}</p>
              <p v-if="currentFileDetail(job)" class="inspector-work-current" :title="job.currentPath">
                {{ currentFileDetail(job) }}
              </p>
            </div>

            <div class="inspector-work-actions" aria-label="Job actions">
              <button
                v-if="canPauseJob(job)"
                v-tooltip="'Pause'"
                type="button"
                class="inspector-icon-button"
                :aria-label="`Pause ${job.label}`"
                @click="pauseJob(job)"
              >
                <AppIcon name="pause" :size="12" :stroke-width="2.4" />
              </button>
              <button
                v-if="canResumeJob(job)"
                v-tooltip="'Resume'"
                type="button"
                class="inspector-icon-button"
                :aria-label="`Resume ${job.label}`"
                @click="resumeJob(job)"
              >
                <AppIcon name="play" :size="12" :stroke-width="2.4" />
              </button>
              <button
                v-if="canRetryJob(job)"
                v-tooltip="'Retry'"
                type="button"
                class="inspector-icon-button"
                :aria-label="`Retry ${job.label}`"
                @click="retryJob(job)"
              >
                <AppIcon name="refresh" :size="12" :stroke-width="2.1" />
              </button>
              <button
                v-if="canCancelJob(job)"
                v-tooltip="'Cancel'"
                type="button"
                class="inspector-icon-button"
                :aria-label="`Cancel ${job.label}`"
                @click="cancelJob(job)"
              >
                <AppIcon name="x" :size="12" :stroke-width="2.4" />
              </button>
              <button
                v-if="canDismissJob(job)"
                v-tooltip="'Dismiss'"
                type="button"
                class="inspector-icon-button"
                :aria-label="`Dismiss ${job.label}`"
                @click="dismissJob(job)"
              >
                <AppIcon name="x" :size="12" :stroke-width="2.4" />
              </button>
            </div>

            <div
              class="inspector-work-progress"
              :class="{ 'inspector-work-progress--indeterminate': job.progress === null && ['running', 'cancelling'].includes(job.status) }"
            >
              <span :style="{ width: job.progress === null ? '38%' : `${Math.max(4, job.progress * 100)}%` }"></span>
            </div>
            <div
              v-if="job.currentProgress !== null"
              class="inspector-work-current-progress"
              aria-hidden="true"
            >
              <span :style="{ width: `${Math.max(4, job.currentProgress * 100)}%` }"></span>
            </div>
          </article>
        </div>

        <div v-else class="inspector-panel-empty">
          <AppIcon name="work-queue" :size="20" :stroke-width="1.9" />
          <span>No file operations are running.</span>
        </div>
      </section>
    </template>

    <template v-else>
      <section class="inspector-section inspector-section--fill">
        <h3 class="section-title">
          <span>Log</span>
          <button
            v-if="store.operationLog.length"
            type="button"
            class="section-action"
            @click="store.clearOperationLog()"
          >
            Clear
          </button>
          <span v-else class="section-pill">{{ logSummary }}</span>
        </h3>

        <div v-if="store.operationLog.length" class="inspector-log-list">
          <article
            v-for="entry in store.operationLog"
            :key="entry.id"
            class="inspector-log-entry"
            :class="`inspector-log-entry--${entry.status}`"
          >
            <span class="inspector-log-dot" aria-hidden="true"></span>
            <div class="inspector-log-main">
              <div class="inspector-log-title">
                <span>{{ entry.label }}</span>
                <small>{{ formatLogTime(entry.createdAt) }}</small>
              </div>
              <p :title="logDetail(entry)">{{ logDetail(entry) }}</p>
              <small v-if="entry.path" class="inspector-log-path" :title="entry.path">{{ entry.path }}</small>
            </div>
            <span class="inspector-log-status">{{ statusLabel(entry.status) }}</span>
          </article>
        </div>

        <div v-else class="inspector-panel-empty">
          <AppIcon name="list" :size="20" :stroke-width="1.9" />
          <span>No operation log entries.</span>
        </div>
      </section>
    </template>
    </div>

  </aside>
</template>

<style scoped>
.preview-panel {
  display: flex;
  flex-direction: column;
  min-width: 0;
  min-height: 0;
  overflow: hidden;
  border-radius: 0 0 12px 0;
  background: var(--footer-bg);
  box-shadow: inset -1px 0 0 var(--hairline);
  transition: opacity 180ms ease;
}

.preview-panel--hidden {
  opacity: 0;
  pointer-events: none;
}

/* ── Tab row ──────────────────────────────────────────────── */
.inspector-tabs {
  position: relative;
  z-index: 4;
  display: flex;
  align-items: center;
  justify-content: flex-end;
  gap: 18px;
  flex: 0 0 55px;
  min-width: 0;
  width: 100%;
  padding: 7px 34px 0;
  border-bottom: 1px solid var(--hairline);
  background: var(--footer-bg);
  box-shadow: 0 1px 0 var(--separator);
}

.inspector-content {
  flex: 1 1 auto;
  min-width: 0;
  min-height: 0;
  overflow-x: hidden;
  overflow-y: auto;
  overscroll-behavior: contain;
  scrollbar-gutter: stable;
}

.inspector-tab {
  display: inline-flex;
  width: 28px;
  height: 38px;
  align-items: center;
  justify-content: center;
  border-radius: 7px;
  background: transparent;
  color: var(--icon);
  cursor: pointer;
  transition: background 100ms ease, color 100ms ease;
}

.inspector-tab:hover {
  background: var(--btn-hover);
  color: var(--text);
}

.inspector-tab--active {
  color: var(--accent);
}

/* ── Preview hero ─────────────────────────────────────────── */
.preview-hero {
  flex-shrink: 0;
  overflow: hidden;
  margin: 37px 20px 0;
  border-radius: 7px;
  background: color-mix(in srgb, var(--text) 6%, transparent);
}

.preview-image,
.preview-video,
.preview-pdf-shell {
  display: block;
  width: 100%;
  aspect-ratio: 16 / 9;
  object-fit: contain;
  border-radius: 7px;
  background: color-mix(in srgb, var(--text) 8%, transparent);
}

.preview-pdf-shell {
  --preview-pdf-toolbar-height: 44px;
  height: min(420px, 52vh);
  overflow: hidden;
}

.preview-pdf {
  display: block;
  width: 100%;
  height: calc(100% + var(--preview-pdf-toolbar-height));
  border: 0;
  border-radius: 0;
  transform: translateY(calc(var(--preview-pdf-toolbar-height) * -1));
}

.preview-video-shell,
.preview-audio-shell,
.preview-text-shell {
  position: relative;
  display: grid;
  width: 100%;
  aspect-ratio: 16 / 9;
  place-items: center;
  border-radius: 7px;
  background: color-mix(in srgb, var(--text) 8%, transparent);
}

.preview-text-shell {
  display: block;
  aspect-ratio: auto;
  max-height: min(420px, 52vh);
  overflow: hidden;
  border: 1px solid color-mix(in srgb, var(--text) 8%, transparent);
}

.preview-text {
  min-height: 220px;
  max-height: min(420px, 52vh);
  margin: 0;
  overflow: auto;
  padding: 14px;
  color: var(--text);
  font-family:
    "SF Mono",
    "Cascadia Code",
    "Roboto Mono",
    ui-monospace,
    monospace;
  font-size: 11.5px;
  line-height: 1.5;
  tab-size: 2;
  white-space: pre-wrap;
  user-select: text;
}

.preview-text-truncated {
  position: absolute;
  right: 10px;
  bottom: 10px;
  border-radius: 999px;
  padding: 5px 8px;
  background: var(--popover-bg);
  color: var(--text-muted);
  font-size: 11px;
  font-weight: 650;
  box-shadow: var(--shadow-overlay);
}

.preview-audio-shell {
  grid-template-rows: minmax(0, 1fr) auto;
  gap: 15px;
  align-content: center;
  padding: 24px;
}

.preview-video {
  max-height: 260px;
}

.preview-audio-art {
  display: grid;
  width: 78px;
  height: 78px;
  place-items: center;
  border: 1px solid color-mix(in srgb, var(--accent) 36%, transparent);
  border-radius: 999px;
  background: color-mix(in srgb, var(--accent) 14%, transparent);
  color: var(--accent);
}

.preview-audio {
  width: min(100%, 300px);
  height: 36px;
  color-scheme: light dark;
  accent-color: var(--accent);
}

.preview-media-status {
  position: absolute;
  inset: auto 12px 12px;
  border-radius: 6px;
  padding: 7px 9px;
  background: var(--popover-bg);
  color: var(--text-muted);
  box-shadow: var(--shadow-overlay);
  font-size: 11.5px;
  font-weight: 650;
  text-align: center;
}

.preview-folder,
.preview-file {
  display: grid;
  width: 100%;
  aspect-ratio: 4 / 3;
  place-items: center;
}

.preview-folder {
  color: var(--folder-icon);
}

.preview-file {
  position: relative;
  color: var(--icon);
}

.preview-file-icon {
  position: relative;
  display: grid;
  width: 104px;
  height: 104px;
  place-items: center;
  border: 1px solid color-mix(in srgb, currentColor 18%, transparent);
  border-radius: 16px;
  background:
    linear-gradient(
      180deg,
      color-mix(in srgb, currentColor 12%, transparent),
      color-mix(in srgb, currentColor 5%, transparent)
    );
  box-shadow: inset 0 1px 0 color-mix(in srgb, white 18%, transparent);
}

.preview-file--archive,
.preview-file--audio,
.preview-file--code,
.preview-file--config,
.preview-file--document,
.preview-file--image,
.preview-file--spreadsheet,
.preview-file--presentation,
.preview-file--video {
  color: color-mix(in srgb, var(--file-icon) 82%, var(--file-type-tint, var(--accent)) 18%);
}

.preview-file--archive,
.preview-file--spreadsheet {
  --file-type-tint: var(--folder-icon);
}

.preview-file--audio,
.preview-file--config,
.preview-file--presentation,
.preview-file--video {
  --file-type-tint: var(--accent-warm);
}

.preview-file--code,
.preview-file--document,
.preview-file--image {
  --file-type-tint: var(--accent);
}

.preview-ext {
  position: absolute;
  right: 10px;
  bottom: 10px;
  min-width: 29px;
  max-width: 54px;
  overflow: hidden;
  border: 1px solid color-mix(in srgb, currentColor 24%, transparent);
  border-radius: 7px;
  padding: 3px 5px;
  background: color-mix(in srgb, var(--modal-bg) 86%, currentColor 14%);
  font-size: 11px;
  font-weight: 780;
  letter-spacing: 0;
  line-height: 1;
  color: var(--text);
  pointer-events: none;
  text-align: center;
  text-overflow: ellipsis;
}

/* ── File identity ────────────────────────────────────────── */
.file-identity {
  padding: 36px 34px 0;
  flex-shrink: 0;
}

h2 {
  margin: 0;
  overflow: hidden;
  color: var(--text);
  font-size: 16px;
  font-weight: 750;
  letter-spacing: -0.01em;
  line-height: 1.2;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.file-meta-row {
  display: flex;
  align-items: center;
  gap: 5px;
  margin-top: 2px;
  color: var(--text-muted);
  font-size: 13px;
  font-weight: 650;
}

.type-badge {
  display: inline-flex;
  align-items: center;
  height: 18px;
  border-radius: 4px;
  padding: 0 6px;
  background: var(--btn-hover);
  color: var(--text-muted);
  font-size: 10px;
  font-weight: 700;
  letter-spacing: 0.04em;
}

.size-label {
  color: var(--text-faint);
  font-size: 11.5px;
  font-weight: 500;
}

/* ── Multi-selection summary ──────────────────────────────── */
.selection-overview {
  display: grid;
  gap: 16px;
  margin: 37px 20px 0;
}

.selection-stack-art {
  position: relative;
  display: grid;
  width: 100%;
  aspect-ratio: 4 / 3;
  place-items: center;
  overflow: hidden;
  border-radius: 7px;
  background: color-mix(in srgb, var(--text) 6%, transparent);
}

.selection-stack-card {
  position: absolute;
  display: grid;
  width: 128px;
  height: 86px;
  place-items: center;
  border: 1px solid color-mix(in srgb, var(--accent) 32%, transparent);
  border-radius: 8px;
  background: var(--popover-bg);
  color: var(--accent);
  box-shadow: 0 1px 0 rgb(255 255 255 / 0.07);
}

.selection-stack-card--back {
  transform: translate(-18px, -12px) rotate(-7deg);
  opacity: 0.48;
}

.selection-stack-card--middle {
  transform: translate(10px, -2px) rotate(4deg);
  opacity: 0.7;
}

.selection-stack-card--front {
  transform: translate(0, 12px);
}

.selection-identity {
  min-width: 0;
  padding: 0 14px;
}

.selection-identity p {
  margin: 4px 0 0;
  overflow: hidden;
  color: var(--text-muted);
  font-size: 13px;
  font-weight: 650;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.selection-metrics {
  display: grid;
  grid-template-columns: minmax(0, 0.72fr) minmax(0, 1.28fr);
  gap: 8px;
}

.selection-metric {
  display: grid;
  min-width: 0;
  min-height: 72px;
  align-content: center;
  gap: 3px;
  border-radius: 7px;
  padding: 12px 13px;
  background: color-mix(in srgb, var(--text) 6%, transparent);
}

.selection-metric span,
.selection-metric small {
  overflow: hidden;
  color: var(--text-muted);
  font-size: 11.5px;
  font-weight: 670;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.selection-metric strong {
  overflow: hidden;
  color: var(--text);
  font-size: 16px;
  font-weight: 760;
  letter-spacing: 0;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.selection-metric small {
  color: var(--text-faint);
  font-weight: 620;
}

.selection-general {
  border-radius: 7px;
}

/* ── Inspector sections ───────────────────────────────────── */
.inspector-section {
  padding: 17px 20px 0;
  flex-shrink: 0;
}

.inspector-section--fill {
  min-height: 0;
  padding-top: 20px;
}

.section-title {
  display: flex;
  align-items: center;
  justify-content: space-between;
  margin: 0;
  border-radius: 7px 7px 0 0;
  padding: 10px 14px 8px;
  background: color-mix(in srgb, var(--text) 6%, transparent);
  color: var(--text-muted);
  font-size: 14px;
  font-weight: 690;
  letter-spacing: 0;
  text-transform: none;
}

.section-pill,
.section-action {
  display: inline-flex;
  height: 22px;
  align-items: center;
  justify-content: center;
  border-radius: 6px;
  padding: 0 8px;
  font-size: 11px;
  font-weight: 700;
}

.section-pill {
  background: var(--btn-hover);
  color: var(--text-faint);
}

.section-action {
  background: transparent;
  color: var(--icon);
  cursor: pointer;
  transition: background 80ms ease, color 80ms ease;
}

.section-action:hover {
  background: var(--btn-hover);
  color: var(--text-muted);
}

.section-action:active {
  background: var(--btn-active-bg);
}

dl {
  margin: 0;
  border-radius: 0 0 7px 7px;
  overflow: hidden;
  background: color-mix(in srgb, var(--text) 6%, transparent);
}

dl > div {
  display: grid;
  grid-template-columns: 92px minmax(0, 1fr);
  align-items: baseline;
  gap: 8px;
  min-height: 30px;
  padding: 5px 13px;
  border-top: 1px solid var(--hairline);
}

dl > div:nth-child(odd) {
  background: transparent;
}

dt {
  color: var(--text-muted);
  font-size: 11.5px;
  font-weight: 670;
  white-space: nowrap;
}

dd {
  min-width: 0;
  margin: 0;
  overflow: hidden;
  color: var(--text);
  font-size: 11.5px;
  font-weight: 650;
  text-align: right;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.metadata-note {
  margin: 8px 0 0;
  border-radius: 7px;
  padding: 8px 12px;
  background: color-mix(in srgb, var(--text) 6%, transparent);
  color: var(--text-faint);
  font-size: 11.5px;
  font-weight: 620;
}

.metadata-note--warning {
  color: rgb(255 198 125 / 0.9);
}

/* ── Current work and log ─────────────────────────────────── */
.inspector-work-list,
.inspector-log-list {
  display: grid;
  gap: 8px;
  overflow: hidden;
  border-radius: 0 0 7px 7px;
  padding: 10px;
  background: color-mix(in srgb, var(--text) 6%, transparent);
}

.inspector-work-job {
  position: relative;
  display: grid;
  grid-template-columns: minmax(0, 1fr) auto;
  gap: 8px;
  overflow: hidden;
  border: 1px solid var(--hairline);
  border-radius: 8px;
  padding: 9px 9px 17px;
  background: color-mix(in srgb, var(--control-glass) 74%, transparent);
}

.inspector-work-job--failed {
  border-color: rgb(var(--danger-rgb) / 0.32);
}

.inspector-work-job--cancelled,
.inspector-work-job--cancelling {
  border-color: rgb(var(--warning-rgb) / 0.28);
}

.inspector-work-job--paused {
  border-color: rgb(var(--warning-rgb) / 0.34);
}

.inspector-work-main,
.inspector-log-main {
  display: grid;
  gap: 4px;
  min-width: 0;
}

.inspector-work-title,
.inspector-log-title {
  display: flex;
  min-width: 0;
  align-items: center;
  justify-content: space-between;
  gap: 10px;
}

.inspector-work-title span,
.inspector-work-job p,
.inspector-work-current,
.inspector-log-title span,
.inspector-log-entry p,
.inspector-log-path {
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.inspector-work-title span,
.inspector-log-title span {
  color: var(--text);
  font-size: 12.5px;
  font-weight: 670;
}

.inspector-work-title small,
.inspector-log-title small,
.inspector-log-status {
  flex: 0 0 auto;
  color: var(--text-faint);
  font-size: 10.5px;
  font-weight: 700;
  text-transform: uppercase;
}

.inspector-work-job p,
.inspector-work-current,
.inspector-log-entry p,
.inspector-log-path {
  margin: 0;
  color: var(--text-muted);
  font-size: 11px;
  font-weight: 520;
}

.inspector-work-current {
  color: var(--text-faint);
  font-size: 10.5px;
}

.inspector-work-actions {
  display: flex;
  align-items: flex-start;
  gap: 4px;
  align-self: start;
}

.inspector-icon-button {
  display: inline-flex;
  width: 24px;
  height: 24px;
  align-items: center;
  justify-content: center;
  align-self: start;
  border-radius: 6px;
  background: transparent;
  color: var(--icon);
  cursor: pointer;
  transition: background 80ms ease, color 80ms ease;
}

.inspector-icon-button:hover:not(:disabled) {
  background: var(--btn-hover);
  color: var(--text-muted);
}

.inspector-icon-button:active:not(:disabled) {
  background: var(--btn-active-bg);
}

.inspector-icon-button:disabled {
  cursor: default;
  opacity: 0.35;
}

.inspector-work-progress {
  position: absolute;
  right: 8px;
  bottom: 6px;
  left: 8px;
  height: 4px;
  overflow: hidden;
  border-radius: 999px;
  background: color-mix(in srgb, var(--text-faint) 20%, transparent);
}

.inspector-work-progress span {
  position: absolute;
  inset: 0 auto 0 0;
  min-width: 18px;
  border-radius: inherit;
  background: var(--accent);
}

.inspector-work-progress--indeterminate span {
  animation: inspector-progress-slide 1s ease-in-out infinite;
}

.inspector-work-current-progress {
  position: absolute;
  right: 8px;
  bottom: 2px;
  left: 8px;
  height: 2px;
  overflow: hidden;
  border-radius: 999px;
  background: color-mix(in srgb, var(--text-faint) 13%, transparent);
}

.inspector-work-current-progress span {
  position: absolute;
  inset: 0 auto 0 0;
  min-width: 12px;
  border-radius: inherit;
  background: color-mix(in srgb, var(--accent) 58%, var(--text));
}

.inspector-panel-empty {
  display: flex;
  min-height: 92px;
  align-items: center;
  gap: 9px;
  border-radius: 0 0 7px 7px;
  padding: 16px 14px;
  background: color-mix(in srgb, var(--text) 6%, transparent);
  color: var(--text-muted);
  font-size: 12px;
  font-weight: 560;
}

.inspector-log-list {
  gap: 0;
  padding: 0 10px 10px;
}

.inspector-log-entry {
  display: grid;
  grid-template-columns: 8px minmax(0, 1fr) auto;
  align-items: start;
  gap: 9px;
  min-width: 0;
  padding: 10px 0;
  border-top: 1px solid var(--hairline);
}

.inspector-log-entry:first-child {
  border-top: 0;
}

.inspector-log-dot {
  width: 7px;
  height: 7px;
  margin-top: 5px;
  border-radius: 999px;
  background: var(--text-faint);
}

.inspector-log-entry--running .inspector-log-dot {
  background: var(--accent);
}

.inspector-log-entry--completed .inspector-log-dot {
  background: var(--success);
}

.inspector-log-entry--failed .inspector-log-dot {
  background: rgb(var(--danger-rgb));
}

.inspector-log-entry--cancelled .inspector-log-dot,
.inspector-log-entry--cancelling .inspector-log-dot,
.inspector-log-entry--paused .inspector-log-dot {
  background: rgb(var(--warning-rgb));
}

.inspector-log-path {
  display: block;
  color: var(--text-faint);
}

/* ── Permissions ──────────────────────────────────────────── */
.permissions-block {
  border-radius: 0 0 7px 7px;
  overflow: hidden;
  background: color-mix(in srgb, var(--text) 6%, transparent);
}

.permissions-block dl {
  border-radius: 0;
  background: transparent;
}

.perm-bits {
  display: grid;
  grid-template-columns: minmax(0, 1fr) 28px 28px 28px;
  align-items: center;
  column-gap: 2px;
  padding: 6px 13px 8px;
  border-bottom: 1px solid var(--separator);
}

.perm-col-head {
  color: var(--text-faint);
  font-size: 10px;
  font-weight: 700;
  letter-spacing: 0.07em;
  text-transform: uppercase;
  text-align: center;
  padding-bottom: 2px;
}

.perm-row-head {
  min-height: 25px;
  display: flex;
  align-items: center;
  color: var(--text-muted);
  font-size: 11.5px;
  font-weight: 580;
}

.perm-dot {
  display: block;
  width: 9px;
  height: 9px;
  border-radius: 50%;
  place-self: center;
  background: transparent;
  border: 1.5px solid color-mix(in srgb, var(--text) 28%, transparent);
}

.perm-dot--on {
  background: color-mix(in srgb, var(--text) 50%, transparent);
  border-color: transparent;
}

.perm-dot--na {
  opacity: 0.25;
}

.perm-mono {
  font-family: ui-monospace, 'SF Mono', Menlo, monospace;
  font-size: 10.5px;
  letter-spacing: 0.02em;
}

.checkbox {
  display: inline-block;
  width: 9px;
  height: 9px;
  border-radius: 50%;
  vertical-align: middle;
  background: transparent;
  border: 1.5px solid color-mix(in srgb, var(--text) 28%, transparent);
}

.checkbox--checked {
  background: color-mix(in srgb, var(--text) 52%, transparent);
  border-color: transparent;
}

.permissions-unavailable {
  border-radius: 0 0 7px 7px;
  padding: 10px 13px 12px;
  background: color-mix(in srgb, var(--text) 6%, transparent);
  color: var(--text-faint);
  font-size: 11.5px;
  font-weight: 500;
}

/* ── Empty state ──────────────────────────────────────────── */
.empty-state {
  display: flex;
  flex: 1;
  flex-direction: column;
  align-items: center;
  justify-content: center;
  gap: 10px;
  padding: 40px 20px;
  color: var(--text-faint);
}

.empty-state p {
  margin: 0;
  font-size: 12px;
}

@keyframes inspector-progress-slide {
  0% {
    transform: translateX(-100%);
  }

  100% {
    transform: translateX(270%);
  }
}
</style>
