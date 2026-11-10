import { createApp } from 'vue';
import { createPinia } from 'pinia';
import App from './App.vue';
import './assets/main.css';
import { isMac, isWindows } from './platform';

// WebKit's own context menu (Reload, Inspect Element…) gives away the web view;
// keep it only where it's useful: text fields and selected text.
document.addEventListener('contextmenu', (e) => {
  const target = e.target as HTMLElement | null;
  const editable = target?.closest('input, textarea, [contenteditable="true"]');
  const hasSelection = !!window.getSelection()?.toString();
  if (!editable && !hasSelection) e.preventDefault();
});

// For styles that differ by system (see main.css).
document.documentElement.dataset.platform = isMac ? 'mac' : isWindows ? 'windows' : 'other';

const app = createApp(App);
app.use(createPinia());
app.mount('#app');
