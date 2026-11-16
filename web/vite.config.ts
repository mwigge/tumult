import adapter from '@sveltejs/adapter-static';
import { sveltekit } from '@sveltejs/kit/vite';
import { defineConfig } from 'vitest/config';

export default defineConfig({
    plugins: [sveltekit({ adapter: adapter({ fallback: '200.html' }) })],
    // Unit tests live next to the code; tests/ holds the Playwright specs.
    test: { include: ['src/**/*.test.ts'] },
    server: {
        // Local dev against a running tumultd.
        proxy: {
            '/api': 'http://127.0.0.1:4318',
        },
    },
});
