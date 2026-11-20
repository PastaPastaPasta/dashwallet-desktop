# Screenshots: localization foundation, merged with main

- before/: base revision 9e7318f4c31ef2eb42018db096332ccb8d8e957a (origin/main)
- after/: head revision ee639692f08c0b5f4397623a065d60a04755d8fc (feat/i18n-foundation)

Fixture build (`pnpm build:fixture`), fixture wallet "@alice" on Testnet, headless Chromium in the
pinned Playwright image, 1280×820, `?theme=light` and `?theme=dark`.

| File | State |
|---|---|
| shell-home-{light,dark} | the shell (toolbar, sidebar, status bar) on Home; pixel-identical before and after |
| lock-screen-{light,dark} | the lock screen; pixel-identical before and after |
| settings-general-language-closed-{light,dark} | Settings ▸ General: before, the "Coming soon" row; after, the Language picker in Display |
| settings-general-language-open-{light,dark} | after only: the Language picker's list (drawn with `size` set, as a headless browser does not draw a native select's popup) |
| pseudo-en-XA-settings-general-{light,dark} | `?lang=en-XA`; before ignores it |
| pseudo-ar-XB-rtl-shell-home-{light,dark} | `?lang=ar-XB` (right to left); before ignores it |
