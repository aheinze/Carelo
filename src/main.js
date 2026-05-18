import { createApp } from 'vue';
import { createPinia } from 'pinia';
import App from './App.vue';
import './assets/main.css';
import { vTooltip } from './directives/vTooltip';
import { restoreWindowDimensions } from './composables/useWindowDimensions';

await restoreWindowDimensions();

const app = createApp(App);

app.use(createPinia());
app.directive('tooltip', vTooltip);
app.mount('#app');
