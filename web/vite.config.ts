import { sveltekit } from '@sveltejs/kit/vite';
import { defineConfig } from 'vite';

export default defineConfig({
  plugins: [sveltekit()],
  server: {
    // Local dev against a running kronikad.
    proxy: {
      '/api': 'http://127.0.0.1:4318'
    }
  }
});
