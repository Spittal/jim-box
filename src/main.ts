import { createApp } from 'vue';
import { createPinia } from 'pinia';
import { createRouter, createWebHistory } from 'vue-router';
import App from './App.vue';
import './assets/main.css';

const router = createRouter({
  history: createWebHistory(),
  routes: [
    { path: '/', redirect: '/search' },
    {
      path: '/search',
      name: 'search',
      component: () => import('./views/SearchView.vue'),
    },
    {
      path: '/collection',
      name: 'collection',
      component: () => import('./views/CollectionView.vue'),
    },
    {
      path: '/review',
      name: 'review',
      component: () => import('./views/ReviewView.vue'),
    },
    // The folder browser this replaced.
    { path: '/library', redirect: '/collection' },
    {
      path: '/downloads/:id?',
      name: 'downloads',
      component: () => import('./views/DownloadsView.vue'),
    },
    {
      path: '/settings',
      name: 'settings',
      component: () => import('./views/SettingsView.vue'),
    },
  ],
});

// WebKit's own context menu (Reload, Inspect Element…) gives away the web view;
// keep it only where it's useful: text fields and selected text.
document.addEventListener('contextmenu', (e) => {
  const target = e.target as HTMLElement | null;
  const editable = target?.closest('input, textarea, [contenteditable="true"]');
  const hasSelection = !!window.getSelection()?.toString();
  if (!editable && !hasSelection) e.preventDefault();
});

const app = createApp(App);
app.use(createPinia());
app.use(router);
app.mount('#app');
