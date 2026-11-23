# Join DashPay keyboard focus while an action runs (issue 316)

Fixture backend, Chromium, 1280×820, light and dark. Fixture data only.

- **Before:** base `e7b53869eb72895ca375006d7523f1d1cd3994f4` (main).
- **After:** head `a25a74e5e83b2c8541f4095e29c513bcb196691a`.

Each state has two images. The plain one is the screen as shot. The `-focus` twin is the same screen with a magenta dashed box and label drawn around `document.activeElement` by the capture script (`capture-316.spec.ts.txt`). The box isn't part of the app.

| State | What it shows | Before | After |
|---|---|---|---|
| continue | Continue pressed with Enter, the passphrase accepted, the resume call held | `<body>` | the screen's title |
| finish | Finish pressed with Enter, the passphrase accepted, the finish call held | `<body>` | the screen's title |
| wedge-ok | The wedge's OK pressed, the passphrase accepted, the start call held | `<body>` | the wedge sheet |
| no-passphrase | Continue on a wallet with no passphrase, the resume call held | `<body>` | the screen's title |
| escape-slow-quote | Continue pressed, the quote slow, then Escape from the passphrase | `<body>` | Continue, with its focus ring |

The title draws no focus ring, so the plain before and after shots of the first four states look the same; the wedge pair is pixel-identical. The escape pair differs by Continue's focus ring.
