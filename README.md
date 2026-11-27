# Receive and Contacts at narrow window widths

Before revision: `fca6fa6b12f5e38b7c84b8d9e422972a213467cd`

After revision: `9ce4b96c42b178280855a6781774260e20329554`

Headless Chromium, en-US, UTC, 1× scale, with fixture data and matching viewport and theme for each pair.

| State | Viewport | Fixture |
| --- | --- | --- |
| Receive, Dash request form with the USD toggle | 900×1100 and 920×1100 | `fiat-contacts` |
| Receive, Platform address | 920×1100 | `funded` |
| Receive, Shielded address during the first scan | 920×1100 | `shielded-syncing` |
| Contacts, incoming requests | 920×600 | `dashpay-registered` |
| Contacts, Bob Builder selected | 920×1100 | `dashpay-registered` |

Each pair uses the same file name in the `before` and `after` directories. The light and dark variants are in both directories.
