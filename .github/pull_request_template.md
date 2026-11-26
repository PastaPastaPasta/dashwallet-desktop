<!-- Lead with the problem this solves, then what changes. -->

## Visual and e2e

<!-- Required when this PR touches apps/desktop/src or apps/desktop/e2e. Replace N, M, <specs> and <sha> with the counts from the e2e-docker.sh runs at the PR head: the full visual suite (its baselines are Chromium only), then the e2e specs of every screen the PR changes and every spec it edits, on Chromium and WebKit. From apps/desktop, the visual suite is `DW_VISUAL=1 scripts/e2e-docker.sh --project=chromium e2e/visual.spec.ts` (without DW_VISUAL=1 every baseline is skipped), and the e2e specs are `scripts/e2e-docker.sh e2e/<spec>.spec.ts`. Delete this section only when neither path changes. -->
Visual: N passed, 0 failed; e2e <specs>: M passed, 0 failed (Chromium + WebKit) at <sha>

## Checklist

- [ ] Tests cover the change.
- [ ] UI changes only: light and dark screenshots or a short recording of every changed state are on this PR, with before (base) and after (head) for changed states. Images are on a non-main assets branch, linked by SHA-pinned raw URLs, fixture data only, each captioned with its state and revision. See CLAUDE.md, "UI artifacts on every UI pull request".
- [ ] Funds paths only (`ci/funds-paths.txt`; check with `git diff --no-renames --name-only -z origin/main... | ci/github/funds-gate.py match -z`): a funds-reviewer verdict and an Opus review are recorded for the head SHA (docs/ci.md, "Funds review gate"). A push makes them stale.
