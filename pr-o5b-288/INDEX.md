# Confirm payment keeps focus while a contact payment is sent (issue 288)

- Before: exact base `5f5af0ecfcf6b55870ce409e10d811a3b26b22d1` (the PR's merge base)
- After: head `ee8b766015272e547a5af15cc2831df8cf10ca25` (fix/288-confirm-payment-focus)
- Fixture backend, scenario `dashpay-registered`, Chromium (Playwright image), 1280×820. State: Pay ▸ To a contact ▸ Grace Hopper ▸ 0.01 ▸ Review ▸ passphrase ▸ Enter, with the broadcast held so the sheet stays in its sending state.
- The dialog panel has no visible focus ring by design, so the plain shots look the same. In each `-focus` twin, the magenta dashed outline and label are an annotation that the capture script drew around `document.activeElement`. They are not app UI. Before, the outline is the whole page (`<body>`). After, it is the dialog (`pay-confirm`).

| State | Theme | Before (base 5f5af0ecf) | After (head ee8b76601) |
|---|---|---|---|
| Confirm payment while sending | light | before/confirm-sending-light-base-5f5af0ecf.png | after/confirm-sending-light-head-ee8b76601.png |
| Same, focus marked (annotation) | light | before/confirm-sending-light-focus-base-5f5af0ecf.png (`<body>`) | after/confirm-sending-light-focus-head-ee8b76601.png (`pay-confirm`) |
| Confirm payment while sending | dark | before/confirm-sending-dark-base-5f5af0ecf.png | after/confirm-sending-dark-head-ee8b76601.png |
| Same, focus marked (annotation) | dark | before/confirm-sending-dark-focus-base-5f5af0ecf.png (`<body>`) | after/confirm-sending-dark-focus-head-ee8b76601.png (`pay-confirm`) |
