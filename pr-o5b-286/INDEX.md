# Registration progress focus after the passphrase (issue 286)

- Before: exact base `df04daea928618afd5fef319c0b053762552e7c0` (the PR's merge base)
- After: head `0dcb3d67ff1a60ad8a5aec288811bc9ffd958d78` (fix/286-registration-focus)
- Fixture backend, scenario `dashpay-no-identity`, username `alice-77`, Chromium (Playwright image), 1280×820. Flow: Join DashPay ▸ Create username ▸ `alice-77` ▸ Continue ▸ Confirm & pay ▸ passphrase ▸ Enter; shot 500 ms after the progress screen appears.
- The title has no visible focus ring by design, like the welcome title, so the plain shots look the same. Each `focus-marked` twin adds a magenta dashed outline on `:focus`, injected by the capture script. That outline is an annotation, not app UI.

| State | Theme | Before (base df04daea9): focus on `<body>` | After (head 0dcb3d67f): focus on "Creating your username" |
|---|---|---|---|
| Progress screen after the passphrase | light | before/progress-light-base-df04daea9.png | after/progress-light-head-0dcb3d67f.png |
| Same, focus marked | light | before/progress-light-focus-marked-base-df04daea9.png | after/progress-light-focus-marked-head-0dcb3d67f.png |
| Progress screen after the passphrase | dark | before/progress-dark-base-df04daea9.png | after/progress-dark-head-0dcb3d67f.png |
| Same, focus marked | dark | before/progress-dark-focus-marked-base-df04daea9.png | after/progress-dark-focus-marked-head-0dcb3d67f.png |
