<script setup>
defineProps({
  name: {
    type: String,
    required: true,
  },
  size: {
    type: [Number, String],
    default: 18,
  },
  strokeWidth: {
    type: [Number, String],
    default: 1.8,
  },
});
</script>

<template>
  <svg
    class="app-icon"
    :width="size"
    :height="size"
    viewBox="0 0 24 24"
    fill="none"
    :stroke-width="strokeWidth"
    stroke="currentColor"
    stroke-linecap="round"
    stroke-linejoin="round"
    aria-hidden="true"
    focusable="false"
  >
    <template v-if="name === 'chevron-left'">
      <path d="m15 18-6-6 6-6" />
    </template>

    <template v-else-if="name === 'chevron-right'">
      <path d="m9 18 6-6-6-6" />
    </template>

    <template v-else-if="name === 'grid'">
      <rect x="4" y="4" width="6" height="6" rx="1.4" />
      <rect x="14" y="4" width="6" height="6" rx="1.4" />
      <rect x="4" y="14" width="6" height="6" rx="1.4" />
      <rect x="14" y="14" width="6" height="6" rx="1.4" />
    </template>

    <template v-else-if="name === 'list'">
      <path d="M8 6h12" />
      <path d="M8 12h12" />
      <path d="M8 18h12" />
      <circle cx="4" cy="6" r="1" fill="currentColor" stroke="none" />
      <circle cx="4" cy="12" r="1" fill="currentColor" stroke="none" />
      <circle cx="4" cy="18" r="1" fill="currentColor" stroke="none" />
    </template>

    <template v-else-if="name === 'columns'">
      <rect x="4" y="5" width="16" height="14" rx="2" />
      <path d="M9.5 5v14" />
      <path d="M14.5 5v14" />
    </template>

    <template v-else-if="name === 'star'">
      <path d="m12 3.7 2.45 5 5.5.8-3.98 3.88.94 5.48L12 16.28l-4.91 2.58.94-5.48L4.05 9.5l5.5-.8L12 3.7Z" />
    </template>

    <template v-else-if="name === 'network'">
      <circle cx="7" cy="7" r="2.4" />
      <circle cx="17" cy="17" r="2.4" />
      <circle cx="17" cy="6" r="1.8" />
      <path d="m8.8 8.8 6.4 6.4" />
      <path d="m9.4 6.8 5.8-.6" />
    </template>

    <template v-else-if="name === 'refresh'">
      <path d="M20 12a8 8 0 0 1-14.6 4.55" />
      <path d="M4 12A8 8 0 0 1 18.6 7.45" />
      <path d="M18.8 3.7v3.9h-3.9" />
      <path d="M5.2 20.3v-3.9h3.9" />
    </template>

    <template v-else-if="name === 'pause'">
      <path d="M8.5 5.5v13" />
      <path d="M15.5 5.5v13" />
    </template>

    <template v-else-if="name === 'play'">
      <path d="M8 5.4v13.2L18.4 12 8 5.4Z" />
    </template>

    <template v-else-if="name === 'copy'">
      <rect x="8" y="7" width="10" height="12" rx="2" />
      <path d="M6 15H5a2 2 0 0 1-2-2V6a2 2 0 0 1 2-2h7a2 2 0 0 1 2 2v1" />
    </template>

    <template v-else-if="name === 'archive'">
      <path d="M5.6 5.2h12.8A1.6 1.6 0 0 1 20 6.8v10.4a1.6 1.6 0 0 1-1.6 1.6H5.6A1.6 1.6 0 0 1 4 17.2V6.8a1.6 1.6 0 0 1 1.6-1.6Z" />
      <path d="M8 5.2v13.6" />
      <path d="M10.3 7.4H8" />
      <path d="M10.3 11H8" />
      <path d="M10.3 14.6H8" />
    </template>

    <template v-else-if="name === 'extract'">
      <path d="M5.6 4.8h12.8A1.6 1.6 0 0 1 20 6.4v4.1H4V6.4a1.6 1.6 0 0 1 1.6-1.6Z" />
      <path d="M5.6 10.5h12.8A1.6 1.6 0 0 1 20 12.1v5.5a1.6 1.6 0 0 1-1.6 1.6H5.6A1.6 1.6 0 0 1 4 17.6v-5.5a1.6 1.6 0 0 1 1.6-1.6Z" />
      <path d="M12 8v7" />
      <path d="m8.9 12.2 3.1 3.1 3.1-3.1" />
    </template>

    <template v-else-if="name === 'folder-plus'">
      <path d="M3.8 7.6A2.6 2.6 0 0 1 6.4 5h3.4l2.1 2.3h5.7a2.6 2.6 0 0 1 2.6 2.6v6.5a2.6 2.6 0 0 1-2.6 2.6H6.4a2.6 2.6 0 0 1-2.6-2.6Z" />
      <path d="M12 10.6v5" />
      <path d="M9.5 13.1h5" />
    </template>

    <template v-else-if="name === 'trash'">
      <path d="M5 7h14" />
      <path d="M9 7V5.8A1.8 1.8 0 0 1 10.8 4h2.4A1.8 1.8 0 0 1 15 5.8V7" />
      <path d="M17.5 7 16.7 19a2 2 0 0 1-2 1.9H9.3a2 2 0 0 1-2-1.9L6.5 7" />
      <path d="M10 11v5" />
      <path d="M14 11v5" />
    </template>

    <template v-else-if="name === 'search'">
      <circle cx="10.8" cy="10.8" r="6.3" />
      <path d="m16 16 4 4" />
    </template>

    <template v-else-if="name === 'settings'">
      <circle cx="12" cy="12" r="3" />
      <path d="M12 3.8v2.1" />
      <path d="M12 18.1v2.1" />
      <path d="m5.8 5.8 1.5 1.5" />
      <path d="m16.7 16.7 1.5 1.5" />
      <path d="M3.8 12h2.1" />
      <path d="M18.1 12h2.1" />
      <path d="m5.8 18.2 1.5-1.5" />
      <path d="m16.7 7.3 1.5-1.5" />
    </template>

    <template v-else-if="name === 'sliders'">
      <path d="M4 6.5h8.2" />
      <path d="M16.8 6.5H20" />
      <circle cx="14.3" cy="6.5" r="2.1" />
      <path d="M4 12h3.2" />
      <path d="M11.8 12H20" />
      <circle cx="9.3" cy="12" r="2.1" />
      <path d="M4 17.5h9.2" />
      <path d="M17.8 17.5H20" />
      <circle cx="15.3" cy="17.5" r="2.1" />
    </template>

    <template v-else-if="name === 'monitor'">
      <rect x="4" y="5" width="16" height="11" rx="2" />
      <path d="M9 20h6" />
      <path d="M12 16v4" />
    </template>

    <template v-else-if="name === 'sun'">
      <circle cx="12" cy="12" r="3.7" />
      <path d="M12 3.5v2" />
      <path d="M12 18.5v2" />
      <path d="M4.6 4.6 6 6" />
      <path d="m18 18 1.4 1.4" />
      <path d="M3.5 12h2" />
      <path d="M18.5 12h2" />
      <path d="M4.6 19.4 6 18" />
      <path d="m18 6 1.4-1.4" />
    </template>

    <template v-else-if="name === 'moon'">
      <path d="M18.8 14.2A7.1 7.1 0 0 1 9.8 5.2 7.4 7.4 0 1 0 18.8 14.2Z" />
    </template>

    <template v-else-if="name === 'shield'">
      <path d="M12 3.8 19 6.5v5.2c0 4.1-2.8 7-7 8.5-4.2-1.5-7-4.4-7-8.5V6.5Z" />
      <path d="m9.2 12 2 2 4-4" />
    </template>

    <template v-else-if="name === 'sidebar'">
      <rect x="4" y="5" width="16" height="14" rx="2.2" />
      <path d="M9.5 5v14" />
    </template>

    <template v-else-if="name === 'panel-right'">
      <rect x="4" y="5" width="16" height="14" rx="2.2" />
      <path d="M14.5 5v14" />
    </template>

    <template v-else-if="name === 'open-other-pane'">
      <rect x="3.5" y="5" width="7" height="14" rx="1.7" />
      <rect x="13.5" y="5" width="7" height="14" rx="1.7" />
      <path d="M7.1 12h8.2" />
      <path d="m12.7 9.4 2.6 2.6-2.6 2.6" />
    </template>

    <template v-else-if="name === 'terminal'">
      <rect x="3.8" y="5" width="16.4" height="14" rx="2.2" />
      <path d="m7.4 9 2.5 2.4-2.5 2.4" />
      <path d="M12.2 14h4.3" />
    </template>

    <template v-else-if="name === 'eye'">
      <path d="M3.6 12s3.1-5.4 8.4-5.4 8.4 5.4 8.4 5.4-3.1 5.4-8.4 5.4S3.6 12 3.6 12Z" />
      <circle cx="12" cy="12" r="2.7" />
    </template>

    <template v-else-if="name === 'eye-off'">
      <path d="m4 4 16 16" />
      <path d="M9.4 5.2A9.8 9.8 0 0 1 12 4.8c5.3 0 8.4 5.4 8.4 5.4a15.2 15.2 0 0 1-2.5 3.1" />
      <path d="M14.2 14.4A3 3 0 0 1 9.6 9.8" />
      <path d="M6.5 7.1A15 15 0 0 0 3.6 12s3.1 5.4 8.4 5.4a9.7 9.7 0 0 0 3.9-.8" />
    </template>

    <template v-else-if="name === 'home'">
      <path d="m3.8 11.2 8.2-7 8.2 7" />
      <path d="M6.2 10v8.4A1.6 1.6 0 0 0 7.8 20h8.4a1.6 1.6 0 0 0 1.6-1.6V10" />
      <path d="M10 20v-5.6h4V20" />
    </template>

    <template v-else-if="name === 'info'">
      <circle cx="12" cy="12" r="8.5" />
      <path d="M12 10.8v5.6" />
      <path d="M12 7.4h.01" />
    </template>

    <template v-else-if="name === 'sync'">
      <path d="M17.6 7.1A7.5 7.5 0 0 0 5 10.4" />
      <path d="M17.6 3.8v3.3h-3.3" />
      <path d="M6.4 16.9A7.5 7.5 0 0 0 19 13.6" />
      <path d="M6.4 20.2v-3.3h3.3" />
    </template>

    <template v-else-if="name === 'activity-list'">
      <path d="M5 6.4h4.2" />
      <path d="M14.8 6.4H19" />
      <path d="M5 12h2.6" />
      <path d="M16.4 12H19" />
      <path d="M5 17.6h4.2" />
      <path d="M14.8 17.6H19" />
      <circle cx="12" cy="6.4" r="2.2" />
      <circle cx="12" cy="17.6" r="2.2" />
      <path d="m8 12 2.2 2.2L16 8.4" />
    </template>

    <template v-else-if="name === 'work-queue'">
      <path d="M5 6.8h8.2" />
      <path d="M5 11.8h6.4" />
      <path d="M5 16.8h4.8" />
      <circle cx="16.5" cy="15.5" r="3.7" />
      <path d="M16.5 13.4v2.2l1.5.9" />
    </template>

    <template v-else-if="name === 'alert'">
      <path d="M10.4 4.4 3.6 17a2 2 0 0 0 1.8 3h13.2a2 2 0 0 0 1.8-3L13.6 4.4a1.8 1.8 0 0 0-3.2 0Z" />
      <path d="M12 9v4.2" />
      <path d="M12 16.8h.01" />
    </template>

    <template v-else-if="name === 'chevron-down'">
      <path d="m6 9 6 6 6-6" />
    </template>

    <template v-else-if="name === 'plus'">
      <path d="M12 5v14" />
      <path d="M5 12h14" />
    </template>

    <template v-else-if="name === 'x'">
      <path d="M7 7l10 10" />
      <path d="M17 7 7 17" />
    </template>

    <template v-else-if="name === 'folder'">
      <path d="M3.8 7.6A2.6 2.6 0 0 1 6.4 5h3.4l2.1 2.3h5.7a2.6 2.6 0 0 1 2.6 2.6v6.5a2.6 2.6 0 0 1-2.6 2.6H6.4a2.6 2.6 0 0 1-2.6-2.6Z" />
    </template>

    <template v-else-if="name === 'file'">
      <path d="M7 3.8h6l4 4V19a1.8 1.8 0 0 1-1.8 1.8H7A1.8 1.8 0 0 1 5.2 19V5.6A1.8 1.8 0 0 1 7 3.8Z" />
      <path d="M13 3.8V8h4" />
    </template>

    <template v-else-if="name === 'image'">
      <rect x="4" y="5" width="16" height="14" rx="2" />
      <circle cx="9" cy="10" r="1.5" />
      <path d="m5.7 17 4.4-4.4 3.2 3.2 1.6-1.6 3.4 2.8" />
    </template>

    <template v-else-if="name === 'drive'">
      <path d="M5.2 6.6h13.6l2 6.2v4.6A2.6 2.6 0 0 1 18.2 20H5.8a2.6 2.6 0 0 1-2.6-2.6v-4.6Z" />
      <path d="M3.2 12.8h17.6" />
      <circle cx="17" cy="16.4" r="1" fill="currentColor" stroke="none" />
    </template>

    <template v-else-if="name === 'server'">
      <rect x="4" y="4.5" width="16" height="6" rx="2" />
      <rect x="4" y="13.5" width="16" height="6" rx="2" />
      <path d="M8 7.5h.01" />
      <path d="M8 16.5h.01" />
    </template>

    <template v-else-if="name === 'download'">
      <path d="M12 4v10" />
      <path d="m7.5 9.5 4.5 4.5 4.5-4.5" />
      <path d="M5 19h14" />
    </template>

    <template v-else-if="name === 'music'">
      <path d="M9 18.2a2.4 2.4 0 1 1-1.4-2.2" />
      <path d="M17 16.2a2.4 2.4 0 1 1-1.4-2.2" />
      <path d="M9 16V6.2l8-1.6v9.6" />
      <path d="M9 9.2 17 7.6" />
    </template>

    <template v-else-if="name === 'cloud'">
      <path d="M17.5 18H8a4 4 0 1 1 .7-7.9 5.3 5.3 0 0 1 10 2.2A2.9 2.9 0 0 1 17.5 18Z" />
    </template>

    <template v-else-if="name === 'lock'">
      <rect x="5.5" y="10" width="13" height="10" rx="2" />
      <path d="M8.5 10V7.8a3.5 3.5 0 0 1 7 0V10" />
    </template>

    <template v-else-if="name === 'briefcase'">
      <rect x="4" y="7" width="16" height="12" rx="2" />
      <path d="M9 7V5.6A1.6 1.6 0 0 1 10.6 4h2.8A1.6 1.6 0 0 1 15 5.6V7" />
      <path d="M4 12h16" />
    </template>

    <template v-else-if="name === 'app'">
      <rect x="5" y="5" width="6" height="6" rx="1.4" />
      <rect x="13" y="5" width="6" height="6" rx="1.4" />
      <rect x="5" y="13" width="6" height="6" rx="1.4" />
      <rect x="13" y="13" width="6" height="6" rx="1.4" />
    </template>

    <template v-else>
      <circle cx="12" cy="12" r="8" />
    </template>
  </svg>
</template>

<style scoped>
.app-icon {
  display: block;
  flex: 0 0 auto;
  vector-effect: non-scaling-stroke;
}
</style>
