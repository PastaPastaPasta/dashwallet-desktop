import { expect, test, type Locator, type Page } from "@playwright/test";
import { authorize, fixtureUrl, THEMES, trackPageErrors } from "./helpers";
import { checkArea, PSEUDO, untranslated } from "./pseudo";

// Localization (MP-14): DashPay's credits under the two pseudo-locales (./pseudo.ts): the top-up
// sheet (amount, confirmation, progress, outcome, refusals), the withdraw sheet (form, its
// problems, confirmation, progress, outcome, refusals), and My Profile's withdrawal rows and
// buttons. States are reached as profile.spec.ts and withdraw.spec.ts reach them.

/** Amounts, addresses, the fixture's withdrawal document, the engine's codes and a row's date. */
const DATA = [
  /^[+-]?[\p{Nd}.,٫٬\s]+( (t?DASH|mDASH|μDASH|duffs))?$/u,
  /^[Xy][1-9A-HJ-NP-Za-km-z]{25,34}$/,
  /^tdash1[0-9a-z]+$/,
  /^Wd0001x+$/,
  /^[a-z]+\.[a-z_]+$/,
  /^· .*\p{Nd}{4}$/u,
];

const PLATFORM = "tdash1kzc0lmmvxmfkd8ztchz4m6j8lgdjn4pxc0quhs7";

const command = (page: Page, c: object) => page.evaluate((c) => window.__dwFixture!.shellCommand(c as never), c);

async function toProfile(page: Page) {
  await page.getByTestId("toolbar").waitFor();
  await command(page, { kind: "navigate", route: "profile" });
  await page.getByTestId("page-profile").waitFor();
}

const topUp = (page: Page) => page.getByTestId("topup-body");
const withdraw = (page: Page) => page.getByTestId("withdraw-body");

async function topUpOpen(page: Page) {
  await toProfile(page);
  await page.getByTestId("profile-top-up").click();
  await page.getByTestId("topup-preset-5000000").waitFor();
}

async function topUpQuote(page: Page) {
  await topUpOpen(page);
  await page.getByTestId("topup-preset-10000000").click();
  await page.getByTestId("topup-next").click();
  await expect(page.getByTestId("topup-quote")).toContainText(/\p{Nd}/u);
  await expect(page.getByTestId("topup-pay")).toBeEnabled();
}

async function topUpPaid(page: Page) {
  await topUpQuote(page);
  await page.getByTestId("topup-pay").click();
  await authorize(page);
}

async function withdrawOpen(page: Page) {
  await toProfile(page);
  await page.getByTestId("profile-withdraw").click();
  await expect(withdraw(page)).toHaveAttribute("data-step", "form");
  await expect(page.getByTestId("withdraw-to")).not.toHaveValue("");
}

async function withdrawQuote(page: Page) {
  await withdrawOpen(page);
  await page.getByTestId("withdraw-next").click();
  await expect(page.getByTestId("withdraw-quote")).toContainText(/\p{Nd}/u);
}

async function withdrawSent(page: Page) {
  await withdrawQuote(page);
  await page.getByTestId("withdraw-go").click();
  await authorize(page);
}

/** Holds the fixture's `cmd` until the returned function is called. */
async function hold(page: Page, cmd: "credits_top_up" | "credits_withdraw") {
  await page.getByTestId("toolbar").waitFor();
  return page.evaluateHandle((c) => window.__dwFixture!.hold(c), cmd);
}

interface State {
  scenario: string;
  extra?: string;
  roots: string[];
  go: (page: Page) => Promise<void>;
}

const TOPUP = ["topup-sheet"];
const WITHDRAW = ["withdraw-sheet"];

export const STATES: Record<string, State> = {
  "topup-amount": { scenario: "dashpay-registered", roots: TOPUP, go: topUpOpen },
  "topup-other-empty": {
    scenario: "dashpay-registered",
    roots: TOPUP,
    go: async (page) => {
      await topUpOpen(page);
      await page.getByTestId("topup-other").click();
      await expect(page.getByTestId("topup-next")).toBeDisabled();
    },
  },
  "topup-other-below": {
    scenario: "dashpay-registered",
    roots: TOPUP,
    go: async (page) => {
      await topUpOpen(page);
      await page.getByTestId("topup-other").click();
      await page.getByTestId("topup-other-field").fill("0.001");
      await expect(page.getByTestId("topup-amount-problem")).toContainText(/\p{Nd}/u);
    },
  },
  "topup-confirm": { scenario: "dashpay-registered", roots: TOPUP, go: topUpQuote },
  "topup-quote-refused": {
    scenario: "dashpay-registered",
    extra: "&fail=credits_top_up_quote:none",
    roots: TOPUP,
    go: async (page) => {
      await topUpOpen(page);
      await page.getByTestId("topup-next").click();
      await page.getByTestId("topup-quote-error").waitFor();
    },
  },
  "topup-running": {
    scenario: "dashpay-registered",
    roots: TOPUP,
    go: async (page) => {
      await hold(page, "credits_top_up");
      await topUpPaid(page);
      await expect(topUp(page)).toHaveAttribute("data-step", "running");
    },
  },
  "topup-done": {
    scenario: "dashpay-registered",
    roots: TOPUP,
    go: async (page) => {
      await topUpPaid(page);
      await expect(page.getByTestId("topup-outcome-text")).toContainText(/\p{Nd}/u);
    },
  },
  "topup-refused-budget": {
    scenario: "dashpay-registered",
    extra: "&fail=credits_top_up:budget",
    roots: TOPUP,
    go: async (page) => {
      await topUpPaid(page);
      await page.getByTestId("topup-refused-back").waitFor();
    },
  },
  "topup-refused-lost": {
    scenario: "dashpay-top-up-lost",
    roots: TOPUP,
    go: async (page) => {
      await topUpPaid(page);
      await page.getByTestId("topup-repair").waitFor();
    },
  },
  "withdraw-form": { scenario: "dashpay-registered", roots: WITHDRAW, go: withdrawOpen },
  "withdraw-other-invalid": {
    scenario: "dashpay-registered",
    roots: WITHDRAW,
    go: async (page) => {
      await withdrawOpen(page);
      await page.getByTestId("withdraw-other").click();
      await page.getByTestId("withdraw-amount").fill("0.0000000001");
      await page.getByTestId("withdraw-amount-problem").waitFor();
    },
  },
  "withdraw-to-platform": {
    scenario: "dashpay-registered",
    roots: WITHDRAW,
    go: async (page) => {
      await withdrawOpen(page);
      await page.getByTestId("withdraw-to").fill(PLATFORM);
      await expect(page.getByTestId("withdraw-to-problem")).not.toBeEmpty();
    },
  },
  "withdraw-confirm": { scenario: "dashpay-registered", roots: WITHDRAW, go: withdrawQuote },
  "withdraw-quote-refused": {
    scenario: "dashpay-registered",
    extra: "&fail=credits_withdraw_quote:creditRange",
    roots: WITHDRAW,
    go: async (page) => {
      await withdrawOpen(page);
      await page.getByTestId("withdraw-next").click();
      await expect(page.getByTestId("withdraw-quote-error")).toContainText(/\p{Nd}/u);
    },
  },
  "withdraw-running": {
    scenario: "dashpay-registered",
    roots: WITHDRAW,
    go: async (page) => {
      await hold(page, "credits_withdraw");
      await withdrawSent(page);
      await expect(withdraw(page)).toHaveAttribute("data-step", "running");
    },
  },
  "withdraw-done": {
    scenario: "dashpay-registered",
    roots: WITHDRAW,
    go: async (page) => {
      await withdrawSent(page);
      await expect(withdraw(page)).toHaveAttribute("data-step", "done");
    },
  },
  "withdraw-landed": {
    scenario: "dashpay-withdraw-landed",
    roots: WITHDRAW,
    go: async (page) => {
      await withdrawSent(page);
      await expect(withdraw(page)).toHaveAttribute("data-step", "done");
    },
  },
  "withdraw-refused-credits": {
    scenario: "dashpay-registered",
    extra: "&fail=credits_withdraw:credits",
    roots: WITHDRAW,
    go: async (page) => {
      await withdrawSent(page);
      await page.getByTestId("withdraw-refused-back").waitFor();
    },
  },
  "withdraw-refused-lost": {
    scenario: "dashpay-withdraw-lost",
    roots: WITHDRAW,
    go: async (page) => {
      await withdrawSent(page);
      await page.getByTestId("withdraw-document").waitFor();
    },
  },
  "withdraw-after-lost": {
    scenario: "dashpay-withdraw-lost",
    roots: WITHDRAW,
    go: async (page) => {
      await STATES["withdraw-refused-lost"].go(page);
      await page.getByTestId("withdraw-close").click();
      await page.getByTestId("profile-withdraw").click();
      await page.getByTestId("withdraw-unconfirmed").waitFor();
    },
  },
  "profile-unconfirmed": {
    scenario: "dashpay-withdraw-lost",
    roots: ["profile-withdraw-unconfirmed"],
    go: async (page) => {
      await STATES["withdraw-refused-lost"].go(page);
      await page.getByTestId("withdraw-close").click();
      await page.getByTestId("profile-withdraw-unconfirmed").waitFor();
    },
  },
  "profile-unseen": {
    scenario: "dashpay-registered",
    roots: ["profile-withdraw-unseen"],
    go: async (page) => {
      const release = await hold(page, "credits_withdraw");
      await withdrawSent(page);
      await expect(withdraw(page)).toHaveAttribute("data-step", "running");
      await page.getByTestId("withdraw-sheet-header").locator(".sheet-close").click();
      await release.evaluate((r) => r());
      await page.getByTestId("profile-withdraw-unseen").waitFor();
    },
  },
};

for (const theme of THEMES)
  for (const pseudo of PSEUDO)
    for (const [name, state] of Object.entries(STATES))
      test(`DashPay credits ${name} shows only catalog text under ${pseudo[0]}, fits, and passes axe, ${theme}`, async ({ page }, info) => {
        test.setTimeout(120_000);
        const errors = trackPageErrors(page);
        await checkArea(page, info, theme, pseudo, {
          name: `dashpay-${name}`,
          roots: state.roots,
          data: DATA,
          open: async (lang) => {
            await page.goto(fixtureUrl(state.scenario, theme, `${state.extra ?? ""}&lang=${lang}`));
            await state.go(page);
          },
        });
        expect(errors).toEqual([]);
      });

// My Profile's withdrawal rows, one in each state. A row has no root of its own, so each is tagged
// here for the check.
for (const [lang, kind] of PSEUDO)
  test(`DashPay credits withdrawal rows show only catalog text under ${lang}`, async ({ page }) => {
    const errors = trackPageErrors(page);
    await page.goto(fixtureUrl("dashpay-withdrawals", "light", `&lang=${lang}`));
    await toProfile(page);
    const rows = page.getByTestId("profile-withdrawal");
    await expect(rows).toHaveCount(4);
    const roots = await rows.evaluateAll((els) => els.map((el, i) => (el.setAttribute("data-testid", `zz-withdrawal-${i}`), `zz-withdrawal-${i}`)));
    expect(await untranslated(page, roots, kind, DATA)).toEqual([]);
    expect(errors).toEqual([]);
  });

test("the top-up and withdraw sheets follow a language change with no reload", async ({ page }) => {
  const errors = trackPageErrors(page);
  await page.goto(fixtureUrl("dashpay-registered", "light", "&lang=en"));
  await topUpOpen(page);
  await expect(page.getByTestId("topup-other")).toHaveText("Other amount");
  await page.getByTestId("topup-cancel").click();
  await command(page, { kind: "navigate", route: "settings" });
  await page.getByTestId("settings-language").selectOption("en-XA");
  await topUpOpen(page);
  await expect(page.getByTestId("topup-other")).toHaveText(/^⟦.+⟧$/);
  await page.getByTestId("topup-cancel").click();
  await page.getByTestId("profile-withdraw").click();
  await expect(page.getByTestId("withdraw-next")).toHaveText(/^⟦.+⟧$/);
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

/** An amount's first digit and its unit: the digit is drawn first. */
const AMOUNT_UNIT = /(\p{Nd})[\p{Nd}.,٫٬]* (t?DASH)/u;

async function inOrder(root: Locator, what: string, re: RegExp = AMOUNT_UNIT) {
  await expect(root).toContainText(/\p{Nd}/u);
  const at = await lefts(root, re);
  expect(at, what).not.toBeNull();
  expect(at![0], what).toBeLessThan(at![1]);
}

test.describe("amounts keep their order under ar-XB", () => {
  const open = (page: Page, scenario = "dashpay-registered", extra = "") => page.goto(fixtureUrl(scenario, "light", `${extra}&lang=ar-XB`));

  test("a top-up preset's amount comes before its unit", async ({ page }) => {
    await open(page);
    await topUpOpen(page);
    await inOrder(page.getByTestId("topup-preset-5000000").locator(".topup-choice-amount"), "the preset");
  });

  for (const [n, row] of ["the amount", "the fee", "the total"].entries())
    test(`the top-up confirmation's ${row.slice(4)}: the amount comes before its unit`, async ({ page }) => {
      await open(page);
      await topUpQuote(page);
      await inOrder(page.getByTestId("topup-quote").locator(".detail-value").nth(n), row);
    });

  for (const [n, row] of ["the credits", "the fee", "the total", "the Dash received"].entries())
    test(`the withdraw confirmation's ${row.slice(4)}: the amount comes before its unit`, async ({ page }) => {
      await open(page);
      await withdrawQuote(page);
      await inOrder(page.getByTestId("withdraw-quote").locator(".detail-value").nth(n), row);
    });

  test("a credit balance before the cost table is read: a catalog sentence, its number in order", async ({ page }) => {
    await open(page, "dashpay-registered", "&fail=credits_cost_table:none");
    await toProfile(page);
    const figure = page.getByTestId("profile-credits-figure");
    await expect(figure).toContainText(/\p{Nd}/u);
    // A catalog sentence: ar-XB marks it, so the number is a placeholder isolated from it.
    await expect(figure).toContainText("‮");
    await inOrder(figure, "the credits", /(\p{Nd})[\p{Nd},.٬٫\s]*(\p{Nd})/u);
  });
});
