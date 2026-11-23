import { expect, test, type Locator, type Page } from "@playwright/test";
import { FIXTURE_PASSPHRASE as PASS } from "../src/fixture/scenarios";
import { SecurityText } from "../src/settings/security";
import { fixtureUrl, THEMES, trackPageErrors } from "./helpers";
import { checkArea, PSEUDO } from "./pseudo";

// Localization (MP-14): Settings ▸ Security under the two pseudo-locales (./pseudo.ts): its cards,
// the payment and spending-limit confirmations, Encrypt, Change passphrase and the wipe, reached as
// settings.spec.ts reaches them; and the wipe's typed sentence in another language.

/** Amounts, wallet names, passphrase-free paths and durations' numbers. */
const DATA = [/^[\p{Nd}.,٫٬\s]+$/u, /^(Main|Savings)$/, /^\/\S+$/];

const command = (page: Page, c: object) => page.evaluate((c) => window.__dwFixture!.shellCommand(c as never), c);

async function toSecurity(page: Page) {
  await page.getByTestId("toolbar").waitFor();
  await command(page, { kind: "navigate", route: "settings" });
  await page.getByTestId("settings-tabs-security").click();
  await expect(page.getByTestId("settings-encryption-state")).toBeVisible();
}

const failNext = (page: Page, cmd: string) => page.evaluate((c) => window.__dwFixture!.fail(c as never, "call"), cmd);

/** The wipe's sentence as the sheet shows it, in the language in use. */
const phrase = (page: Page) => page.getByTestId("settings-wipe-phrase").innerText();

async function toWipe(page: Page) {
  await page.getByTestId("settings-wipe").click();
  await expect(page.getByTestId("settings-wipe-sentence")).toBeVisible();
}

async function pastSentence(page: Page) {
  await toWipe(page);
  await page.getByTestId("settings-wipe-sentence").fill(await phrase(page));
  await page.getByTestId("settings-wipe-continue").click();
}

async function wipeTo(page: Page, failing: string) {
  await failNext(page, failing);
  await pastSentence(page);
  await page.getByTestId("settings-wipe-passphrase-field").fill(PASS);
  await page.getByTestId("settings-wipe-passphrase-submit").click();
  await expect(page.getByTestId("settings-wipe-retry")).toBeVisible();
}

interface State {
  scenario: string;
  roots: string[];
  go: (page: Page) => Promise<void>;
}

const CARDS = ["settings-encryption", "settings-unlocking", "settings-payments", "settings-privacy", "settings-wipe-card"];
const state = (scenario: string, roots: string[], go: (p: Page) => Promise<void> = async () => {}): State => ({
  scenario,
  roots,
  go: async (p) => {
    await toSecurity(p);
    await go(p);
  },
});

export const STATES: Record<string, State> = {
  encrypted: state("funded", CARDS),
  unencrypted: state("unencrypted", CARDS),
  "payment-auth": state("funded", ["settings-payment-auth-sheet"], async (p) => {
    await p.getByTestId("settings-payment-auth").click();
    await expect(p.getByTestId("settings-payment-auth-passphrase-field")).toBeVisible();
  }),
  "spend-limit": state("funded", ["settings-spend-limit-sheet"], async (p) => {
    await p.getByTestId("settings-spend-limit").selectOption("100000000");
    await expect(p.getByTestId("settings-spend-limit-pending")).toBeVisible();
  }),
  encrypt: state("unencrypted", ["settings-encrypt-sheet"], async (p) => {
    await p.getByTestId("settings-encrypt").click();
    await expect(p.getByTestId("settings-encrypt-new")).toBeVisible();
  }),
  "encrypt-mismatch": state("unencrypted", ["settings-encrypt-sheet"], async (p) => {
    await p.getByTestId("settings-encrypt").click();
    await p.getByTestId("settings-encrypt-new").fill("correct horse battery staple");
    await p.getByTestId("settings-encrypt-repeat").fill("correct horse battery");
    await p.getByTestId("settings-encrypt-submit").click();
    await expect(p.getByTestId("settings-encrypt-error")).toBeVisible();
  }),
  change: state("funded", ["settings-change-sheet"], async (p) => {
    await p.getByTestId("settings-change-passphrase").click();
    await expect(p.getByTestId("settings-change-current")).toBeVisible();
  }),
  "change-wrong": state("funded", ["settings-change-sheet"], async (p) => {
    await p.getByTestId("settings-change-passphrase").click();
    await p.getByTestId("settings-change-current").fill("wrong passphrase");
    await p.getByTestId("settings-change-new").fill("correct horse battery staple");
    await p.getByTestId("settings-change-repeat").fill("correct horse battery staple");
    await p.getByTestId("settings-change-submit").click();
    await expect(p.getByTestId("settings-change-error")).toBeVisible();
  }),
  "wipe-sentence": state("funded", ["settings-wipe-sheet"], toWipe),
  "wipe-mismatch": state("funded", ["settings-wipe-sheet"], async (p) => {
    await toWipe(p);
    await p.getByTestId("settings-wipe-sentence").fill("I accept");
    await p.getByTestId("settings-wipe-continue").click();
    await expect(p.getByTestId("settings-wipe-error")).toBeVisible();
  }),
  "wipe-passphrase": state("funded", ["settings-wipe-sheet"], async (p) => {
    await pastSentence(p);
    await expect(p.getByTestId("settings-wipe-passphrase-field")).toBeVisible();
  }),
  "wipe-confirm": state("unencrypted", ["settings-wipe-sheet"], async (p) => {
    await pastSentence(p);
    await expect(p.getByTestId("settings-wipe-confirm")).toBeVisible();
  }),
  "wipe-stopped": state("two-wallets", ["settings-wipe-sheet"], (p) => wipeTo(p, "remove_wallet")),
  "wipe-vault-kept": state("funded", ["settings-wipe-sheet"], (p) => wipeTo(p, "destroy_vault")),
};

for (const theme of THEMES)
  for (const pseudo of PSEUDO)
    for (const [name, s] of Object.entries(STATES))
      test(`Settings security ${name} shows only catalog text under ${pseudo[0]}, fits, and passes axe, ${theme}`, async ({ page }, info) => {
        test.setTimeout(120_000);
        const errors = trackPageErrors(page);
        await checkArea(page, info, theme, pseudo, {
          name: `settings-security-${name}`,
          roots: s.roots,
          data: DATA,
          open: async (lang) => {
            await page.goto(fixtureUrl(s.scenario, theme, `&lang=${lang}`));
            await s.go(page);
          },
        });
        expect(errors).toEqual([]);
      });

test("Security follows a language change with no reload", async ({ page }) => {
  const errors = trackPageErrors(page);
  await page.goto(fixtureUrl("funded", "light", "&lang=en"));
  await toSecurity(page);
  await expect(page.getByTestId("settings-wipe")).toHaveText("Wipe wallet…");
  await page.getByTestId("settings-tabs-general").click();
  await page.getByTestId("settings-language").selectOption("en-XA");
  await page.getByTestId("settings-tabs-security").click();
  await expect(page.getByTestId("settings-wipe")).toHaveText(/^⟦.+⟧$/);
  await expect(page.getByTestId("settings-auto-lock").locator("option").first()).toHaveText(/⟦.+⟧/);
  expect(errors).toEqual([]);
});

test.describe("the wipe's typed sentence", () => {
  test("is the app language's sentence, shown isolated, typed into a field that follows its direction", async ({ page }) => {
    await page.goto(fixtureUrl("funded", "light", "&lang=ar-XB"));
    await toSecurity(page);
    await toWipe(page);
    await expect(page.getByTestId("settings-wipe-sentence")).toHaveAttribute("dir", "auto");
    const shown = await phrase(page);
    expect(shown).not.toBe(SecurityText.wipeAcceptPhrase);
    await page.getByTestId("settings-wipe-sentence").fill(shown);
    await page.getByTestId("settings-wipe-continue").click();
    await expect(page.getByTestId("settings-wipe-passphrase-field")).toBeVisible();
  });

  test("the English sentence works in every language", async ({ page }) => {
    await page.goto(fixtureUrl("funded", "light", "&lang=ar-XB"));
    await toSecurity(page);
    await toWipe(page);
    await page.getByTestId("settings-wipe-sentence").fill(SecurityText.wipeAcceptPhrase);
    await page.getByTestId("settings-wipe-continue").click();
    await expect(page.getByTestId("settings-wipe-passphrase-field")).toBeVisible();
  });
});

/** The left edges of `re`'s two groups in the first text node under `root` that matches it. */
async function lefts(root: Locator, re: RegExp): Promise<[number, number] | null> {
  return root.evaluate(
    (el, [source, flags]) => {
      const re = new RegExp(source, flags);
      const walker = document.createTreeWalker(el, NodeFilter.SHOW_TEXT);
      for (let n = walker.nextNode(); n; n = walker.nextNode()) {
        const m = re.exec(n.textContent ?? "");
        if (!m?.indices) continue;
        const left = (g: number) => {
          const r = document.createRange();
          r.setStart(n!, m.indices![g]![0]);
          r.setEnd(n!, m.indices![g]![0] + 1);
          return r.getBoundingClientRect().left;
        };
        return [left(1), left(2)] as [number, number];
      }
      return null;
    },
    [re.source, `${re.flags}d`] as const,
  );
}

test("a spending limit's amount comes before its unit under ar-XB", async ({ page }) => {
  await page.goto(fixtureUrl("funded", "light", "&lang=ar-XB"));
  await toSecurity(page);
  await page.getByTestId("settings-spend-limit").selectOption("100000000");
  const pending = page.getByTestId("settings-spend-limit-pending");
  await expect(pending).toContainText(/\p{Nd}/u);
  const at = await lefts(pending, /(\p{Nd})[\p{Nd}.,٫٬]*[\u2066-\u2069\u202a-\u202e]* (DASH)/u);
  expect(at, "the limit").not.toBeNull();
  expect(at![0], "the limit").toBeLessThan(at![1]);
});
