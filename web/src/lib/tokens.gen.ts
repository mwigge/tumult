// The design tokens of each theme, resolved, for code that cannot read a
// CSS custom property: ECharts draws on a canvas. Written by
// scripts/build-tokens.mjs from web/tokens/. Do not edit: change a token
// file and run `npm run build:tokens`.

export const tokens = {
    "dark": {
        "mono": "'SF Mono', 'JetBrains Mono', ui-monospace, Menlo, Consolas, monospace",
        "chart": {
            "series": [
                "#5eb1ef",
                "#E69F00",
                "#009E73",
                "#CC79A7",
                "#56B4E9",
                "#D55E00",
                "#F0E442"
            ]
        },
        "bg": "#0b0e11",
        "bg-raised": "#11161b",
        "bg-panel": "#151b21",
        "bg-hover": "#1c242c",
        "border": "#242d36",
        "border-strong": "#33404c",
        "text": "#d8dee4",
        "text-dim": "#8b98a5",
        "text-faint": "#5b6772",
        "ok": "#34d399",
        "warn": "#f59e0b",
        "fail": "#ef4444",
        "accent": "#5eb1ef",
        "accent-dim": "#2d5d84"
    },
    "light": {
        "mono": "'SF Mono', 'JetBrains Mono', ui-monospace, Menlo, Consolas, monospace",
        "chart": {
            "series": [
                "#5eb1ef",
                "#E69F00",
                "#009E73",
                "#CC79A7",
                "#56B4E9",
                "#D55E00",
                "#F0E442"
            ]
        },
        "bg": "#f4f6f8",
        "bg-raised": "#ffffff",
        "bg-panel": "#ffffff",
        "bg-hover": "#e9eef2",
        "border": "#e3e8ed",
        "border-strong": "#c5ced6",
        "text": "#1b2329",
        "text-dim": "#5b6772",
        "text-faint": "#8b98a5",
        "ok": "#0f8a5f",
        "warn": "#b45309",
        "fail": "#c0392b",
        "accent": "#1f6fb2",
        "accent-dim": "#9cc4e4"
    }
} as const;
