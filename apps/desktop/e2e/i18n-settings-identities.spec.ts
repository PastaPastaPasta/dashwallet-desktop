import { expect, test, type Locator, type Page } from "@playwright/test";
import { IDENTITY } from "../src/fixture/dashpay";
import { WORK_IDENTITY } from "../src/fixture/identities";
import { FIXTURE_WALLET_ID, SECOND_WALLET_ID } from "../src/fixture/scenarios";
import { fixtureUrl, THEMES, trackPageErrors } from "./helpers";
import { toAdvanced } from "./identities";
import { checkArea, PSEUDO } from "./pseudo";

// Localization (MP-14): Settings ▸ Advanced (Identities), Wallets, CoinJoin and About under the two
// pseudo-locales (./pseudo.ts), reached as identities.spec.ts, settings.spec.ts and coinjoin.spec.ts
// reach them; and the toolbar chip's username order.

/** Amounts, usernames, ids, keys, paths, versions and the fixture's wallet names. */
const DATA = [
  /^[+-]?[\p{Nd}.,٫٬\s]+( (t?DASH|mDASH|μDASH|duffs))?$/u,
  /^@?[a-z0-9-]+$/,
  /^(@[a-z0-9-]+(, |،)?)+$/,
  /^[1-9A-HJ-NP-Za-km-z]{6}…[1-9A-HJ-NP-Za-km-z]{6}$/,
  /^[1-9A-HJ-NP-Za-km-z]{30,}$/,
  /^[0-9a-f]{64}$/,
  /^[tx]pub[1-9A-HJ-NP-Za-km-z]+$/,
  /^m(\/\d+'?)+$/,
  /^\/\S+$/,
  /^\d+\.\d+\.\d+\S*$/,
  /^[\p{Nd}]+( · )?$/u,
  /^(·|—)$/,
  /^(testnet|mainnet|devnet|regtest)$/,
  /^\/[^/]+\/$/,
  /^(Main|Savings|Rainy day|Spending)$/,
];

const command = (page: Page, c: object) => page.evaluate((c) => window.__dwFixture!.shellCommand(c as never), c);

async function toTab(page: Page, tab: string) {
  await page.getByTestId("toolbar").waitFor();
  await command(page, { kind: "navigate", route: "settings" });
  await page.getByTestId(`settings-tabs-${tab}`).click();
}

const row = (page: Page, identity: string) => page.locator(`[data-testid=identity-row][data-identity="${identity}"]`);
const B = `settings-wallet-${SECOND_WALLET_ID.slice(0, 8)}`;
const A = `settings-wallet-${FIXTURE_WALLET_ID.slice(0, 8)}`;

async function lookUp(page: Page, text: string) {
  await page.getByTestId("identities-look-up").click();
  await page.getByTestId("identity-find-field").fill(text);
  await page.getByTestId("identity-find-submit").click();
}

const failRefresh = (page: Page, identity: string, code: string) =>
  page.evaluate(([id, c]) => void (window.__dwFixture!.world.dashpay.identityState!.refreshFailures[id] = c as never), [identity, code]);

interface State {
  scenario: string;
  extra?: string;
  roots: string[];
  scrollers?: string[];
  go: (page: Page) => Promise<void>;
}

/** The keys table scrolls sideways inside its card. */
const KEYS = [".identity-keys-wrap"];

const IDS = ["identities-page"];
const identities = (scenario: string, ready: (p: Page) => Promise<void>, extra?: string, roots = IDS): State => ({
  scenario,
  extra,
  roots,
  scrollers: KEYS,
  go: async (p) => {
    await p.getByTestId("toolbar").waitFor();
    await toAdvanced(p);
    await ready(p);
  },
});
const tab = (name: string, scenario: string, roots: string[], ready: (p: Page) => Promise<void>): State => ({
  scenario,
  roots,
  go: async (p) => {
    await toTab(p, name);
    await ready(p);
  },
});

export const STATES: Record<string, State> = {
  "I-failed": identities("dashpay-registered", (p) => expect(p.getByTestId("identities-failed")).toBeVisible(), "&fail=identities:none"),
  "I-none": identities("dashpay-no-identity", (p) => expect(p.getByTestId("identities-join")).toBeVisible()),
  "I-rows": identities("dashpay-identities", (p) => expect(p.getByTestId("identity-row")).toHaveCount(2)),
  "I-detail": identities(
    "dashpay-identities",
    async (p) => {
      await row(p, IDENTITY.alice).getByTestId("identity-keys").click();
      await expect(p.getByTestId("identity-key")).toHaveCount(7);
    },
    undefined,
    ["identity-detail"],
  ),
  "I-found": identities("dashpay-identities", async (p) => {
    await lookUp(p, "bob");
    await expect(p.getByTestId("identity-found-keys")).toBeVisible();
  }),
  "I-refresh-all-failed": identities("dashpay-identities", async (p) => {
    await failRefresh(p, WORK_IDENTITY, "platform.unavailable");
    await p.getByTestId("identities-refresh-all").click();
    await expect(p.getByTestId("identities-result")).toBeVisible();
  }),
  "W-list": tab("wallets", "two-wallets", ["settings-wallet", "settings-wallets"], async (p) => {
    await expect(p.getByTestId(`${B}-state`)).toBeVisible();
  }),
  "W-rename": tab("wallets", "two-wallets", ["settings-rename-sheet"], async (p) => {
    await p.getByTestId(`${B}-rename`).click();
    await p.getByTestId("settings-rename-field").fill("  ");
    await p.getByTestId("settings-rename-save").click();
    await expect(p.getByTestId("settings-rename-error")).toBeVisible();
  }),
  "W-remove": tab("wallets", "two-wallets", ["settings-remove-sheet"], async (p) => {
    await p.getByTestId(`${B}-remove`).click();
    await expect(p.getByTestId("settings-remove-passphrase-field")).toBeVisible();
  }),
  "W-xpub": tab("wallets", "funded", ["settings-xpub-sheet"], async (p) => {
    await p.getByTestId(`${A}-xpub`).click();
    await expect(p.getByTestId("settings-xpub-qr")).toBeVisible();
  }),
  "W-watch-only": tab("wallets", "funded", ["settings-watch-only-sheet"], async (p) => {
    await p.getByTestId("settings-add-watch-only").click();
    await p.getByTestId("settings-watch-only-add").click();
    await expect(p.getByTestId("settings-watch-only-error")).toBeVisible();
  }),
  About: tab("about", "funded", ["settings-about", "settings-about-license", "settings-about-more"], async (p) => {
    await p.getByTestId("settings-about-agent").getByText("…").waitFor({ state: "detached" });
  }),
  CoinJoin: tab("coinjoin", "funded", ["settings-coinjoin", "settings-coinjoin-mixing"], async (p) => {
    await expect(p.getByTestId("settings-coinjoin-save")).toBeVisible();
  }),
  "CoinJoin-restore": tab("coinjoin", "funded", ["settings-coinjoin-restore-sheet"], async (p) => {
    await p.getByTestId("settings-coinjoin-restore").click();
    await expect(p.getByTestId("settings-coinjoin-restore-confirm")).toBeVisible();
  }),
};

for (const theme of THEMES)
  for (const pseudo of PSEUDO)
    for (const [name, state] of Object.entries(STATES))
      test(`Settings ${name} shows only catalog text under ${pseudo[0]}, fits, and passes axe, ${theme}`, async ({ page }, info) => {
        test.setTimeout(120_000);
        const errors = trackPageErrors(page);
        await checkArea(page, info, theme, pseudo, {
          name: `settings-${name}`,
          roots: state.roots,
          scrollers: state.scrollers,
          data: DATA,
          open: async (lang) => {
            await page.goto(fixtureUrl(state.scenario, theme, `${state.extra ?? ""}&lang=${lang}`));
            await state.go(page);
          },
        });
        expect(errors).toEqual([]);
      });

test("Identities, Wallets, CoinJoin and About follow a language change with no reload", async ({ page }) => {
  const errors = trackPageErrors(page);
  await page.goto(fixtureUrl("dashpay-identities", "light", "&lang=en"));
  await page.getByTestId("toolbar").waitFor();
  await toAdvanced(page);
  await expect(page.getByTestId("identities-refresh-all")).toHaveText("Refresh all");
  await page.getByTestId("settings-tabs-general").click();
  await page.getByTestId("settings-language").selectOption("en-XA");
  await page.getByTestId("settings-tabs-advanced").click();
  await expect(page.getByTestId("identities-refresh-all")).toHaveText(/^⟦.+⟧$/);
  for (const [name, testId] of [
    ["wallets", "settings-add-watch-only"],
    ["coinjoin", "settings-coinjoin-save"],
    ["about", "settings-about-version"],
  ]) {
    await page.getByTestId(`settings-tabs-${name}`).click();
    await expect(page.getByTestId(testId)).toContainText(/⟦.+⟧/);
  }
  expect(errors).toEqual([]);
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

/** A username's "@" and its first letter: the "@" is drawn first. */
const AT_NAME = /(@)([a-z])/;

async function inOrder(root: Locator, what: string, re: RegExp = AT_NAME) {
  await expect(root).toContainText(re);
  const at = await lefts(root, re);
  expect(at, what).not.toBeNull();
  expect(at![0], what).toBeLessThan(at![1]);
}

test.describe("usernames and amounts keep their order under ar-XB", () => {
  const open = async (page: Page, scenario: string) => {
    await page.goto(fixtureUrl(scenario, "light", "&lang=ar-XB"));
    await page.getByTestId("toolbar").waitFor();
  };

  test("an identity's credit balance: the amount before its unit", async ({ page }) => {
    await open(page, "dashpay-identities");
    await toAdvanced(page);
    await inOrder(row(page, IDENTITY.alice).getByTestId("identity-balance"), "the balance", /(\p{Nd})[\p{Nd}.,٫٬]* (t?DASH)/u);
  });

  test("an identity's username in the list", async ({ page }) => {
    await open(page, "dashpay-identities");
    await toAdvanced(page);
    await inOrder(row(page, IDENTITY.alice).getByTestId("identity-username"), "the row");
  });

  test("an identity's usernames in its detail", async ({ page }) => {
    await open(page, "dashpay-identities");
    await toAdvanced(page);
    await row(page, IDENTITY.alice).getByTestId("identity-keys").click();
    await inOrder(page.getByTestId("identity-name").first(), "the detail");
  });

  test("a found identity's usernames and how it was found", async ({ page }) => {
    await open(page, "dashpay-identities");
    await toAdvanced(page);
    await lookUp(page, "bob");
    await inOrder(page.getByTestId("identity-found-names"), "the usernames");
    await inOrder(page.getByTestId("identity-found-how"), "found by");
  });

  test("the toolbar chip while a requested name is voted on", async ({ page }) => {
    await open(page, "dashpay-contest-pending");
    await inOrder(page.getByTestId("toolbar-chip"), "the contest chip");
  });

  test("the toolbar chip while a name registers", async ({ page }) => {
    await open(page, "dashpay-registering");
    await inOrder(page.getByTestId("toolbar-chip"), "the registering chip");
  });

  test("the build version reads left to right in About, Settings and Information", async ({ page }) => {
    const VERSION = /(0)\.1\.0-(d)ev/;
    await open(page, "funded");
    await page.evaluate(() => window.__dwFixture!.shellCommand({ kind: "about" }));
    await inOrder(page.getByTestId("about-version"), "the About sheet", VERSION);
    await page.keyboard.press("Escape");
    await page.evaluate(() => window.__dwFixture!.shellCommand({ kind: "navigate", route: "settings" }));
    await page.getByTestId("settings-tabs-about").click();
    await inOrder(page.getByTestId("settings-about-version"), "Settings ▸ About", VERSION);
    await page.evaluate(() => window.__dwFixture!.shellCommand({ kind: "tools", tab: "information" }));
    await inOrder(page.getByTestId("information-general"), "Tools ▸ Information", VERSION);
  });

  test("the toolbar chip's accessible name isolates the whole username", async ({ page }) => {
    await open(page, "dashpay-registered");
    await expect(page.getByTestId("toolbar-chip")).toHaveAttribute("aria-label", /⁨@alice⁩/);
  });
});
