// The design tokens of each theme, resolved, for code that cannot read a
// CSS custom property: ECharts draws on a canvas. Written by
// scripts/build-tokens.mjs from web/tokens/. Do not edit: change a token
// file and run `npm run build:tokens`.

export const tokens = {
    "dark": {
        "mono": "'SF Mono', 'JetBrains Mono', ui-monospace, Menlo, Consolas, monospace",
        "chart": {
            "font": {
                "family": "-apple-system, 'Segoe UI', Inter, Roboto, sans-serif",
                "weight": 600,
                "size": {
                    "2xs": 9,
                    "xs": 10,
                    "sm": 11,
                    "md": 12,
                    "lg": 15
                }
            },
            "ink-on-fill": "#0b0e11",
            "neutral-dot": "#888888",
            "band": {
                "good": "#009E73",
                "fair": "#E69F00",
                "poor": "#D55E00"
            },
            "unscored": "#5b6772",
            "status": {
                "none": "#D55E00",
                "designed": "#E69F00",
                "existing": "#56B4E9",
                "operating": "#009E73"
            },
            "starmap": {
                "kind": {
                    "org": "#4C8EDA",
                    "entity": "#C990C0",
                    "system": "#7B6CD9",
                    "component": "#57C7E3",
                    "service": "#2A9D8F",
                    "workload": "#B08D57",
                    "ext": "#6E7B8B",
                    "def": "#B8A038",
                    "run": "#9AA5B1",
                    "slo": "#3B5BA9",
                    "article": "#A45EBF",
                    "gap": "#D9B8A0",
                    "cluster": "#2F7F9E"
                },
                "relation": {
                    "owns": "#8B6CE0",
                    "implements": "#0E9488",
                    "calls": "#2F80D0",
                    "runs_on": "#A8752A",
                    "depends_on": "#C0408F",
                    "tested_by": "#6B8E23",
                    "covers": "#1A9BBD",
                    "measures": "#8A7FA8"
                }
            },
            "series": [
                "#5eb1ef",
                "#E69F00",
                "#009E73",
                "#CC79A7",
                "#56B4E9",
                "#D55E00",
                "#F0E442"
            ],
            "extra-series": [
                "#a78bfa",
                "#f472b6"
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
            "font": {
                "family": "-apple-system, 'Segoe UI', Inter, Roboto, sans-serif",
                "weight": 600,
                "size": {
                    "2xs": 9,
                    "xs": 10,
                    "sm": 11,
                    "md": 12,
                    "lg": 15
                }
            },
            "ink-on-fill": "#0b0e11",
            "neutral-dot": "#888888",
            "band": {
                "good": "#009E73",
                "fair": "#E69F00",
                "poor": "#D55E00"
            },
            "unscored": "#5b6772",
            "status": {
                "none": "#D55E00",
                "designed": "#E69F00",
                "existing": "#56B4E9",
                "operating": "#009E73"
            },
            "starmap": {
                "kind": {
                    "org": "#4C8EDA",
                    "entity": "#C990C0",
                    "system": "#7B6CD9",
                    "component": "#57C7E3",
                    "service": "#2A9D8F",
                    "workload": "#B08D57",
                    "ext": "#6E7B8B",
                    "def": "#B8A038",
                    "run": "#9AA5B1",
                    "slo": "#3B5BA9",
                    "article": "#A45EBF",
                    "gap": "#D9B8A0",
                    "cluster": "#2F7F9E"
                },
                "relation": {
                    "owns": "#8B6CE0",
                    "implements": "#0E9488",
                    "calls": "#2F80D0",
                    "runs_on": "#A8752A",
                    "depends_on": "#C0408F",
                    "tested_by": "#6B8E23",
                    "covers": "#1A9BBD",
                    "measures": "#8A7FA8"
                }
            },
            "series": [
                "#1f6fb2",
                "#c28600",
                "#009E73",
                "#CC79A7",
                "#1d9ae0",
                "#D55E00",
                "#9e940d"
            ],
            "extra-series": [
                "#9d7df9",
                "#f258a9"
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
