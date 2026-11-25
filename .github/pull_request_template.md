<!-- Lead with the problem this solves, then what changes. -->

## Visual and e2e

<!-- Required when this PR touches apps/desktop/src or apps/desktop/e2e. Replace N, M, <specs> and <sha> with the counts from the e2e-docker.sh runs at the PR head: the full visual suite, then the e2e specs of every screen the PR changes, on both browsers. Delete this section only when neither path changes. -->
Visual: N passed, 0 failed; e2e <specs>: M passed, 0 failed (Chromium + WebKit) at <sha>

## Checklist

- [ ] Tests cover the change.
- [ ] UI changes only: light and dark screenshots or a short recording of every changed state are on this PR, with before (base) and after (head) for changed states. Images are on a non-main assets branch, linked by SHA-pinned raw URLs, fixture data only, each captioned with its state and revision. See CLAUDE.md, "UI artifacts on every UI pull request".
