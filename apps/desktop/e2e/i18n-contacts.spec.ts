import { expect, test, type Page } from "@playwright/test";
import { authorize, fixtureUrl, THEMES, trackPageErrors, type Theme } from "./helpers";
import { checkArea, PSEUDO } from "./pseudo";

// Localization (MP-14): Contacts under the two pseudo-locales (./pseudo.ts): the list with
// contacts and empty, a contact's and a request's pane, and Add contact's search, found user, no
// match and sent request. States are reached as contacts.spec.ts, contact-pane.spec.ts and
// add-contact.spec.ts reach them.

async function openContacts(page: Page, theme: Theme, lang: string, scenario = "dashpay-registered") {
  await page.goto(fixtureUrl(scenario, theme, `&lang=${lang}`));
  await page.getByTestId("toolbar").waitFor();
  await page.evaluate(() => window.__dwFixture!.shellCommand({ kind: "navigate", route: "contacts" }));
  await expect(page.getByTestId("page-contacts").locator("h1")).toBeFocused();
}

const row = (page: Page, name: string) => page.getByTestId("contact-row").filter({ has: page.locator(".contact-row-name", { hasText: name }) });
const hit = (page: Page, name: string) => page.getByTestId("add-contact-hit").filter({ has: page.locator(".contact-row-name", { hasText: name }) });
const field = (page: Page) => page.getByTestId("add-contact-search").locator("input");

async function openAdd(page: Page, theme: Theme, lang: string, query: string) {
  await openContacts(page, theme, lang);
  await page.getByTestId("contacts-add").click();
  await field(page).fill(query);
}

async function pickMallory(page: Page, theme: Theme, lang: string) {
  await openAdd(page, theme, lang, "mal");
  await hit(page, "Mallory").click();
  await expect(page.getByTestId("add-contact-send")).toBeVisible();
}

/** The fixture's people, their usernames, initials and identity ids. */
const PEOPLE = [
  /^(heidi|Grace Hopper|Bob Builder|Carol|dave|Erin|Frank|Ivan|Judy \(work\)|Mallory|mallory|Marta|mateo|Maxine|Mayumi)$/,
  /^\p{Lu}{2}$/u,
  /^[1-9A-HJ-NP-Za-km-z]{32,}$/,
];

const STATES: Record<string, { open: (page: Page, theme: Theme, lang: string) => Promise<void>; data?: RegExp[] }> = {
  list: {
    open: async (page, theme, lang) => {
      await openContacts(page, theme, lang);
      await expect(page.getByTestId("contacts-group-requests")).toBeVisible();
      await expect(page.getByTestId("contact-pane-empty")).toBeVisible();
    },
  },
  empty: {
    open: async (page, theme, lang) => {
      await openContacts(page, theme, lang, "dashpay-contacts-empty");
      await expect(page.getByTestId("contacts-empty")).toBeVisible();
    },
  },
  pane: {
    open: async (page, theme, lang) => {
      await openContacts(page, theme, lang);
      await row(page, "Bob Builder").click();
      await expect(page.getByTestId("contact-activity-row").first()).toBeVisible();
    },
  },
  request: {
    open: async (page, theme, lang) => {
      await openContacts(page, theme, lang);
      await row(page, "heidi").click();
      await expect(page.getByTestId("contact-accept")).toBeVisible();
    },
  },
  "enable-banner": {
    open: async (page, theme, lang) => {
      await openContacts(page, theme, lang, "dashpay-no-keys");
      await expect(page.getByTestId("contacts-enable-fee")).toBeVisible();
    },
  },
  "add-scan": {
    open: async (page, theme, lang) => {
      await openAdd(page, theme, lang, "");
      await page.getByTestId("add-contact-tabs-scan").click();
      await expect(page.getByTestId("add-contact-drop")).toBeVisible();
    },
  },
  "add-my-qr": {
    open: async (page, theme, lang) => {
      await openAdd(page, theme, lang, "");
      await page.getByTestId("add-contact-tabs-myQr").click();
      await expect(page.getByTestId("add-contact-qr")).toBeVisible();
    },
    data: [/^dashpay:\/\/user\?\S+$/],
  },
  "add-search": {
    open: async (page, theme, lang) => {
      await openAdd(page, theme, lang, "ma");
      await expect(hit(page, "Mayumi")).toBeVisible();
    },
  },
  "add-found": { open: pickMallory },
  "add-none": {
    open: async (page, theme, lang) => {
      await openAdd(page, theme, lang, "zzzz");
      await expect(page.getByTestId("add-contact-search-none")).toBeVisible();
    },
  },
  "add-sent": {
    open: async (page, theme, lang) => {
      await pickMallory(page, theme, lang);
      await page.getByTestId("add-contact-send").click();
      await authorize(page);
      await expect(page.locator(".toast-host")).toBeVisible();
      await expect(page.getByTestId("add-contact-line")).toBeVisible();
    },
  },
};

for (const theme of THEMES)
  for (const pseudo of PSEUDO)
    for (const [name, state] of Object.entries(STATES))
      test(`Contacts ${name} shows only catalog text under ${pseudo[0]}, fits, and passes axe, ${theme}`, async ({ page }, info) => {
        test.setTimeout(120_000);
        const errors = trackPageErrors(page);
        await checkArea(page, info, theme, pseudo, {
          name: `contacts-${name}`,
          roots: ["page-contacts"],
          data: [...PEOPLE, ...(state.data ?? [])],
          open: (lang) => state.open(page, theme, lang),
        });
        expect(errors).toEqual([]);
      });
