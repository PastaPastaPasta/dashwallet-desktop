# Localization (MP-14)

The renderer's own screens are localized through one typed message catalog. English is the source and
the fallback; the translations are made in this repository (docs/PLAN-1.0.md, lane P). This is how it
works, how a screen is converted, and what stays English.

## The pieces

- `apps/desktop/src/i18n/en.ts`: the English catalog, composed of one file per screen area under
  `apps/desktop/src/i18n/catalog/` (`catalog/pay.ts` holds the `pay.*` keys).
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
`settings.json`; `null` follows the system). The tag grammar dw-app accepts is deliberately narrower
than BCP 47: a 2–3 letter language, then subtags of 1–8 letters or digits, each single-character
subtag (an extension or private-use singleton) followed by a longer subtag, at most 35 bytes. So
`de-u-co-phonebk` and `en-x-wallet` are accepted, `en-u`, `en-x` and private-use-only tags
(`x-wallet`) are not. The grammar is a check of shape, not of the registry: it lets through tags such as
`con` and `nul`, which are reserved file names on Windows. No path is built from a tag today; one that
ever is gets a fixed prefix. A `language` of the wrong
type in a hand-edited file, of any shape or depth, reads as `null` and leaves
the other settings as saved; an IPC write with one is refused. System default takes the system's first
language that has a **complete** catalog, by tag and then by language alone in a script that does not conflict (`pt-PT` takes `pt`, `zh-Hans-CN` takes
`zh-Hans`, `zh-Hant-TW` does not), and
English otherwise. Reset options goes back to System default. A saved tag is matched to its
language whatever its case (`EN` shows as English); a saved tag this build does not ship shows as its
own row ("pt-BR (not available, using System default)") until another choice replaces it.

The language sets `<html lang>` and `dir`, and the locale numbers, dates and fiat are formatted with
(`Backend.locale`): the system's while the language follows the system or is the system's own language
(so `en-GB` keeps its date order; a system language in a conflicting script, `zh-TW` under a chosen
`zh-Hans`, does not count as its own), the chosen language's otherwise. Under System default, numbers
follow the system's formats even when the system's language has no catalog and the text falls back to
English.

The first local currency (prices turned on with no currency saved) comes from the system's region
(`Backend.regionLocale`), never from the UI language; a saved currency always wins. The region is the one
the webview reports (its first language with a region, else its default locale), which can differ
from the operating system's own region setting. Currency names,
prices and dates use the display locale.

## Adding a string

Put the English text in the screen area's file under `i18n/catalog/`, and read it in the component with `useT`:

```tsx
const t = useT();
<h2>{t("settings.general.display")}</h2>
<p>{t("network.confirmTitle", { network: networkName(to) })}</p>
```

- **Placeholders** are named: `"Switch to {network}?"`. `t` requires exactly the placeholders the
  English text has; numbers are formatted for the locale. A number placeholder uses `Intl.NumberFormat`'s default
  precision, so it is for counts only: an amount goes through the amount formatter (`format/amount.ts`)
  and is passed as the formatted string. Never build a sentence from pieces
  (`"Switch to " + name`): word order differs between languages.
- **Plurals** are an object chosen by a numeric `count` (a string `count` fails `tsc`) through
  `Intl.PluralRules`:
  `peers: { one: "{count} peer", other: "{count} peers" }`, read as `t("shell.status.peers", { count })`.
  English needs `one` and `other`; a translation gives the forms its language has (`few`, `many`, …).
  A key a translation lacks falls back to the English message, whose form is chosen by English's
  rules (a Russian window shows "21 peers", not "21 peer"); the number keeps the language's format.
- **Rich text**: a sentence with a link or bold part is one message with tags,
  `"Read the <link>guide</link> first."`, read with
  `t.rich("…", { link: (text) => <a href={…}>{text}</a> })`. Placeholders of `t.rich` may be elements.
  A tag's renderer and a placeholder's value are read from the values' own properties only; a tag
  without one keeps its text unwrapped.
- A text module that is read outside a component (`NetworkText`, `LockText`) reads `t` in a getter or a
  function, so it is read in the language of the render that reads it. A table becomes
  `messageTable(prefix, keys)`.
- In a right-to-left language each placeholder value that is text or a number is a bidi isolate (U+2068 …
  U+2069); in a left-to-right language only a text value with right-to-left letters or a bidi control is
  (an Arabic or Hebrew username), so English text is unchanged. An element given to `t.rich` is wrapped in
  `<bdi>` in every language. A message a right-to-left catalog lacks falls back to English and is drawn
  left to right as a whole (U+2066 … U+2069). The isolate characters stay in `aria-label` and `title`
  values; screen readers ignore them, and a test comparing an exact accessible name under `ar-XB` includes
  them. Text that is only data outside a message (a wallet name, `@username`) goes in `<bdi>`. An amount
  (`AmountText`, and the Activity table's amount cell) is a left-to-right isolate, so a debit reads
  "-1.25" in a right-to-left window.

`Catalog<typeof en>` types every other language: a key English does not have fails `tsc`, and a missing
key shows English. A test checks that each translation uses only English's placeholders and tags.

## Converting a screen area

1. Wait until the area's open pull requests have merged.
2. Move every user-visible string of the area's components into the area's `i18n/catalog/<area>.ts`: JSX text, `title`,
   `aria-label`, `placeholder`, `alt`, and the copy props of our components (`label`, `text`, `why`,
   `confirm`, …), plus the text modules they render. Keep the English text exactly as it was unless the
   pull request says why it changes.
3. Add the area's directories or files to its list in `AREAS` in `converted.test.ts`. The test fails for any JSX
   text or text-attribute literal left in them; a literal that must stay goes in `ALLOWED` with a reason.
4. Check the area under the pseudo-locales in the fixture build: `?lang=en-XA` brackets, accents and
   lengthens every catalog string by 40%, so text that bypasses the catalog stays plain ASCII and a
   cut-off string loses its closing bracket; `?lang=ar-XB` turns the window right to left. Use logical CSS
   (`margin-inline-start`, `text-align: start`, `inset-inline-end`), never left and right.
5. Add `e2e/i18n-<area>.spec.ts`, calling `checkArea` from `e2e/pseudo.ts` for each state the area shows: no untranslated text visible under either pseudo-locale, no
   sideways overflow at 1280×820 and at the 920×600 minimum, axe clean, light and dark.

An area is done when its paths are in `CONVERTED`, the e2e checks pass for it, and its screenshots under
both pseudo-locales are on the pull request.

Converted so far: the shell (sidebar, toolbar, status bar), the lock screen and the unlock sheet,
Settings' tabs and Settings ▸ General, with the text modules they render (`network/choice.ts`,
`onboarding/lock.ts`, `format/sync.ts`, the lock report's names, `fiat/rates.tsx`).

Still English inside those areas, for the slices that convert the shared components they come from:

- the full-unlock sheet's passphrase field label, its busy button ("Checking…") and the sheets' Close
  button name (`components/passphrase.tsx`, `components/sheet.tsx`);
- "Copy …" on copy buttons and the "Copied" toast (`components/detail.tsx`, `components/toast.tsx`);
- "the payment" inside the refused network switch's message (`network/switch.ts`);
- the renderer's own error texts by code, such as the dust setting's failure (`failureText`,
  `onboarding/model.ts`);
- the status bar's governance item (`screens/masternodes/StatusItem.tsx`) and the bell's popover.

English pixel identity holds for the states shot on the pull request, not for every state: numbers in
catalog messages are now formatted for the locale, so a peer count of 1234 reads "1,234 peers".

### What the checks catch

- `converted.test.ts` reads each converted `.tsx` file and fails on JSX text, on a string literal in a
  text attribute or copy prop, on a template literal, on both literal branches of `?:`, on a literal
  on either side of `??`, `||`, `&&` or a `+` concatenation, and on a literal handed to `showToast`.
  It does not see text built elsewhere and passed in: a helper's return value (`title={helper()}`), an
  imported constant, or an error-code mapping in a `.ts` module.
- `e2e/pseudo.ts` (`checkArea`, used by `e2e/i18n.spec.ts` and each area's spec) fails on a root that
  is not on the page, and walks the visible text nodes under the toolbar, sidebar, status bar and the page
  or lock screen, and fails on plain Latin text that carries neither pseudo-locale's mark. A node with a
  mark anywhere passes whole, so English interpolated into a translated sentence is not caught, and a
  clipped string still has its closing bracket in the DOM. Accessible names, placeholders, unselected
  options and states the spec does not open (the
  unlock sheet, error and copy states) are not checked.
- The overflow check runs at 1280×820 and 920×600 and fails on sideways scrolling and on an element
  that sticks out of the toolbar, sidebar, status bar, page or lock column, or a pane an
  area's spec adds (a sheet, a dialog).
- axe runs at 1280×820 only, for Settings ▸ General and the lock screen under both pseudo-locales,
  light and dark, and fails on serious and critical violations of the WCAG 2.0/2.1 A and AA and
  best-practice rules.
- Settings' tabs follow the tablist's direction: in a right-to-left window ArrowRight moves to the
  tab drawn on the right, the previous one.

## Not translated in 1.0

- Messages that arrive from the engine or the network as text rather than a code: an `AppError`'s
  `message`, a node's reject reason, peer and governance text, a proposal's name and URL.
- Log text, `debug.log`, and the console under Tools.
- Data: names, usernames, addresses, transaction ids, paths, currency codes, units, and each language's
  own name in the picker.
- The native menus and the tray, which dw-app builds in Rust.

The release notes say the translations are machine-made and that corrections are welcome as pull requests.
