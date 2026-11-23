#!/usr/bin/env node
// Runs the Tauri CLI against the app crate in the Rust workspace (rust/crates/dw-app) with
// this directory as the frontend. The crate lives in the workspace so it shares Cargo.lock
// and the pinned platform graph with dw-engine; the CLI would otherwise look for src-tauri/.
import { spawnSync } from "node:child_process";
import { dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const appDir = resolve(dirname(fileURLToPath(import.meta.url)), "..");
const env = {
  ...process.env,
  TAURI_APP_PATH: resolve(appDir, "../../rust/crates/dw-app"),
  TAURI_FRONTEND_PATH: appDir,
};
const cli = resolve(appDir, "node_modules/@tauri-apps/cli/tauri.js");
const r = spawnSync(process.execPath, [cli, ...process.argv.slice(2)], { stdio: "inherit", env, cwd: appDir });
process.exit(r.status ?? 1);
