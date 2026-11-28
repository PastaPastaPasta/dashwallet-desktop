import { expect, test, type Locator, type Page } from "@playwright/test";
import { authorize, fixtureUrl, openTransfer, THEMES, trackPageErrors, type Theme, type TransferRoute } from "./helpers";
import { checkArea, PSEUDO } from "./pseudo";
import { hold } from "./transfer";

// Localization (MP-14): Transfer under the two pseudo-locales (./pseudo.ts): each route's form, the
// picker, the unavailable and loading lines, each route's confirmation, progress, outcome and
// refusal, the Running line, and Pay's Send from Shielded with its unrecorded-spend question.
// States are reached as transfer.ts, shielded-max-sync.spec.ts and integration.spec.ts reach them.

const SHIELDED_ADDRESS = "tdash1zq0k8yv6m2e3x9n4c5w7a8s0d2f4g6h8j0k2l4m6n8p0q2r4s6t8u0v2w4x6y8z0a2c4e6g8j0m2";

/** Formatted Dash amounts shown as plain strings ("0.5 tDASH", "+0.001 tDASH"), and addresses. */
const DATA = [/^\+?[\p{Nd}.,٫٬\s•*#]+ (t?DASH|mDASH|μDASH|duffs)$/u, /^[Xy][1-9A-HJ-NP-Za-km-z]{25,34}$/, /^tdash1[0-9a-z]+$/];

/** Still English, from TransferSheet.tsx's route header: the hidden " to " between the two pools
 * (needs a whole-sentence key). */
const ROUTE_TO = [/^to$/];

async function openHome(page: Page, scenario: string, theme: Theme, lang: string, extra = "") {
  await page.goto(fixtureUrl(scenario, theme, `${extra}&lang=${lang}`));
  await page.getByTestId("toolbar").waitFor();
}

/** The route's form with `amount` typed and its fee line read. */
async function typed(page: Page, route: TransferRoute, amount: string) {
  await openTransfer(page, route);
  await page.getByTestId("transfer-amount").fill(amount);
  await expect(page.getByTestId("transfer-continue")).toBeEnabled();
  await expect(page.getByTestId("transfer-fee")).toContainText(/\p{Nd}/u);
}

const AMOUNT = { coreToShielded: "0.001", shieldedToCore: "0.5", shieldedToPlatform: "0.5", coreToPlatform: "0.5", platformToCore: "" } as const;
const GO = { coreToShielded: "shielded-go", shieldedToCore: "shielded-go", shieldedToPlatform: "shielded-go", coreToPlatform: "platform-fund-go", platformToCore: "platform-move-go" } as const;
const BODY = { coreToShielded: "shielded-body", shieldedToCore: "shielded-body", shieldedToPlatform: "shielded-body", coreToPlatform: "platform-fund-body", platformToCore: "platform-move-body" } as const;

/** Through Continue to the route's own confirmation, its quote read. */
async function confirm(page: Page, route: TransferRoute) {
  if (route === "platformToCore") await openTransfer(page, route);
  else {
    await openTransfer(page, route);
    await page.getByTestId("transfer-amount").fill(AMOUNT[route]);
  }
  await expect(page.getByTestId("transfer-continue")).toBeEnabled();
  await page.getByTestId("transfer-continue").click();
  await expect(page.getByTestId(BODY[route])).toHaveAttribute("data-step", "confirm");
  await expect(page.getByTestId(GO[route])).toBeEnabled();
}

/** A Dash wallet Max through Continue: its confirmation with what it moves and leaves behind. */
async function maxConfirm(page: Page, route: "coreToShielded" | "coreToPlatform") {
  await openTransfer(page, route);
  await page.getByTestId("transfer-max").click();
  await expect(page.getByTestId("transfer-continue")).toBeEnabled();
  await page.getByTestId("transfer-continue").click();
  await expect(page.getByTestId(BODY[route])).toHaveAttribute("data-step", "confirm");
  await expect(page.getByTestId("core-max-note")).toHaveCount(2);
  await expect(page.getByTestId(GO[route])).toBeEnabled();
}

/** A Dash wallet Max below the smallest amount: its refusal in the fee line, with a figure unless
 * Max has nothing to move and names none (`figure`). */
async function maxBelow(page: Page, route: "coreToShielded" | "coreToPlatform", figure = true) {
  await openTransfer(page, route);
  await page.getByTestId("transfer-max").click();
  await expect(page.getByTestId("transfer-fee")).toHaveAttribute("role", "status");
  await expect(page.getByTestId("transfer-fee")).toHaveClass(/form-problem/);
  if (figure) await expect(page.getByTestId("transfer-fee")).toContainText(/\p{Nd}/u);
  else await expect(page.getByTestId("transfer-fee")).not.toBeEmpty();
  await expect(page.getByTestId("transfer-amount")).toHaveValue("");
}

/** Confirm, the passphrase, and the route's result. */
async function run(page: Page, route: TransferRoute, toast = false) {
  await confirm(page, route);
  await page.getByTestId(GO[route]).click();
  await authorize(page);
  if (toast) {
    await expect(page.getByTestId("toast")).toBeVisible();
    await page.mouse.move(0, 0);
  } else await expect(page.getByTestId(BODY[route])).toHaveAttribute("data-step", /^(done|refused)$/);
}

interface State {
  scenario: string;
  extra?: string;
  roots?: string[];
  routeHeader?: boolean;
  data?: RegExp[];
  go: (page: Page) => Promise<void>;
}

const STATES: Record<string, State> = {
  "form-shielded": {
    scenario: "funded",
    go: async (page) => {
      await page.getByTestId("shortcut-transfer").click();
      await expect(page.getByTestId("transfer-body")).toHaveAttribute("data-route", "coreToShielded");
    },
  },
  picker: {
    scenario: "funded",
    go: async (page) => {
      await page.getByTestId("shortcut-transfer").click();
      await page.getByTestId("transfer-to").click();
      await expect(page.getByTestId("transfer-picker")).toBeVisible();
    },
  },
  "form-platform-insufficient": {
    scenario: "funded",
    go: async (page) => {
      await openTransfer(page, "coreToPlatform");
      await page.getByTestId("transfer-amount").fill("1000");
      await expect(page.getByTestId("transfer-fee")).toHaveClass("form-problem");
    },
  },
  "form-platform-mixing": {
    scenario: "coinjoin-mixing",
    go: async (page) => {
      await openTransfer(page, "coreToPlatform");
      await expect(page.getByTestId("transfer-mixing")).toBeVisible();
      await expect(page.getByTestId("transfer-pause-mixing")).toBeVisible();
    },
  },
  "form-shieldedToCore": { scenario: "funded", go: (page) => typed(page, "shieldedToCore", "0.5") },
  "form-platformToCore": {
    scenario: "funded",
    go: async (page) => {
      await openTransfer(page, "platformToCore");
      await expect(page.getByTestId("transfer-continue")).toBeEnabled();
    },
  },
  "form-platform-empty": {
    scenario: "platform-empty",
    go: async (page) => {
      await openTransfer(page, "platformToCore");
      await expect(page.getByTestId("transfer-unavailable")).toBeVisible();
    },
  },
  unsupported: {
    scenario: "funded",
    go: async (page) => {
      await openTransfer(page, "platformToCore");
      await page.getByTestId("transfer-to").click();
      await page.getByTestId("transfer-picker-shielded").click();
      await expect(page.getByTestId("transfer-unavailable")).toBeVisible();
    },
  },
  "unsupported-mainnet": {
    scenario: "transfer-mainnet",
    go: async (page) => {
      await openTransfer(page, "shieldedToCore");
      await page.getByTestId("transfer-to").click();
      await page.getByTestId("transfer-picker-platform").click();
      await expect(page.getByTestId("transfer-unavailable")).toBeVisible();
    },
  },
  "form-shieldedToPlatform": { scenario: "funded", go: (page) => typed(page, "shieldedToPlatform", "0.5") },
  "none-absent": {
    scenario: "pools-absent",
    go: async (page) => {
      await page.getByTestId("shortcut-transfer").click();
      await expect(page.getByTestId("transfer-unavailable")).toBeVisible();
    },
  },
  "none-inactive": {
    scenario: "transfer-none",
    go: async (page) => {
      await page.getByTestId("shortcut-transfer").click();
      await expect(page.getByTestId("transfer-unavailable")).toBeVisible();
    },
  },
  loading: {
    scenario: "funded",
    extra: "&hold=shielded_status&hold=platform_addresses",
    go: async (page) => {
      await page.getByTestId("shortcut-transfer").click();
      await expect(page.getByTestId("transfer-loading")).toBeVisible();
    },
  },
  "shielded-confirm": { scenario: "funded", routeHeader: true, go: (page) => confirm(page, "coreToShielded") },
  "shielded-max-confirm": { scenario: "coinjoin-move", routeHeader: true, go: (page) => maxConfirm(page, "coreToShielded") },
  "shielded-max-below": { scenario: "transfer-max-dust", routeHeader: true, go: (page) => maxBelow(page, "coreToShielded") },
  "shielded-max-below-left": { scenario: "transfer-max-dust-left", routeHeader: true, go: (page) => maxBelow(page, "coreToShielded") },
  "shielded-max-all-arriving": { scenario: "transfer-max-arriving", routeHeader: true, go: (page) => maxBelow(page, "coreToShielded") },
  "shielded-max-arriving-too-small": { scenario: "transfer-max-arriving-small", routeHeader: true, go: (page) => maxBelow(page, "coreToShielded") },
  "shielded-max-incoming": { scenario: "transfer-max-incoming", routeHeader: true, go: (page) => maxBelow(page, "coreToShielded") },
  "shielded-max-mixed-only": { scenario: "transfer-max-mixed-only", routeHeader: true, go: (page) => maxBelow(page, "coreToShielded", false) },
  "shielded-max-nothing": { scenario: "transfer-max-nothing", routeHeader: true, go: (page) => maxBelow(page, "coreToShielded", false) },
  "shielded-max-empty": { scenario: "transfer-max-tiny", routeHeader: true, go: (page) => maxBelow(page, "coreToShielded", false) },
  "shielded-max-none": { scenario: "transfer-max-none", routeHeader: true, go: async (page) => { await openTransfer(page, "coreToShielded"); await expect(page.getByTestId("transfer-max")).toBeDisabled(); } },
  "shielded-done": { scenario: "funded", routeHeader: true, go: (page) => run(page, "coreToShielded") },
  "shielded-refused": { scenario: "transfer-shielded-refused", routeHeader: true, go: (page) => run(page, "coreToShielded") },
  "shielded-waiting": {
    scenario: "transfer-shield-waiting",
    roots: ["toast"],
    go: (page) => run(page, "coreToShielded", true),
  },
  "out-confirm": { scenario: "funded", routeHeader: true, go: (page) => confirm(page, "shieldedToCore") },
  "out-done": { scenario: "funded", routeHeader: true, go: (page) => run(page, "shieldedToCore") },
  "toPlatform-confirm": { scenario: "funded", routeHeader: true, go: (page) => confirm(page, "shieldedToPlatform") },
  "toPlatform-done": { scenario: "funded", routeHeader: true, go: (page) => run(page, "shieldedToPlatform") },
  "toPlatform-may-be-out": { scenario: "funded", extra: "&fail=unshield_to_platform:artifact", routeHeader: true, go: (page) => run(page, "shieldedToPlatform") },
  "fund-confirm": { scenario: "funded", routeHeader: true, go: (page) => confirm(page, "coreToPlatform") },
  "fund-max-confirm": { scenario: "coinjoin-move", routeHeader: true, go: (page) => maxConfirm(page, "coreToPlatform") },
  "fund-progress": {
    scenario: "funded",
    routeHeader: true,
    go: async (page) => {
      await hold(page, "platform_fund");
      await confirm(page, "coreToPlatform");
      await page.getByTestId("platform-fund-go").click();
      await authorize(page);
      await expect(page.getByTestId("platform-fund-progress")).toBeVisible();
      await expect(page.getByTestId("transfer-preparing")).toBeVisible();
    },
  },
  "fund-max-below": { scenario: "transfer-max-dust", routeHeader: true, go: (page) => maxBelow(page, "coreToPlatform") },
  "fund-max-below-left": { scenario: "transfer-max-dust-left", routeHeader: true, go: (page) => maxBelow(page, "coreToPlatform") },
  "fund-max-all-arriving": { scenario: "transfer-max-arriving", routeHeader: true, go: (page) => maxBelow(page, "coreToPlatform") },
  "fund-max-arriving-too-small": { scenario: "transfer-max-arriving-small", routeHeader: true, go: (page) => maxBelow(page, "coreToPlatform") },
  "fund-max-incoming": { scenario: "transfer-max-incoming", routeHeader: true, go: (page) => maxBelow(page, "coreToPlatform") },
  "fund-max-mixed-only": { scenario: "transfer-max-mixed-only", routeHeader: true, go: (page) => maxBelow(page, "coreToPlatform", false) },
  "fund-max-nothing": { scenario: "transfer-max-nothing", routeHeader: true, go: (page) => maxBelow(page, "coreToPlatform", false) },
  "fund-max-empty": { scenario: "transfer-max-tiny", routeHeader: true, go: (page) => maxBelow(page, "coreToPlatform", false) },
  "fund-max-none": { scenario: "transfer-max-none", routeHeader: true, go: async (page) => { await openTransfer(page, "coreToPlatform"); await expect(page.getByTestId("transfer-max")).toBeDisabled(); } },
  "fund-done": { scenario: "funded", routeHeader: true, go: (page) => run(page, "coreToPlatform") },
  "fund-not-applied": { scenario: "transfer-fund-not-applied", routeHeader: true, go: (page) => run(page, "coreToPlatform") },
  "fund-earlier-lock": {
    scenario: "paddr-fund-proof-waiting",
    routeHeader: true,
    go: async (page) => {
      await openTransfer(page, "coreToPlatform");
      await page.getByTestId("transfer-amount").fill("0.5");
      await page.getByTestId("transfer-continue").click();
      await expect(page.getByTestId("platform-fund-quote-error")).toBeVisible();
    },
  },
  "move-confirm": { scenario: "funded", routeHeader: true, go: (page) => confirm(page, "platformToCore") },
  "move-done": { scenario: "funded", routeHeader: true, go: (page) => run(page, "platformToCore") },
  "move-refused": { scenario: "transfer-move-refused", routeHeader: true, go: (page) => run(page, "platformToCore") },
  "move-pending": { scenario: "transfer-move-pending", roots: ["toast"], go: (page) => run(page, "platformToCore", true) },
  "running-line": {
    scenario: "funded",
    go: async (page) => {
      await hold(page, "shield_from_core");
      await confirm(page, "coreToShielded");
      await page.getByTestId("shielded-go").click();
      await authorize(page);
      await expect(page.getByTestId("shielded-progress")).toBeVisible();
      await page.keyboard.press("Escape");
      await expect(page.getByTestId("transfer-sheet")).toHaveCount(0);
      await openTransfer(page, "platformToCore");
      await expect(page.getByTestId("transfer-running")).toBeVisible();
    },
  },
  "send-form": {
    scenario: "funded",
    roots: ["shielded-sheet"],
    go: async (page) => {
      await page.getByTestId("sidebar-pay").click();
      await page.getByTestId("shielded-send").click();
      await expect(page.getByTestId("shielded-body")).toHaveAttribute("data-step", "form");
    },
  },
  "send-unrecorded": {
    scenario: "shielded-unrecorded",
    roots: ["shielded-unrecorded-dialog"],
    go: async (page) => {
      await page.getByTestId("sidebar-pay").click();
      await page.getByTestId("shielded-send").click();
      await page.getByTestId("shielded-amount").fill("0.5");
      await page.getByTestId("shielded-to").fill(SHIELDED_ADDRESS);
      await page.getByTestId("shielded-next").click();
      await expect(page.getByTestId("shielded-body")).toHaveAttribute("data-step", "confirm");
      await expect(page.getByTestId("shielded-go")).toBeEnabled();
      await page.getByTestId("shielded-go").click();
      await expect(page.getByTestId("shielded-unrecorded-dialog")).toBeVisible();
    },
  },
};

for (const theme of THEMES)
  for (const pseudo of PSEUDO)
    for (const [name, state] of Object.entries(STATES))
      test(`Transfer ${name} shows only catalog text under ${pseudo[0]}, fits, and passes axe, ${theme}`, async ({ page }, info) => {
        test.setTimeout(120_000);
        const errors = trackPageErrors(page);
        const roots = state.roots ?? ["transfer-sheet"];
        await checkArea(page, info, theme, pseudo, {
          name: `transfer-${name}`,
          roots,
          panes: roots.map((id) => `[data-testid="${id}"]`),
          data: [...DATA, ...(state.routeHeader ? ROUTE_TO : []), ...(state.data ?? [])],
          open: async (lang) => {
            await openHome(page, state.scenario, theme, lang, state.extra);
            await state.go(page);
          },
        });
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
  const open = (page: Page, scenario = "funded") => openHome(page, scenario, "light", "ar-XB");

  test("the From end's balance comes before its unit", async ({ page }) => {
    await open(page);
    await openTransfer(page, "coreToPlatform");
    await inOrder(page.getByTestId("transfer-from").locator(".transfer-end-balance"), "the From balance");
  });

  test("the To end's +amount: the sign, then the amount, then its unit", async ({ page }) => {
    await open(page);
    await openTransfer(page, "coreToPlatform");
    await page.getByTestId("transfer-amount").fill("0.5");
    const to = page.getByTestId("transfer-to").locator(".transfer-end-balance");
    await expect(to).toContainText(/\+.*\p{Nd}/u);
    await inOrder(to, "the To amount");
    await inOrder(to, "the sign before the amount", /(\+)(\p{Nd})/u);
  });

  test("the picker's balances come before their unit", async ({ page }) => {
    await open(page);
    await page.getByTestId("shortcut-transfer").click();
    await page.getByTestId("transfer-to").click();
    await inOrder(page.getByTestId("transfer-picker-platform").locator(".transfer-end-balance"), "a picker balance");
  });

  test("the mixing line's amount comes before its unit", async ({ page }) => {
    await open(page, "coinjoin-mixing");
    await openTransfer(page, "coreToPlatform");
    await inOrder(page.getByTestId("transfer-mixing"), "the mixing amount");
  });

  test("Not enough spendable Dash: the amount comes before its unit", async ({ page }) => {
    await open(page);
    await openTransfer(page, "coreToPlatform");
    await page.getByTestId("transfer-amount").fill("1000");
    await expect(page.getByTestId("transfer-fee")).toHaveClass("form-problem");
    // A catalog sentence: ar-XB marks it, so the amount is a placeholder isolated from it.
    await expect(page.getByTestId("transfer-fee")).toContainText("\u202e");
    await inOrder(page.getByTestId("transfer-fee"), "the available amount");
  });

  for (const row of ["credits", "fee", "duffs"])
    test(`Platform → Dash wallet's confirm row ${row}`, async ({ page }) => {
      await open(page);
      await confirm(page, "platformToCore");
      await inOrder(page.getByTestId(`platform-move-quote-${row}`), row);
    });

  for (const row of ["amount", "fee", "total", "platform-fee", "receive"])
    test(`Dash wallet → Platform's confirm row ${row}`, async ({ page }) => {
      await open(page);
      await confirm(page, "coreToPlatform");
      await inOrder(page.getByTestId(`platform-fund-confirm-${row}`), row);
    });

  for (const row of ["amount", "fee", "total", "pool-fee", "gain"])
    test(`Dash wallet → Shielded's quote row ${row}`, async ({ page }) => {
      await open(page);
      await confirm(page, "coreToShielded");
      await inOrder(page.getByTestId(`shielded-quote-${row}`), row);
    });

  for (const row of ["amount", "fee", "total", "receive"])
    test(`Shielded → Dash wallet's quote row ${row}`, async ({ page }) => {
      await open(page);
      await confirm(page, "shieldedToCore");
      await inOrder(page.getByTestId(`shielded-quote-${row}`), row);
    });

  async function platformSend(page: Page, max: boolean) {
    await open(page);
    await page.getByTestId("sidebar-pay").click();
    await page.getByTestId("pay-address").fill("tdash1kq7r3m9x2v5c8n4z6w0e2t4y6u8p0s2a4d6f8h");
    await page.getByTestId("platform-send-intro").waitFor();
    if (max) await page.getByTestId("pay-max").click();
    else await page.getByTestId("pay-amount").fill("0.01");
    await page.getByTestId("pay-fee-line").waitFor();
    await page.getByTestId("pay-continue").click();
    await page.getByTestId("platform-send-confirm").waitFor();
  }

  test("Pay from Platform's amount row", async ({ page }) => {
    await platformSend(page, false);
    await inOrder(page.getByTestId("platform-send-confirm-amount"), "the amount");
  });

  test("Pay from Platform's fee row", async ({ page }) => {
    await platformSend(page, false);
    await inOrder(page.getByTestId("platform-send-confirm-fee"), "the fee");
  });

  test("Pay from Platform's total row", async ({ page }) => {
    await platformSend(page, false);
    await inOrder(page.getByTestId("platform-send-confirm-total"), "the total");
  });

  test("Pay from Platform's Max: everything, recipient gets and total", async ({ page }) => {
    await platformSend(page, true);
    await inOrder(page.getByTestId("platform-send-confirm-amount"), "everything");
    await inOrder(page.getByTestId("platform-send-confirm-recipient"), "the recipient gets");
    await inOrder(page.getByTestId("platform-send-confirm-total"), "the total");
  });
});
