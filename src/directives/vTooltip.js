import { tooltipState } from '../composables/useTooltipState';

let showTimer = null;

function parseBinding(value) {
  if (!value) return { text: '', description: '' };
  if (typeof value === 'string') return { text: value, description: '' };
  return { text: value.text || '', description: value.description || '' };
}

function showTooltip(el) {
  clearTimeout(showTimer);
  showTimer = setTimeout(() => {
    const rect = el.getBoundingClientRect();
    const { text, description } = el._vTooltipBinding;
    tooltipState.text = text;
    tooltipState.description = description;
    tooltipState.x = rect.left + rect.width / 2;
    tooltipState.y = rect.top;
    tooltipState.targetBottom = rect.bottom;
    tooltipState.visible = true;
  }, 380);
}

function hideTooltip() {
  clearTimeout(showTimer);
  tooltipState.visible = false;
}

export const vTooltip = {
  mounted(el, binding) {
    const parsed = parseBinding(binding.value);
    if (!parsed.text) return;
    el._vTooltipBinding = parsed;
    el._vTooltipShow = () => showTooltip(el);
    el._vTooltipHide = hideTooltip;
    el.addEventListener('mouseenter', el._vTooltipShow);
    el.addEventListener('mouseleave', el._vTooltipHide);
    el.addEventListener('mousedown', el._vTooltipHide);
  },
  updated(el, binding) {
    el._vTooltipBinding = parseBinding(binding.value);
  },
  unmounted(el) {
    el.removeEventListener('mouseenter', el._vTooltipShow);
    el.removeEventListener('mouseleave', el._vTooltipHide);
    el.removeEventListener('mousedown', el._vTooltipHide);
  },
};
