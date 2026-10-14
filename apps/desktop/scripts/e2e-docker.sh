#!/usr/bin/env bash
# Playwright (fixture backend, no engine) in the pinned Playwright image, as CI's
# "UI tests (Playwright, axe)" job runs it: same image, same commands. Arguments go to
# `playwright test`, e.g.
#
#   scripts/e2e-docker.sh                                  # the whole suite
#   DW_VISUAL=1 scripts/e2e-docker.sh --project=chromium e2e/visual.spec.ts
#   DW_VISUAL=1 scripts/e2e-docker.sh --project=chromium --update-snapshots e2e/visual.spec.ts
#
# The image has the browsers and their system libraries; pnpm, its store and the container's
# own node_modules (mounted over the checkout's, so a local run leaves the host install alone)
# live in $DW_E2E_CACHE (default .e2e-cache/, ignored by git). Files are written as the calling
# user.
set -euo pipefail
cd "$(dirname "$0")/.."
app=$PWD
repo=$(cd ../.. && pwd)
playwright=$(node -p 'require("./package.json").devDependencies["@playwright/test"]')
pnpm_version=$(node -p 'require("./package.json").packageManager.split("@")[1]')
image="mcr.microsoft.com/playwright:v${playwright}-noble"
cache=${DW_E2E_CACHE:-$app/.e2e-cache}
mkdir -p "$cache/node_modules"

docker run --rm --init --ipc=host --user "$(id -u):$(id -g)" \
  -e CI=1 -e HOME=/cache/home -e npm_config_cache=/cache/npm -e DW_VISUAL="${DW_VISUAL:-}" \
  -v "$repo:$repo" -v "$cache/node_modules:$app/node_modules" -v "$cache:/cache" -w "$app" "$image" \
  bash -euo pipefail -c '
    npm install --global --prefix /cache/npm-global "pnpm@$0" >/dev/null
    export PATH=/cache/npm-global/bin:$PATH
    pnpm install --frozen-lockfile --store-dir /cache/pnpm-store
    pnpm build:fixture
    pnpm exec playwright test "$@"
  ' "$pnpm_version" "$@"
