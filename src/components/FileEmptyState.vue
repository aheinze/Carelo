<script setup>
import AppIcon from './AppIcon.vue';

defineProps({
  icon: {
    type: String,
    default: 'folder',
  },
  title: {
    type: String,
    required: true,
  },
  detail: {
    type: String,
    default: '',
  },
  compact: {
    type: Boolean,
    default: false,
  },
  grid: {
    type: Boolean,
    default: false,
  },
});
</script>

<template>
  <div
    class="file-empty-state"
    :class="{
      'file-empty-state--compact': compact,
      'file-empty-state--grid': grid,
    }"
    role="status"
  >
    <span class="file-empty-state__icon" aria-hidden="true">
      <AppIcon :name="icon" :size="compact ? 20 : 24" :stroke-width="1.55" />
    </span>
    <span class="file-empty-state__copy">
      <strong>{{ title }}</strong>
      <span v-if="detail">{{ detail }}</span>
    </span>
  </div>
</template>

<style scoped>
.file-empty-state {
  display: grid;
  min-height: 190px;
  align-content: center;
  justify-items: center;
  gap: 9px;
  padding: 36px 28px;
  color: var(--text-muted);
  text-align: center;
}

.file-empty-state--grid {
  grid-column: 1 / -1;
  min-height: min(260px, calc(100vh - 300px));
  padding-top: 28px;
  padding-bottom: 28px;
}

.file-empty-state--compact {
  min-height: 180px;
  gap: 8px;
  padding: 28px 14px;
}

.file-empty-state__icon {
  display: grid;
  width: 42px;
  height: 42px;
  place-items: center;
  border: 1px solid color-mix(in srgb, var(--text) 6%, transparent);
  border-radius: 12px;
  background: color-mix(in srgb, var(--text) 2.8%, transparent);
  color: color-mix(in srgb, var(--folder-icon) 58%, var(--text-muted));
}

.file-empty-state--compact .file-empty-state__icon {
  width: 36px;
  height: 36px;
  border-radius: 10px;
}

.file-empty-state__copy {
  display: grid;
  max-width: min(360px, 100%);
  gap: 5px;
}

.file-empty-state--compact .file-empty-state__copy {
  max-width: 210px;
}

.file-empty-state__copy strong {
  color: var(--text-muted);
  font-size: 13px;
  font-weight: 650;
  letter-spacing: 0;
}

.file-empty-state__copy span {
  color: var(--text-faint);
  font-size: 12px;
  font-weight: 520;
  line-height: 1.45;
}

.file-empty-state--compact .file-empty-state__copy span {
  display: -webkit-box;
  overflow: hidden;
  white-space: normal;
  -webkit-box-orient: vertical;
  -webkit-line-clamp: 2;
}
</style>
