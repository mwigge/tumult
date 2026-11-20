import { defineConfig, devices } from '@playwright/test';

// The tests of Krönika against a real daemon. They start `tumultd`, which
// serves the pages and the API, and fill its lake with a real run, so they
// need the two binaries: TUMULTD_BIN and TUMULT_BIN name them.
export default defineConfig({
    testDir: './live-tests',
    fullyParallel: false,
    forbidOnly: Boolean(process.env.CI),
    retries: process.env.CI ? 1 : 0,
    workers: 1,
    reporter: 'list',
    use: { trace: 'retain-on-failure' },
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
});
