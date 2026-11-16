import { defineConfig, devices } from '@playwright/test';

// The tests of the HTML report `tumult report` writes. They open a report
// from its file, so they need no server: they need the binary, which
// TUMULT_BIN names.
export default defineConfig({
    testDir: './report-tests',
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
