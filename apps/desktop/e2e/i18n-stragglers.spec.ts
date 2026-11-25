import { expect, test, type Page } from "@playwright/test";
import { FIXTURE_NOW } from "../src/fixture/wallet";
import { fixtureUrl, openTransfer, THEMES, trackPageErrors, type Theme } from "./helpers";
import { checkArea, PSEUDO } from "./pseudo";

// Localization (MP-14): the copy that lives outside the screen files, under the two pseudo-locales
// (./pseudo.ts): Shielded's syncing text and sync line where Receive, Transfer, Send from Shielded and
// Home show them, Explore's placeholder, and the shell's toasts for a lock the engine refused and a
// link that was not opened.

/** Amounts and counts. */
// Amounts, and the Shielded address Receive shows while the first scan runs.
const DATA = [/^[+−-]?[\p{Nd}.,٫٬\s]+( (t?DASH|mDASH|μDASH|duffs))?$/u, /^t?dash1[a-z0-9]{15,}$/];

const command = (page: Page, c: object) => page.evaluate((c) => window.__dwFixture!.shellCommand(c as never), c);

const open = async (page: Page, scenario: string, theme: Theme, lang: string) => {
  await page.clock.setFixedTime(FIXTURE_NOW);
  await page.goto(fixtureUrl(scenario, theme, `&lang=${lang}`));
  await page.getByTestId("toolbar").waitFor();
};

const send = async (page: Page) => {
  await page.getByTestId("sidebar-pay").click();
  await page.getByTestId("shielded-send").click();
  await expect(page.getByTestId("shielded-sheet")).toBeVisible();
};

interface State {
  scenario: string;
  roots: string[];
  go: (page: Page) => Promise<void>;
}

export const STATES: Record<string, State> = {
  explore: {
    scenario: "funded",
    roots: ["page-explore"],
    go: async (p) => {
      await command(p, { kind: "navigate", route: "explore" });
      await expect(p.getByTestId("placeholder-explore")).toBeVisible();
    },
  },
  "home-syncing": { scenario: "shielded-syncing-pending", roots: ["unfinished-card"], go: (p) => expect(p.getByTestId("shielded-finish")).toBeDisabled() },
  "receive-syncing": {
    scenario: "shielded-syncing-pending",
    roots: ["page-receive"],
    go: async (p) => {
      await p.getByTestId("sidebar-receive").click();
      await p.getByTestId("shielded-receive-open").click();
      await expect(p.getByTestId("shielded-receive-progress")).toBeVisible();
      await expect(p.getByTestId("shielded-receive-address")).toBeVisible();
    },
  },
  "transfer-syncing": {
    scenario: "shielded-syncing-pending",
    roots: ["transfer-sheet"],
    go: async (p) => {
      await openTransfer(p, "coreToShielded");
      await expect(p.getByTestId("transfer-continue")).toBeDisabled();
    },
  },
  "pay-syncing": {
    scenario: "shielded-syncing",
    roots: ["pay-shielded"],
    go: async (p) => {
      await p.getByTestId("sidebar-pay").click();
      await expect(p.getByTestId("shielded-send")).toBeDisabled();
    },
  },
  "send-scanning": { scenario: "shielded-scanning", roots: ["shielded-sheet"], go: async (p) => (await send(p), await expect(p.getByTestId("shielded-sync")).toBeVisible()) },
};

for (const theme of THEMES)
  for (const pseudo of PSEUDO)
    for (const [name, s] of Object.entries(STATES))
      test(`${name} shows only catalog text under ${pseudo[0]}, fits, and passes axe, ${theme}`, async ({ page }, info) => {
        test.setTimeout(120_000);
        const errors = trackPageErrors(page);
        await checkArea(page, info, theme, pseudo, {
          name: `stragglers-${name}`,
          roots: s.roots,
          data: DATA,
          open: async (lang) => {
            await open(page, s.scenario, theme, lang);
            await s.go(page);
          },
        });
        expect(errors).toEqual([]);
      });

const toast = (page: Page) => page.locator(".toast-host");

/** The toasts: English as before, and a whole catalog message under en-XA. */
const TOASTS: Record<string, { english: RegExp; go: (page: Page) => Promise<void> }> = {
  "a lock the engine refused": {
    english: /^Could not lock: fixture: .+$/,
    go: async (p) => {
      await p.evaluate(() => window.__dwFixture!.fail("lock", "call"));
      await p.getByTestId("toolbar-lock").click();
    },
  },
  "links that could not be read": {
    english: /^Could not open the link: fixture: .+$/,
    go: async (p) => {
      await p.evaluate(() => window.__dwFixture!.fail("take_links", "call"));
      await p.evaluate(() => window.__dwFixture!.deliverLinks([{ kind: "scanQr" }]));
    },
  },
  "a link while the wipe sheet is open": {
    english: /^The link was not opened: close “Delete all wallet data\?” first$/,
    go: async (p) => {
      await command(p, { kind: "navigate", route: "settings" });
      await p.getByTestId("settings-tabs-security").click();
      await p.getByTestId("settings-wipe").click();
      await expect(p.getByTestId("settings-wipe-sentence")).toBeVisible();
      await p.evaluate(() => window.__dwFixture!.deliverLinks([{ kind: "scanQr" }]));
    },
  },
};

for (const [name, { english, go }] of Object.entries(TOASTS)) {
  test(`the toast for ${name} reads as before in English`, async ({ page }) => {
    const errors = trackPageErrors(page);
    await open(page, "funded", "light", "en");
    await go(page);
    await expect(toast(page)).toHaveText(english);
    expect(errors).toEqual([]);
  });

  test(`the toast for ${name} is catalog text under en-XA`, async ({ page }) => {
    const errors = trackPageErrors(page);
    await open(page, "funded", "light", "en-XA");
    await go(page);
    await expect(toast(page)).toHaveText(/^⟦.+⟧$/);
    expect(errors).toEqual([]);
  });
}

test("Shielded's syncing text follows a language change with no reload", async ({ page }) => {
  const errors = trackPageErrors(page);
  await open(page, "shielded-syncing-pending", "light", "en");
  await page.getByTestId("sidebar-receive").click();
  await page.getByTestId("shielded-receive-open").click();
  const why = page.getByTestId("shielded-receive-progress");
  await expect(why).toHaveText("Syncing Shielded…");
  await page.evaluate(() => window.__dwFixture!.setLanguage("en-XA"));
  await expect(why).toHaveText(/^⟦.+⟧$/);
  expect(errors).toEqual([]);
});
