import { resolve } from "node:path";
import react from "@vitejs/plugin-react";
import type { Plugin } from "vite";
import { defineConfig } from "vitest/config";

const repo = resolve(import.meta.dirname, "../..");

// Tauri expects a fixed dev port and serves `dist/` in release builds.
//
// Fixtures (U-07): `__DW_FIXTURES__` is true for the dev server, vitest and
// `vite build --mode fixture` (into `dist-fixture/`, which Playwright serves), and false for
// `vite build`, the production renderer Tauri embeds. Nothing else decides it: no environment
// variable can put the fixture backend into `dist/`. Two checks: `noFixtureModules` fails a
// production build whose chunks hold any `src/fixture/` module or Tauri's mock IPC, whoever
// imports it, and scripts/check-bundle.mjs scans the written files.
const noFixtureModules: Plugin = {
  name: "dw-no-fixture-modules",
  generateBundle(_, bundle) {
    for (const chunk of Object.values(bundle))
      if (chunk.type === "chunk")
        for (const id of Object.keys(chunk.modules))
          if (/[\\/]src[\\/]fixture[\\/]|@tauri-apps[\\/]api[\\/]mocks/.test(id)) this.error(`production build includes fixture code: ${id}`);
  },
};

export default defineConfig(({ command, mode }) => {
  const fixtures = command === "serve" || mode === "fixture";
  return {
    plugins: fixtures ? [react()] : [react(), noFixtureModules],
    define: { __DW_FIXTURES__: JSON.stringify(fixtures) },
    clearScreen: false,
    resolve: {
      alias: { "@icons": resolve(repo, "Resources/Icons") },
    },
    server: {
      port: 1420,
      strictPort: true,
      fs: { allow: [repo] },
    },
    preview: { port: 1420, strictPort: true },
    build: {
      // WebKitGTK 2.4x, WKWebView (macOS 14) and WebView2 all run ES2022.
      target: ["es2022", "safari16"],
      // Icons stay files: the CSP allows `img-src 'self'`, and nothing is inlined into JS.
      assetsInlineLimit: 0,
      sourcemap: false,
      outDir: mode === "fixture" ? "dist-fixture" : "dist",
    },
    test: {
      include: ["src/**/*.test.ts"],
      environment: "node",
    },
  };
});
