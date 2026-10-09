export default [
  {
    files: ["web/app.js"],
    languageOptions: {
      ecmaVersion: "latest",
      sourceType: "module",
      globals: Object.fromEntries(
        ["document", "window", "fetch", "setInterval", "console"].map(
          (name) => [name, "readonly"],
        ),
      ),
    },
    rules: {
      "no-undef": "error",
      "no-unused-vars": "error",
      "no-eval": "error",
    },
  },
  {
    files: ["tests/ui.test.mjs"],
    languageOptions: {
      ecmaVersion: "latest",
      sourceType: "module",
      globals: Object.fromEntries(
        [
          "process",
          "Buffer",
          "window",
          "document",
          "localStorage",
          "sessionStorage",
          "innerWidth",
        ].map((name) => [name, "readonly"]),
      ),
    },
    rules: { "no-undef": "error", "no-unused-vars": "error" },
  },
];
