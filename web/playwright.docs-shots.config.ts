import { defineConfig, devices } from '@playwright/test';

// The pictures of docs/reference/ui-standard.md, taken from the mocked pages
// of the style snapshot. They are not tests: run them by hand after a change
// to how Krönika looks, and commit the pictures. SHOTS_OUT names another
// folder for them, to compare first.
export default defineConfig({
    testDir: './docs-shots',
    fullyParallel: false,
    workers: 1,
    reporter: 'list',
    use: { baseURL: 'http://127.0.0.1:4174' },
    projects: [
        {
            name: 'chromium',
            use: {
                ...devices['Desktop Chrome'],
                launchOptions: process.env.PLAYWRIGHT_CHROMIUM_EXECUTABLE
                    ? { executablePath: process.env.PLAYWRIGHT_CHROMIUM_EXECUTABLE }
                    : {},
            },
        },
    ],
    webServer: {
        command: 'npm run build && npm run preview -- --host 127.0.0.1 --port 4174',
        url: 'http://127.0.0.1:4174',
        reuseExistingServer: false,
    },
});
