import { expect, test, type Page } from "@playwright/test";
import { fixtureUrl, THEMES, trackPageErrors, type Theme } from "./helpers";
import { checkArea, PSEUDO } from "./pseudo";

// Localization (MP-14): Receive under the two pseudo-locales (./pseudo.ts): Dash, Platform and
// Shielded tabs, request creation with and without an amount, the request sheet (with username and
// with a URI too long for QR), the requests list, the zero-amount error,
// and an incoming payment banner. States are reached as receive.spec.ts and wallet.spec.ts reach them.

const TAB = { dash: "receive-select-dash", platform: "platform-receive-toggle", shielded: "shielded-receive-open" } as const;

async function openReceive(page: Page, theme: Theme, lang: string, scenario = "funded") {
  await page.goto(fixtureUrl(scenario, theme, `&lang=${lang}`));
  await page.getByTestId("toolbar").waitFor();
  await page.getByTestId("sidebar-receive").click();
  await expect(page.getByTestId("receive-qr")).toBeVisible();
}

async function createRequest(page: Page, fields: { label?: string; amount?: string; message?: string; username?: boolean } = {}) {
  if (fields.label !== undefined) await page.getByTestId("request-label").fill(fields.label);
  if (fields.amount !== undefined) await page.getByTestId("request-amount").fill(fields.amount);
  if (fields.message !== undefined) await page.getByTestId("request-message").fill(fields.message);
  if (fields.username !== undefined) {
    const box = page.getByTestId("request-include-username");
    if ((await box.isChecked()) !== fields.username) await box.click();
  }
  const create = page.getByTestId("request-create");
  await expect(create).toBeEnabled();
  await create.click();
  await expect(page.getByTestId("request-sheet")).toBeVisible();
}

/** Data rendered on Receive: addresses (Core, Platform, Shielded), payment URIs, fiat/unit codes
 * and values, localized request dates, and the fixture label/message strings entered by this spec. */
const DATA = [
  /^[Xy789][1-9A-HJ-NP-Za-km-z]{25,34}$/,
  /^(t?dash1|tevo1|evo1|utest1|u1|z)[a-z0-9]{15,}$/i,
  /^dash:\S+$/,
  /^(USD|EUR|JPY)$/,
  /^[\u200e\u200f]?[\p{Nd}.,\u066b\u066c\s]+(t?DASH|mDASH|μDASH|duffs)$/u,
  /^[\u200e\u200f]?[\p{Nd}.,\u066b\u066c\s]*(US\$|USD|\$)[\u200e\u200f]?$/u,
  /^(?:[A-Z][a-z]{2}\.? \p{Nd}{1,2}, \p{Nd}{4}|\p{Nd}{1,2} [A-Z][a-z]{2}\.? \p{Nd}{4})$/u,
  /^(Invoice 104|Design work|(word ){47}word)$/,
];

/** Still English, from modules outside the receive area:
 * - shielded/flow.ts (`SHIELDED_COPY.receive.text`)
 * - screens/notifications/Received.tsx (`receivedText`) */
const SHARED = [
  /^Money sent to this address stays private\. Anyone with a Shielded balance can pay it\.$/,
  /^Received [\p{Nd}.,\u066b\u066c]+ (t?DASH|mDASH|μDASH|duffs)$/u,
];

const SHEET = '[data-testid="request-sheet"]';

const STATES: Record<string, { open: (page: Page, theme: Theme, lang: string) => Promise<void>; roots: string[]; panes?: string[]; data?: RegExp[] }> = {
  dash: {
    roots: ["page-receive"],
    open: (page, theme, lang) => openReceive(page, theme, lang),
  },
  platform: {
    roots: ["page-receive"],
    open: async (page, theme, lang) => {
      await openReceive(page, theme, lang);
      await page.getByTestId(TAB.platform).click();
      await expect(page.getByTestId("platform-receive-qr")).toBeVisible();
    },
  },
  shielded: {
    roots: ["page-receive"],
    open: async (page, theme, lang) => {
      await openReceive(page, theme, lang);
      await page.getByTestId(TAB.shielded).click();
      await expect(page.getByTestId("shielded-receive-qr")).toBeVisible();
    },
    data: SHARED,
  },
  "request-amount-sheet": {
    roots: ["request-sheet"],
    panes: [SHEET],
    open: async (page, theme, lang) => {
      await openReceive(page, theme, lang, "dashpay-registered");
      await createRequest(page, { label: "Invoice 104", amount: "1.5", message: "Design work", username: true });
      await expect(page.getByTestId("request-qr")).toBeVisible();
      await expect(page.getByTestId("request-username")).toBeVisible();
    },
  },
  "request-any-sheet": {
    roots: ["request-sheet"],
    panes: [SHEET],
    open: async (page, theme, lang) => {
      await openReceive(page, theme, lang);
      await createRequest(page);
      await expect(page.getByTestId("request-qr")).toBeVisible();
    },
  },
  "requests-list": {
    roots: ["page-receive"],
    open: async (page, theme, lang) => {
      await openReceive(page, theme, lang);
      await createRequest(page, { label: "Invoice 104", amount: "1.5", message: "Design work" });
      await page.keyboard.press("Escape");
      await expect(page.getByTestId("request-sheet")).toHaveCount(0);
      await createRequest(page);
      await page.keyboard.press("Escape");
      await expect(page.getByTestId("request-sheet")).toHaveCount(0);
      await expect(page.getByTestId("requests-table").locator("tbody tr")).toHaveCount(2);
    },
  },
  "amount-zero": {
    roots: ["page-receive"],
    open: async (page, theme, lang) => {
      await openReceive(page, theme, lang);
      await page.getByTestId("request-amount").fill("0");
      await expect(page.locator(".field-error")).toBeVisible();
    },
  },
  "qr-too-long": {
    roots: ["request-sheet"],
    panes: [SHEET],
    open: async (page, theme, lang) => {
      await openReceive(page, theme, lang);
      await createRequest(page, { message: "word ".repeat(48).trim() });
      await expect(page.locator(".qr-problem")).toBeVisible();
    },
  },
  received: {
    roots: ["page-receive"],
    open: async (page, theme, lang) => {
      await openReceive(page, theme, lang);
      const paidTo = (await page.getByTestId("receive-address").textContent())!;
      await page.evaluate(() => void window.__dwFixture!.hold("current_receive_address"));
      await page.evaluate(`window.__dwFixture.receivePayment(150000000, undefined, ${JSON.stringify(paidTo)})`);
      await expect(page.getByTestId("receive-received")).toBeVisible();
    },
    data: SHARED,
  },
};

for (const theme of THEMES)
  for (const pseudo of PSEUDO)
    for (const [name, state] of Object.entries(STATES))
      test(`Receive ${name} shows only catalog text under ${pseudo[0]}, fits, and passes axe, ${theme}`, async ({ page }, info) => {
        test.setTimeout(120_000);
        const errors = trackPageErrors(page);
        await checkArea(page, info, theme, pseudo, {
          name: `receive-${name}`,
          roots: state.roots,
          panes: state.panes,
          data: [...DATA, ...(state.data ?? [])],
          open: (lang) => state.open(page, theme, lang),
        });
        expect(errors).toEqual([]);
      });
