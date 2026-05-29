import { computed, nextTick, ref } from 'vue';

const activeDialog = ref(null);
const queue = [];
let nextDialogId = 1;

const defaultLabels = {
  alert: {
    confirm: 'OK',
  },
  confirm: {
    confirm: 'Confirm',
    cancel: 'Cancel',
  },
  prompt: {
    confirm: 'Save',
    cancel: 'Cancel',
  },
  choice: {
    cancel: 'Cancel',
  },
};

function normalizeDialogAction(action = {}) {
  if (typeof action === 'string') {
    return {
      value: action,
      label: action,
      variant: 'default',
      primary: false,
      destructive: false,
      default: false,
      cancel: false,
    };
  }

  return {
    value: action.value ?? action.id ?? '',
    label: action.label || String(action.value ?? action.id ?? ''),
    variant: action.variant || 'default',
    primary: Boolean(action.primary),
    destructive: Boolean(action.destructive || action.variant === 'danger'),
    default: Boolean(action.default),
    cancel: Boolean(action.cancel),
  };
}

function normalizeDialog(options = {}) {
  const type = options.type || 'alert';
  const labels = defaultLabels[type] || defaultLabels.alert;

  return {
    id: nextDialogId++,
    type,
    size: options.size || '',
    variant: options.variant || 'default',
    icon: options.icon || '',
    title: options.title || '',
    message: options.message || '',
    detail: options.detail || '',
    inputLabel: options.inputLabel || '',
    inputType: options.inputType || 'text',
    inputValue: options.inputValue ?? options.defaultValue ?? '',
    inputSelection: options.inputSelection || null,
    inputPlaceholder: options.inputPlaceholder || options.placeholder || '',
    inputRequired: Boolean(options.inputRequired),
    facts: Array.isArray(options.facts) ? options.facts : [],
    actions: Array.isArray(options.actions)
      ? options.actions.map(normalizeDialogAction).filter((action) => action.value || action.cancel)
      : [],
    checkboxLabel: options.checkboxLabel || '',
    checkboxValue: Boolean(options.checkboxValue),
    confirmLabel: options.confirmLabel || labels.confirm,
    cancelLabel: options.cancelLabel || labels.cancel,
    showCancel: options.showCancel ?? (type !== 'alert' && type !== 'choice'),
    destructive: Boolean(options.destructive || options.variant === 'danger'),
    resolve: null,
  };
}

function showNextDialog() {
  if (!activeDialog.value && queue.length > 0) {
    activeDialog.value = queue.shift();
  }
}

function openDialog(options = {}) {
  const dialog = normalizeDialog(options);

  return new Promise((resolve) => {
    dialog.resolve = resolve;

    if (activeDialog.value) {
      queue.push(dialog);
    } else {
      activeDialog.value = dialog;
    }
  });
}

function resolveDialog(result) {
  const dialog = activeDialog.value;

  if (!dialog) {
    return;
  }

  activeDialog.value = null;
  dialog.resolve?.(result);
  nextTick(showNextDialog);
}

function alertDialog(options = {}) {
  const config = typeof options === 'string' ? { message: options } : options;

  return openDialog({
    type: 'alert',
    title: 'Notice',
    ...config,
  }).then(() => true);
}

function confirmDialog(options = {}) {
  const config = typeof options === 'string' ? { message: options } : options;

  return openDialog({
    type: 'confirm',
    title: 'Confirm Action',
    ...config,
  }).then(Boolean);
}

function promptDialog(options = {}) {
  const config = typeof options === 'string' ? { title: options } : options;

  return openDialog({
    type: 'prompt',
    title: 'Enter Value',
    ...config,
  });
}

function choiceDialog(options = {}) {
  const config = typeof options === 'string' ? { message: options } : options;

  return openDialog({
    type: 'choice',
    title: 'Choose Action',
    ...config,
  });
}

export function useDialog() {
  return {
    activeDialog: computed(() => activeDialog.value),
    hasOpenDialog: computed(() => Boolean(activeDialog.value)),
    open: openDialog,
    alert: alertDialog,
    confirm: confirmDialog,
    prompt: promptDialog,
    choice: choiceDialog,
    resolve: resolveDialog,
  };
}
