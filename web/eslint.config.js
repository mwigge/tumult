import js from '@eslint/js';
import ts from 'typescript-eslint';
import svelte from 'eslint-plugin-svelte';
import globals from 'globals';
import prettier from 'eslint-config-prettier';

export default ts.config(
    {
        ignores: [
            'node_modules/**',
            '.svelte-kit/**',
            'build/**',
            'playwright-report/**',
            'test-results/**',
        ],
    },
    js.configs.recommended,
    ...ts.configs.recommended,
    ...svelte.configs['flat/recommended'],
    prettier,
    ...svelte.configs['flat/prettier'],
    { languageOptions: { globals: { ...globals.browser, ...globals.node } } },
    { files: ['**/*.svelte'], languageOptions: { parserOptions: { parser: ts.parser } } },
);
