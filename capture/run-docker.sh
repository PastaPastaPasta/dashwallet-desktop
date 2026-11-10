#!/usr/bin/env bash
# run-docker.sh <worktree> <rev> <port>: the row 74 capture spec in the pinned Playwright image, as
# apps/desktop/scripts/e2e-docker.sh runs Playwright (same image, pnpm, build:fixture), plus REV/DW_SHOTS.
set -euo pipefail
wt=$1 rev=$2 port=$3
shift 3
R=/work/scratch/dw-rel/0.1.x/row74
app=$wt/apps/desktop
cp $R/capture/zz-row74-capture.spec.ts $app/e2e/
cat > $app/playwright.cap.config.ts <<CFG
import base from "./playwright.config";
import { defineConfig } from "@playwright/test";
export default defineConfig({
  ...base,
  testMatch: /zz-row74-capture/,
  reporter: [["line"]],
  outputDir: "test-results-cap",
  webServer: { command: "pnpm exec vite preview --mode fixture --port $port --strictPort", port: $port, reuseExistingServer: false },
  use: { ...base.use, baseURL: "http://localhost:$port" },
  projects: [{ name: "chromium", use: { browserName: "chromium" } }],
});
CFG
playwright=$(cd $app && node -p 'require("./package.json").devDependencies["@playwright/test"]')
pnpm_version=$(cd $app && node -p 'require("./package.json").packageManager.split("@")[1]')
image="mcr.microsoft.com/playwright:v${playwright}-noble"
cache=$R/capture/e2e-cache-$rev
shots=${SHOTS_DIR:-$R/capture/shots2}
mkdir -p "$cache/node_modules" "$shots/$rev"
echo "=== $rev $(git -C $wt rev-parse HEAD) image $image $(date -u +%T)"
docker run --rm --init --ipc=host --user "$(id -u):$(id -g)" \
  -e CI=1 -e HOME=/cache/home -e npm_config_cache=/cache/npm -e REV=$rev -e DW_SHOTS=$shots \
  -v "$wt:$wt" -v "$shots:$shots" -v "$cache/node_modules:$app/node_modules" -v "$cache:/cache" -w "$app" "$image" \
  bash -euo pipefail -c '
    npm install --global --prefix /cache/npm-global "pnpm@$0" >/dev/null
    export PATH=/cache/npm-global/bin:$PATH
    pnpm install --frozen-lockfile --store-dir /cache/pnpm-store >/dev/null
    pnpm build:fixture 2>&1 | grep -vE "^\s*$" | tail -40
    pnpm exec playwright test -c playwright.cap.config.ts --workers=3 "${@:1}"
  ' "$pnpm_version" "$@"
