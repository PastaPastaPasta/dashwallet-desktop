# Localization (MP-14)

The renderer's own screens are localized through one typed message catalog. English is the source and
the fallback; the translations are made in this repository (docs/PLAN-1.0.md, lane P). This is how it
works, how a screen is converted, and what stays English.

## The pieces

- `apps/desktop/src/i18n/en.ts`: the English catalog, keys grouped by screen area.
- `apps/desktop/src/i18n/index.tsx`: the languages this build ships (`LANGUAGES`), the language in use,
  `t`, `useT` and `<html lang dir>`.
- `apps/desktop/src/i18n/core.ts`: placeholders, plurals, rich text and the pseudo-locales.
- `apps/desktop/src/i18n/glossary.md`: the terms that stay fixed across languages.
- `apps/desktop/src/i18n/converted.test.ts`: the converted paths and the check that keeps them
  converted.

No runtime dependency: placeholders, plurals (`Intl.PluralRules`) and tags take under a hundred lines,
and the typed keys and parameters come from the catalog itself. A library would bring its own message
syntax and a loader.

## The language in use

Settings ▸ General ▸ Language offers System default, then each shipped language named in its own
language. The choice is saved with the other display settings (`UiSettings.language` in dw-app's
`settings.json`, a BCP 47 tag; `null` follows the system). System default takes the system's first
language that has a **complete** catalog, by tag and then by language alone (`pt-PT` takes `pt`), and
English otherwise. Reset options goes back to System default.

The language sets `<html lang>` and `dir`, and the locale numbers, dates and fiat are formatted with
(`Backend.locale`): the system's while the language follows the system or is the system's own language
(so `en-GB` keeps its date order), the chosen language's otherwise.

## Adding a string

Put the English text in `en.ts` under the screen area's group, and read it in the component with `useT`:

```tsx
const t = useT();
<h2>{t("settings.general.display")}</h2>
<p>{t("network.confirmTitle", { network: networkName(to) })}</p>
```

- **Placeholders** are named: `"Switch to {network}?"`. `t` requires exactly the placeholders the
  English text has; numbers are formatted for the locale. Never build a sentence from pieces
  (`"Switch to " + name`): word order differs between languages.
- **Plurals** are an object chosen by `count` through `Intl.PluralRules`:
  `peers: { one: "{count} peer", other: "{count} peers" }`, read as `t("shell.status.peers", { count })`.
  English needs `one` and `other`; a translation gives the forms its language has (`few`, `many`, …).
- **Rich text**: a sentence with a link or bold part is one message with tags,
  `"Read the <link>guide</link> first."`, read with
  `t.rich("…", { link: (text) => <a href={…}>{text}</a> })`. Placeholders of `t.rich` may be elements.
- A text module that is read outside a component (`NetworkText`, `LockText`) reads `t` in a getter or a
  function, so it is read in the language of the render that reads it. A table becomes
  `messageTable(prefix, keys)`.
- In a right-to-left language each placeholder value is a bidi isolate, so a username or address keeps
  its own direction. Text that is only data (a wallet name, `@username`) goes in `<bdi>`.

`Catalog<typeof en>` types every other language: a key English does not have fails `tsc`, and a missing
key shows English. A test checks that each translation uses only English's placeholders and tags.

## Converting a screen area

1. Wait until the area's open pull requests have merged.
2. Move every user-visible string of the area's components into `en.ts`: JSX text, `title`,
   `aria-label`, `placeholder`, `alt`, and the copy props of our components (`label`, `text`, `why`,
   `confirm`, …), plus the text modules they render. Keep the English text exactly as it was unless the
   pull request says why it changes.
3. Add the area's directories or files to `CONVERTED` in `converted.test.ts`. The test fails for any JSX
   text or text-attribute literal left in them; a literal that must stay goes in `ALLOWED` with a reason.
4. Check the area under the pseudo-locales in the fixture build: `?lang=en-XA` brackets, accents and
   lengthens every catalog string by 40%, so text that bypasses the catalog stays plain ASCII and a
   cut-off string loses its closing bracket; `?lang=ar-XB` turns the window right to left. Use logical CSS
   (`margin-inline-start`, `text-align: start`, `inset-inline-end`), never left and right.
5. Extend `e2e/i18n.spec.ts` to the area: no untranslated text visible under either pseudo-locale, no
   sideways overflow at 1280×820 and at the 920×600 minimum, axe clean, light and dark.

An area is done when its paths are in `CONVERTED`, the e2e checks pass for it, and its screenshots under
both pseudo-locales are on the pull request.

Converted so far: the shell (sidebar, toolbar, status bar), the lock screen and the unlock sheet,
Settings' tabs and Settings ▸ General, with the text modules they render (`network/choice.ts`,
`onboarding/lock.ts`, `format/sync.ts`, the lock report's names, `fiat/rates.tsx`).

## Not translated in 1.0

- Messages that arrive from the engine or the network as text rather than a code: an `AppError`'s
  `message`, a node's reject reason, peer and governance text, a proposal's name and URL.
- Log text, `debug.log`, and the console under Tools.
- Data: names, usernames, addresses, transaction ids, paths, currency codes, units, and each language's
  own name in the picker.
- The native menus and the tray, which dw-app builds in Rust.

The release notes say the translations are machine-made and that corrections are welcome as pull requests.
