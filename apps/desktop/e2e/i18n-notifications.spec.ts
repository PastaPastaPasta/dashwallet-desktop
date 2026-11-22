import { expect, test, type Page } from "@playwright/test";
import { fixtureUrl, THEMES, trackPageErrors, type Theme } from "./helpers";
import { checkArea, PSEUDO } from "./pseudo";

// Localization (MP-14) for the notifications area: the bell popover and Notifications page
// (Payments, Contact requests, New, Earlier, empty, no-identity and error states) under the
// two pseudo-locales, light and dark, at 1280×820 and 920×600, with axe.

/** Text in the notifications views that is data, not copy: fixture contact names and two-letter
 * initials (`Grace Hopper`, `heidi`, `GH`, `BB`, …). */
const ALLOWED_DATA: RegExp[] = [/^(Grace Hopper|heidi)$/, /^\p{Lu}{2}$/u];

/** Engine-supplied AppError.message from fixture `fail=dashpay_events:none` (not translated). */
const FIXTURE_ERROR: RegExp[] = [/^fixture: an internal error with no data$/];

async function openScenario(page: Page, scenario: string, theme: Theme, lang: string, extra = "") {
  await page.goto(fixtureUrl(scenario, theme, `&lang=${lang}${extra}`));
  await page.getByTestId("toolbar").waitFor();
}

async function openBell(page: Page) {
  const bell = page.getByTestId("toolbar-bell");
  await expect(bell).toHaveAttribute("aria-haspopup", "dialog");
  await bell.click();
  await expect(page.getByTestId("bell-popover")).toBeVisible();
}

async function openPage(page: Page) {
  await page.evaluate(() => window.__dwFixture!.shellCommand({ kind: "navigate", route: "notifications" }));
  await expect(page.getByTestId("page-notifications")).toBeVisible();
}

const receivePayment = (page: Page, duffs: number) => page.evaluate(`window.__dwFixture.receivePayment(${duffs})`) as Promise<string>;

for (const theme of THEMES) {
  for (const pseudo of PSEUDO) {
    const [lang] = pseudo;

    test(`bell popover (payments, requests, new) under ${lang}, ${theme}`, async ({ page }, info) => {
      test.setTimeout(120_000);
      const errors = trackPageErrors(page);
      await checkArea(page, info, theme, pseudo, {
        name: "notifications-popover",
        roots: ["bell-popover"],
        panes: ['[data-testid="bell-popover"]'],
        data: ALLOWED_DATA,
        open: async (l) => {
          await openScenario(page, "dashpay-registered", theme, l);
          await receivePayment(page, 150_000_000);
          await openBell(page);
          await expect(page.getByTestId("bell-payments")).toBeVisible();
          await expect(page.getByTestId("bell-requests")).toBeVisible();
          await expect(page.getByTestId("bell-new")).toBeVisible();
        },
      });
      expect(errors).toEqual([]);
    });

    test(`bell popover without DashPay keys under ${lang}, ${theme}`, async ({ page }, info) => {
      test.setTimeout(120_000);
      const errors = trackPageErrors(page);
      await checkArea(page, info, theme, pseudo, {
        name: "notifications-popover-no-keys",
        roots: ["bell-popover"],
        panes: ['[data-testid="bell-popover"]'],
        data: ALLOWED_DATA,
        open: async (l) => {
          await openScenario(page, "dashpay-no-keys", theme, l);
          await openBell(page);
          await expect(page.getByTestId("bell-requests-enable-first")).toBeVisible();
        },
      });
      expect(errors).toEqual([]);
    });

    test(`bell popover empty under ${lang}, ${theme}`, async ({ page }, info) => {
      test.setTimeout(120_000);
      const errors = trackPageErrors(page);
      await checkArea(page, info, theme, pseudo, {
        name: "notifications-popover-empty",
        roots: ["bell-popover"],
        panes: ['[data-testid="bell-popover"]'],
        open: async (l) => {
          await openScenario(page, "dashpay-bell-empty", theme, l);
          await openBell(page);
          await expect(page.getByTestId("bell-empty")).toBeVisible();
        },
      });
      expect(errors).toEqual([]);
    });

    test(`bell popover error under ${lang}, ${theme}`, async ({ page }, info) => {
      test.setTimeout(120_000);
      const errors = trackPageErrors(page);
      await checkArea(page, info, theme, pseudo, {
        name: "notifications-popover-error",
        roots: ["bell-popover"],
        panes: ['[data-testid="bell-popover"]'],
        data: FIXTURE_ERROR,
        open: async (l) => {
          await openScenario(page, "dashpay-registered", theme, l, "&fail=dashpay_events:none");
          await openBell(page);
          await expect(page.getByTestId("bell-popover").locator(".form-problem")).toBeVisible();
        },
      });
      expect(errors).toEqual([]);
    });

    test(`notifications page (payments, requests, new, earlier) under ${lang}, ${theme}`, async ({ page }, info) => {
      test.setTimeout(120_000);
      const errors = trackPageErrors(page);
      await checkArea(page, info, theme, pseudo, {
        name: "notifications-page",
        roots: ["page-notifications"],
        data: ALLOWED_DATA,
        open: async (l) => {
          await openScenario(page, "dashpay-registered", theme, l);
          await receivePayment(page, 150_000_000);
          await openPage(page);
          await expect(page.getByTestId("bell-payments")).toBeVisible();
          await expect(page.getByTestId("notifications-requests")).toBeVisible();
          await expect(page.getByTestId("notifications-new")).toBeVisible();
          await expect(page.getByTestId("notifications-earlier")).toBeVisible();
        },
      });
      expect(errors).toEqual([]);
    });

    test(`notifications page empty under ${lang}, ${theme}`, async ({ page }, info) => {
      test.setTimeout(120_000);
      const errors = trackPageErrors(page);
      await checkArea(page, info, theme, pseudo, {
        name: "notifications-page-empty",
        roots: ["page-notifications"],
        open: async (l) => {
          await openScenario(page, "dashpay-bell-empty", theme, l);
          await openPage(page);
          await expect(page.getByTestId("notifications-empty")).toBeVisible();
        },
      });
      expect(errors).toEqual([]);
    });

    test(`notifications page without an identity under ${lang}, ${theme}`, async ({ page }, info) => {
      test.setTimeout(120_000);
      const errors = trackPageErrors(page);
      await checkArea(page, info, theme, pseudo, {
        name: "notifications-page-no-identity",
        roots: ["page-notifications"],
        open: async (l) => {
          await openScenario(page, "dashpay-no-identity", theme, l);
          await page.getByTestId("toolbar-bell").click();
          await expect(page.getByTestId("notifications-no-identity")).toBeVisible();
        },
      });
      expect(errors).toEqual([]);
    });

    test(`notifications page error under ${lang}, ${theme}`, async ({ page }, info) => {
      test.setTimeout(120_000);
      const errors = trackPageErrors(page);
      await checkArea(page, info, theme, pseudo, {
        name: "notifications-page-error",
        roots: ["page-notifications"],
        data: FIXTURE_ERROR,
        open: async (l) => {
          await openScenario(page, "dashpay-registered", theme, l, "&fail=dashpay_events:none");
          await openPage(page);
          await expect(page.getByTestId("notifications-error")).toBeVisible();
        },
      });
      expect(errors).toEqual([]);
    });
  }
}

test("received payment toast and row use catalog text under en-XA (normal and discreet)", async ({ page }) => {
  const errors = trackPageErrors(page);
  await openScenario(page, "dashpay-registered", "light", "en-XA");
  await receivePayment(page, 150_000_000);
  await expect(page.locator(".toast")).toHaveText(/^⟦.+1\.50 tDASH.+⟧$/);
  await page.getByTestId("toolbar-discreet").click();
  await expect(page.locator(".toast")).toHaveText(/^⟦[^0-9]+⟧$/);
  await openBell(page);
  await expect(page.getByTestId("bell-payment").locator(".bell-row-title")).toHaveText(/^⟦[^0-9]+⟧$/);
  expect(errors).toEqual([]);
});
