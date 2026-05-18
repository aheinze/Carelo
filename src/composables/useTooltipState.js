import { reactive } from 'vue';

export const tooltipState = reactive({
  visible: false,
  text: '',
  description: '',
  x: 0,
  y: 0,
  targetBottom: 0,
});
