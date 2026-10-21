import { expect, test, type Page } from "@playwright/test";
import { seriousAxeViolations } from "./axe";
import { fontsReady, THEMES } from "./helpers";
import { openIdentities, toAdvanced } from "./identities";
import { IDENTITY } from "../src/fixture/dashpay";
import { SPARE_IDENTITY, WORK_IDENTITY } from "../src/fixture/identities";
import { FIXTURE_PASSPHRASE } from "../src/fixture/scenarios";

// DP6-01: Settings ▸ Advanced ▸ Identities on the fixture backend, by parity ID: IOS-071 (the
// identities list with the main one marked, Set as main, each identity's keys, Refresh, Find my
// identities, and the look-up of any identity by id or username).

const row = (page: Page, identity: string) => page.locator(`[data-testid=identity-row][data-identity="${identity}"]`);
const mainRows = (page: Page) => page.locator("[data-testid=identity-row][data-main]");
const find = (page: Page) => page.getByTestId("identity-find");

async function lookUp(page: Page, text: string) {
  if (!(await find(page).isVisible())) await page.getByTestId("identities-look-up").click();
  await page.getByTestId("identity-find-field").fill(text);
  await page.getByTestId("identity-find-submit").click();
}

/** Each state the design inventories (L2–L5, the detail, the look-up), its scenario and what shows. */
const STATES: Record<string, { scenario: string; extra?: string; go?: (page: Page) => Promise<void>; ready: (page: Page) => Promise<void> }> = {
  L2: { scenario: "dashpay-registered", extra: "&fail=identities:none", ready: (p) => expect(p.getByTestId("identities-failed")).toBeVisible() },
  L3: { scenario: "dashpay-no-identity", ready: (p) => expect(p.getByTestId("identities-join")).toBeVisible() },
  L4: { scenario: "dashpay-identities", ready: (p) => expect(p.getByTestId("identity-row")).toHaveCount(2) },
  L5: { scenario: "dashpay-names-updating", ready: (p) => expect(p.getByTestId("identity-username")).toHaveText("Updating…") },
  detail: {
    scenario: "dashpay-identities",
    go: (p) => row(p, IDENTITY.alice).getByTestId("identity-keys").click(),
    ready: (p) => expect(p.getByTestId("identity-key")).toHaveCount(7),
  },
  "detail-failed": {
    scenario: "dashpay-identities",
    extra: "&fail=identity_detail:none",
    go: (p) => row(p, IDENTITY.alice).getByTestId("identity-keys").click(),
    ready: (p) => expect(p.getByTestId("identity-detail-failed")).toBeVisible(),
  },
  found: { scenario: "dashpay-identities", go: (p) => lookUp(p, "bob"), ready: (p) => expect(p.getByTestId("identity-found-keys")).toBeVisible() },
  "not-found": { scenario: "dashpay-identities", go: (p) => lookUp(p, "nobody"), ready: (p) => expect(p.getByTestId("identity-find-none")).toBeVisible() },
  invalid: { scenario: "dashpay-identities", go: (p) => lookUp(p, "a.b"), ready: (p) => expect(p.getByTestId("identity-find-invalid")).toBeVisible() },
  "find-failed": {
    scenario: "dashpay-identities",
    extra: "&fail=identity_find:none",
    go: (p) => lookUp(p, "bob"),
    ready: (p) => expect(p.getByTestId("identity-find-failed")).toBeVisible(),
  },
};

for (const theme of THEMES) {
  for (const [state, { scenario, extra, go, ready }] of Object.entries(STATES)) {
    test(`IOS-071 Identities ${state} ${theme}: renders, 0 serious axe issues`, async ({ page }) => {
      const errors = await openIdentities(page, scenario, theme, extra);
      await go?.(page);
      await ready(page);
      await fontsReady(page);
      expect(await seriousAxeViolations(page)).toEqual([]);
      // A `fail=` read is an expected rejection, which the page shows; nothing else may throw.
      expect(errors).toEqual([]);
    });
  }
}

test("IOS-071: the main badge is on the main identity's row; Set as main moves it and a relaunch keeps it", async ({ page }) => {
  // The clipboard as a variable: headless engines refuse writes (tools.spec.ts).
  await page.addInitScript(() => {
    Object.defineProperty(navigator, "clipboard", { value: { writeText: async (t: string) => void ((window as unknown as { __copied: string }).__copied = t) } });
  });
  const errors = await openIdentities(page, "dashpay-identities");
  await expect(mainRows(page)).toHaveCount(1);
  await expect(mainRows(page)).toHaveAttribute("data-identity", WORK_IDENTITY);
  await expect(row(page, WORK_IDENTITY).getByTestId("identity-main-badge")).toHaveText("Main");
  await expect(row(page, WORK_IDENTITY).getByTestId("identity-set-main")).toHaveCount(0);
  // The row shows the id shortened and copies all of it.
  await row(page, WORK_IDENTITY).getByRole("button", { name: "Copy identity id" }).click();
  expect(await page.evaluate(() => (window as unknown as { __copied?: string }).__copied)).toBe(WORK_IDENTITY);
  await expect(row(page, IDENTITY.alice).getByTestId("identity-main-badge")).toHaveCount(0);

  await row(page, IDENTITY.alice).getByTestId("identity-set-main").click();
  await expect(row(page, IDENTITY.alice).getByTestId("identity-result")).toHaveText("Main identity changed");
  await expect(mainRows(page)).toHaveAttribute("data-identity", IDENTITY.alice);
  // The shell's main identity follows: My Profile shows Alice.
  await page.evaluate(() => window.__dwFixture!.shellCommand({ kind: "navigate", route: "profile" }));
  await expect(page.getByTestId("profile-username")).toHaveText("@alice");

  // Quit and relaunch: the choice is on disk.
  await page.evaluate(() => window.__dwFixture!.relaunch());
  await page.getByTestId("lock-field").fill(FIXTURE_PASSPHRASE);
  await page.keyboard.press("Enter");
  await expect(page.getByTestId("lock-screen")).toHaveCount(0);
  await toAdvanced(page);
  await expect(mainRows(page)).toHaveAttribute("data-identity", IDENTITY.alice);
  expect(errors).toEqual([]);
});

test("IOS-071: an identity's keys: purpose, level, type, status and the DashPay binding; Set as main name", async ({ page }) => {
  await openIdentities(page, "dashpay-identities");
  await row(page, IDENTITY.alice).getByTestId("identity-keys").click();
  const detail = page.getByTestId("identity-detail");
  await expect(detail.getByTestId("identity-detail-revision")).toContainText("3");
  const keys = detail.getByTestId("identity-key");
  await expect(keys.first()).toContainText("Authentication");
  await expect(keys.first()).toContainText("Master");
  await expect(keys.first()).toContainText("ECDSA secp256k1");
  await expect(detail.locator("[data-testid=identity-key][data-status=disabled]")).toHaveCount(1);
  await expect(detail.locator("[data-testid=identity-key][data-status=disabled]").getByTestId("identity-key-status")).toHaveText("Disabled");
  await expect(keys.filter({ hasText: "DashPay contact requests" })).toHaveCount(2);
  await detail.getByTestId("identity-detail-close").click();

  await row(page, WORK_IDENTITY).getByTestId("identity-keys").click();
  await expect(page.locator("[data-testid=identity-name][data-main]")).toHaveText(/@alicework/);
  await page.getByTestId("identity-set-main-name").click();
  await expect(page.locator("[data-testid=identity-name][data-main]")).toHaveText(/@aliceatwork/);
  await page.getByTestId("identity-detail-close").click();
  await expect(row(page, WORK_IDENTITY).getByTestId("identity-username")).toHaveText("@aliceatwork");
});

test("IOS-071: Refresh, Refresh all and Find my identities", async ({ page }) => {
  await openIdentities(page, "dashpay-identities");
  await row(page, IDENTITY.alice).getByTestId("identity-refresh").click();
  await expect(row(page, IDENTITY.alice).getByTestId("identity-result")).toHaveText("Identity refreshed");

  await page.evaluate((id) => {
    window.__dwFixture!.world.dashpay.identityState!.refreshFailures[id] = "platform.unavailable";
  }, WORK_IDENTITY);
  await page.getByTestId("identities-refresh-all").click();
  await expect(page.getByTestId("identities-result")).toHaveText("1 identity could not be refreshed: Dash Platform did not answer. Try again.");

  await page.getByTestId("identities-find-mine").click();
  await expect(page.getByTestId("identities-result")).toHaveText("Found 1 new identity");
  await expect(row(page, SPARE_IDENTITY)).toBeVisible();
  await expect(row(page, SPARE_IDENTITY).getByTestId("identity-username")).toHaveText("No username");
  await expect(row(page, IDENTITY.alice).getByTestId("identity-choose-name")).toHaveCount(0);
  // D5: a row without a name offers DP1-09's credits path.
  await row(page, SPARE_IDENTITY).getByTestId("identity-choose-name").click();
  await expect(page.getByTestId("page-joinDashPay")).toBeVisible();
  await toAdvanced(page);
  await page.getByTestId("identities-find-mine").click();
  await expect(page.getByTestId("identities-result")).toHaveText("No new identities found");
  await expect(page.getByTestId("identity-row")).toHaveCount(3);
});

test("IOS-071: the look-up by username and by id; own identities say so; nothing is added", async ({ page }) => {
  await openIdentities(page, "dashpay-identities");
  await lookUp(page, "@Bob.dash");
  await expect(page.getByTestId("identity-found-how")).toHaveText("Found by username @bob");
  await expect(page.getByTestId("identity-found-names").locator("dd")).toHaveText("@bob");
  await expect(page.getByTestId("identity-found-own")).toHaveCount(0);
  await lookUp(page, WORK_IDENTITY);
  await expect(page.getByTestId("identity-found-how")).toHaveText("Found by identity ID");
  await expect(page.getByTestId("identity-found-own")).toHaveText("This is one of your identities");
  await expect(page.getByTestId("identity-row")).toHaveCount(2);
});
