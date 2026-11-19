# Screenshots: localization foundation (branch feat/i18n-foundation)

- before/: base revision 229cc82add38301c3988be7990ace625848d5f08 (origin/main)
- after/: head revision cbaca5135eadde1f86b9e9116e0cb7654a7dbde7 (feat/i18n-foundation)

All shots: fixture build (`pnpm build:fixture`), fixture wallet "@alice" on Testnet, headless Chromium,
1280×820, light and dark via `prefers-color-scheme`.

| File | State |
|---|---|
| general-display-{light,dark} | Settings ▸ General ▸ Display; after adds the Language row |
| general-coming-soon-{light,dark} | Settings ▸ General ▸ Coming soon; before lists "User interface language", after does not |
| pseudo-en-XA-{home,settings,lock}-{light,dark} | after only: `?lang=en-XA` (long, accented, bracketed) |
| pseudo-ar-XB-{home,settings,lock}-{light,dark} | after only: `?lang=ar-XB` (right to left) |
