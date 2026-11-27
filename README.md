# Transfer progress screenshots for #1136

- Before: `fca6fa6b12f5e38b7c84b8d9e422972a213467cd`
- After: `045e631de77c8ebccaf06048d74ef3ae27914600`

Both builds ran with the desktop fixture backend and the pinned Playwright Chromium image in Docker. The viewport was 1280 × 820 at 1×. Each PNG shows the Transfer sheet in light or dark mode at its original scale. The comparison uses the same wallet fixture, amounts, locale and route for each pair.

Before, the progress screenshot represents the one waiting view that stayed in place during each route. After, the fixture pauses at every reported stage. Both revisions also show the corresponding settled, pending and failed outcomes. The delayed-settlement case withholds the engine event and simulates an event queue overflow in the after fixture. Its before image shows the original success view after the command replied.
