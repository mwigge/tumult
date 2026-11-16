import { readFileSync } from 'node:fs';
import { defineConfig } from 'vite';
import type { Plugin } from 'vite';

function packaged(path: string): string {
    return readFileSync(new URL(`./node_modules/${path}`, import.meta.url), 'utf8').trim();
}

// The notices the licences of the libraries ask a copy to carry.
function banner(): string {
    const version = (name: string) => JSON.parse(packaged(`${name}/package.json`)).version;
    return [
        '/*!',
        'The script of the Tumult HTML report. Built from web/src/lib/report/main.ts',
        'by `npm run build:report`. Do not edit.',
        '',
        `It holds Apache ECharts ${version('echarts')} (Apache License 2.0,`,
        'https://www.apache.org/licenses/LICENSE-2.0):',
        '',
        packaged('echarts/NOTICE'),
        '',
        'Apache ECharts embeds code of d3.js:',
        '',
        packaged('echarts/licenses/LICENSE-d3'),
        '',
        `It holds ZRender ${version('zrender')}:`,
        '',
        packaged('zrender/LICENSE'),
        '*/',
        '',
    ].join('\n');
}

// The minifier drops comments, so the banner goes on after it.
function licenceBanner(): Plugin {
    return {
        name: 'tumult-report-licence-banner',
        generateBundle(_options, bundle) {
            for (const file of Object.values(bundle)) {
                if (file.type === 'chunk') file.code = banner() + file.code;
            }
        },
    };
}

// The script of the CLI HTML report: ECharts with the charts a report
// draws, as one file that needs nothing else. `tumult-cli` embeds the file,
// so it is committed; CI builds it again and fails when it differs.
export default defineConfig({
    // ECharts asks for this at run time; a browser has no `process`.
    define: { 'process.env.NODE_ENV': '"production"' },
    plugins: [licenceBanner()],
    build: {
        lib: {
            entry: 'src/lib/report/main.ts',
            formats: ['iife'],
            name: 'TumultReportCharts',
            fileName: () => 'report-charts.js',
        },
        outDir: '../tumult-cli/src/commands/report/html',
        emptyOutDir: false,
        copyPublicDir: false,
    },
});
