import { mkdirSync } from "node:fs";
import { join } from "node:path";
import { expect, test, type Page } from "@playwright/test";
import type { LinkRoute } from "../src/api/generated/LinkRoute";
import type { Route } from "../src/api/generated/Route";
import type { ShellCommand } from "../src/api/generated/ShellCommand";
import { FIXTURE_WALLET_ID } from "../src/fixture/scenarios";
import { seriousAxeViolations } from "./axe";
import { artifactPath, fixtureUrl, fontsReady, THEMES, trackPageErrors } from "./helpers";

// U-01: every route of the shell (DASHPAY §4) renders in light and dark with 0 serious axe
// issues; links, menu commands and the toolbar route as dw-app's shell asks. dw-app's routing of
// the link text itself is tested in Rust (shell/links.rs); here the fixture hands over the routes.

const ROUTES: Route[] = [
  "home",
  "pay",
  "receive",
  "contacts",
  "activity",
  "coinJoin",
  "explore",
  "notifications",
  "profile",
  "settings",
  "tools",
  "joinDashPay",
];

/** DW_ROUTE_SHOTS=<dir>: each route's screenshot is also written there. */
const SHOTS = process.env.DW_ROUTE_SHOTS;

async function open(page: Page, scenario: string, theme: "light" | "dark" = "light") {
  const errors = trackPageErrors(page);
  await page.goto(fixtureUrl(scenario, theme));
  await page.getByTestId("toolbar").waitFor();
  return errors;
}

const command = (page: Page, c: ShellCommand) => page.evaluate((c) => window.__dwFixture!.shellCommand(c), c);
const deliver = (page: Page, routes: LinkRoute[]) => page.evaluate((r) => window.__dwFixture!.deliverLinks(r), routes);

const ADDRESS = "yNPbcFfabtNmmxKdGwhHomdYfVs6gikbPf";
const IDENTITY = "8tvHgfq4BLx3bRBcxSs7ZsjaTnAZDSmLhNpAgXyNomeR";

for (const theme of THEMES) {
  for (const route of ROUTES) {
    test(`route ${route} ${theme}: renders, 0 serious axe issues`, async ({ page }, info) => {
      const errors = await open(page, "dashpay-registered", theme);
      await command(page, { kind: "navigate", route });
      await expect(page.getByTestId(`page-${route}`)).toBeVisible();
      await fontsReady(page);
      const name = `${theme}-route-${route}.png`;
      await page.screenshot({ path: artifactPath(info, name) });
      if (SHOTS && info.project.name === "chromium") {
        mkdirSync(SHOTS, { recursive: true });
        await page.screenshot({ path: join(SHOTS, name) });
      }
      expect(await page.locator("html").getAttribute("data-theme")).toBe(theme);
      expect(await seriousAxeViolations(page)).toEqual([]);
      expect(errors).toEqual([]);
    });
  }
}

test("the sidebar opens the DASHPAY sections in order, Explore hidden", async ({ page }) => {
  await open(page, "dashpay-registered");
  const ids = await page.getByTestId("sidebar").locator("button").evaluateAll((els) => els.map((e) => e.getAttribute("data-testid")));
  expect(ids).toEqual(["sidebar-home", "sidebar-pay", "sidebar-receive", "sidebar-contacts", "sidebar-activity", "sidebar-coinJoin"]);
  for (const id of ids) {
    await page.getByTestId(id!).click();
    await expect(page.getByTestId(id!)).toHaveAttribute("aria-current", "page");
    await expect(page.getByTestId(`page-${id!.slice("sidebar-".length)}`)).toBeVisible();
  }
});

test("a payment link opens Pay ▸ To an address, prefilled", async ({ page }) => {
  await open(page, "funded");
  await deliver(page, [
    {
      kind: "pay",
      uri: { address: ADDRESS, amount: 150_000_000, label: "Coffee", message: "Thanks" },
      username: "bob",
    },
  ]);
  await expect(page.getByTestId("page-pay")).toBeVisible();
  await expect(page.getByTestId("pay-tabs-address")).toHaveAttribute("aria-selected", "true");
  await expect(page.getByTestId("pay-address")).toHaveValue(ADDRESS);
  await expect(page.getByTestId("pay-amount")).toHaveValue(/^1\.5/);
  await expect(page.getByTestId("pay-label")).toHaveValue("Coffee");
  await expect(page.getByTestId("pay-message")).toHaveValue("Thanks");
  await expect(page.getByTestId("pay-username")).toContainText("@bob");
  // Review waits for the engine's estimate of the prefilled payment.
  await expect(page.getByTestId("pay-continue")).toBeEnabled();
  // The page's title takes the focus.
  await expect(page.getByTestId("page-pay").locator("h1")).toBeFocused();
  // The same link again starts a fresh form.
  await page.getByTestId("pay-label").fill("Edited");
  await deliver(page, [{ kind: "pay", uri: { address: ADDRESS, amount: 150_000_000, label: "Coffee", message: "Thanks" }, username: "bob" }]);
  await expect(page.getByTestId("pay-label")).toHaveValue("Coffee");
});

test("dashpay and dashwallet links open their routes; refused ones say why", async ({ page }) => {
  await open(page, "dashpay-registered");
  await deliver(page, [{ kind: "addContact", identity: IDENTITY, username: "bob" }]);
  await expect(page.getByTestId("add-contact-username")).toHaveText("@bob");
  await expect(page.getByTestId("add-contact-identity")).toHaveText(IDENTITY);

  await deliver(page, [{ kind: "scanQr" }]);
  await expect(page.getByTestId("pay-tabs-address")).toHaveAttribute("aria-selected", "true");
  await expect(page.getByTestId("pay-address")).toHaveValue("");

  await deliver(page, [{ kind: "invitation" }]);
  await expect(page.getByTestId("page-joinDashPay")).toBeVisible();
  await expect(page.getByTestId("invitation-received")).toBeVisible();

  await deliver(page, [{ kind: "rejected", reason: { kind: "payment", reason: { kind: "invalidAddress", problem: { kind: "invalidBase58Prefix" } } } }]);
  await expect(page.getByText("not valid on this network")).toBeVisible();
  await deliver(page, [{ kind: "unsupported", what: "addressRequest" }]);
  await expect(page.getByText("does not hand addresses to other apps")).toBeVisible();
});

test("several links route in order; the last one shows", async ({ page }) => {
  await open(page, "funded");
  await deliver(page, [{ kind: "scanQr" }, { kind: "invitation" }]);
  await expect(page.getByTestId("page-joinDashPay")).toBeVisible();
  expect(await page.evaluate(() => window.__dwFixture!.world.links.length)).toBe(0);
});

test("overlapping link drains apply in drain order; the later link shows", async ({ page }) => {
  await open(page, "funded");
  // The start-up take is done, so the hold catches link A's drain.
  await expect.poll(() => page.evaluate(() => window.__dwFixture!.calls.includes("take_links"))).toBe(true);
  // Link A is drained first and its reply held; link B arrives meanwhile.
  await page.evaluate(() => {
    (window as unknown as { release: () => void }).release = window.__dwFixture!.hold("take_links");
  });
  await deliver(page, [{ kind: "pay", uri: { address: ADDRESS, amount: null, label: null, message: null }, username: null }]);
  await expect.poll(() => page.evaluate(() => window.__dwFixture!.world.links.length)).toBe(0);
  await deliver(page, [{ kind: "invitation" }]);
  // Give a concurrent drain the time to take B and apply it before A's reply.
  await page.waitForTimeout(250);
  await expect(page.getByTestId("page-pay")).toHaveCount(0);
  await page.evaluate(() => (window as unknown as { release: () => void }).release());
  await expect(page.getByTestId("page-joinDashPay")).toBeVisible();
  // B stays: A's late batch did not land on top of it.
  await page.waitForTimeout(250);
  await expect(page.getByTestId("page-joinDashPay")).toBeVisible();
  expect(await page.evaluate(() => window.__dwFixture!.world.links.length)).toBe(0);
});

test("a link that arrives while locked shows its page after unlocking", async ({ page }) => {
  await open(page, "locked");
  await deliver(page, [{ kind: "pay", uri: { address: ADDRESS, amount: null, label: null, message: null }, username: null }]);
  await page.getByTestId("lock-field").fill("demo");
  await page.keyboard.press("Enter");
  await expect(page.getByTestId("lock-screen")).toHaveCount(0);
  await expect(page.getByTestId("pay-address")).toHaveValue(ADDRESS);
});

test("File ▸ Open URI routes a typed link, and says why it cannot", async ({ page }) => {
  await open(page, "funded");
  await command(page, { kind: "openUri" });
  const input = page.getByTestId("open-uri-input");
  await expect(input).toBeFocused();
  await input.fill(`dash://${ADDRESS}`);
  await page.keyboard.press("Enter");
  await expect(page.getByTestId("open-uri-problem")).toContainText("dash://");
  await input.fill(`pay:${ADDRESS}?amount=0.25&label=Rent`);
  await page.keyboard.press("Enter");
  await expect(page.getByTestId("open-uri")).toHaveCount(0);
  await expect(page.getByTestId("pay-label")).toHaveValue("Rent");
});

test("menu commands: navigate, tools tabs, discreet, about, lock; the menus mirror the state", async ({ page }) => {
  await open(page, "funded");
  await command(page, { kind: "tools", tab: "console" });
  await expect(page.getByTestId("tools-tabs-console")).toHaveAttribute("aria-selected", "true");
  // The command focuses the page's title (D19).
  await expect(page.getByTestId("page-tools").locator("h1")).toBeFocused();
  // Arrow keys, Home and End move between tabs.
  await page.getByTestId("tools-tabs-console").focus();
  await page.keyboard.press("ArrowRight");
  await expect(page.getByTestId("tools-tabs-peers")).toBeFocused();
  await expect(page.getByTestId("tools-peers")).toBeVisible();
  await page.keyboard.press("End");
  await expect(page.getByTestId("tools-tabs-coinSelection")).toBeFocused();
  await page.keyboard.press("Home");
  await expect(page.getByTestId("tools-tabs-information")).toHaveAttribute("aria-selected", "true");

  await command(page, { kind: "toggleDiscreet" });
  await expect(page.getByTestId("toolbar-discreet")).toHaveAttribute("aria-pressed", "true");
  await expect.poll(() => page.evaluate(() => window.__dwFixture!.world.shellSync)).toEqual({ discreet: true, canLock: true, secretShown: false });

  await command(page, { kind: "about" });
  await expect(page.getByTestId("about-version")).toHaveText("0.1.0");
  await page.keyboard.press("Escape");
  await expect(page.getByTestId("about")).toHaveCount(0);

  await command(page, { kind: "lock" });
  await expect(page.getByTestId("lock-screen")).toBeVisible();
  await expect.poll(() => page.evaluate(() => window.__dwFixture!.world.shellSync?.canLock)).toBe(false);

  // A dialog asked for while locked is dropped, not shown after unlocking.
  await command(page, { kind: "openUri" });
  await page.getByTestId("lock-field").fill("demo");
  await page.keyboard.press("Enter");
  await expect(page.getByTestId("lock-screen")).toHaveCount(0);
  await expect(page.getByTestId("open-uri")).toHaveCount(0);
});

test("a route command closes an open dialog and focuses its page", async ({ page }) => {
  await open(page, "funded");
  await command(page, { kind: "openUri" });
  await expect(page.getByTestId("open-uri-input")).toBeFocused();
  await command(page, { kind: "navigate", route: "activity" });
  await expect(page.getByTestId("open-uri")).toHaveCount(0);
  await expect(page.getByTestId("page-activity").locator("h1")).toBeFocused();
  await command(page, { kind: "about" });
  await expect(page.getByTestId("about")).toBeVisible();
  await command(page, { kind: "tools", tab: "peers" });
  await expect(page.getByTestId("about")).toHaveCount(0);
  await expect(page.getByTestId("page-tools").locator("h1")).toBeFocused();
});

test("in a browser the menus' shortcuts work from the keyboard", async ({ page }) => {
  await open(page, "funded");
  await page.keyboard.press("Control+5");
  await expect(page.getByTestId("page-activity")).toBeVisible();
  await page.keyboard.press("Control+2");
  await expect(page.getByTestId("page-pay")).toBeVisible();
  await page.keyboard.press("Control+Shift+D");
  await expect(page.getByTestId("toolbar-discreet")).toHaveAttribute("aria-pressed", "true");
  await page.keyboard.press("Control+Shift+I");
  await expect(page.getByTestId("tools-tabs-information")).toHaveAttribute("aria-selected", "true");
  await page.keyboard.press("Control+O");
  await expect(page.getByTestId("open-uri")).toBeVisible();
});

test("toolbar: identity chip, bell and Contacts badge follow DashPay", async ({ page }) => {
  await open(page, "dashpay-registered");
  const chip = page.getByTestId("toolbar-chip");
  await expect(chip).toContainText("@alice");
  await expect(page.getByTestId("toolbar-bell-badge")).toBeVisible();
  await expect(page.getByTestId("sidebar-contacts-badge")).toBeVisible();
  await chip.click();
  await expect(page.getByTestId("page-profile")).toBeVisible();
  // The bell opens its popover (DP2-09 D11), and the popover the page.
  await page.getByTestId("toolbar-bell").click();
  await page.getByTestId("bell-see-all").click();
  await expect(page.getByTestId("page-notifications")).toBeVisible();
});

test("toolbar: without an identity the chip and the sidebar lead to Join DashPay", async ({ page }) => {
  await open(page, "dashpay-no-identity");
  await expect(page.getByTestId("toolbar-chip")).toHaveText("Join DashPay");
  await expect(page.getByTestId("sidebar-contacts")).toHaveCount(0);
  await page.getByTestId("sidebar-joinDashPay").click();
  await expect(page.getByTestId("page-joinDashPay")).toBeVisible();
  await expect(page.getByTestId("toolbar-bell-badge")).toHaveCount(0);
});

test("toolbar: a registration in progress shows on the chip", async ({ page }) => {
  await open(page, "dashpay-registering");
  await expect(page.getByTestId("toolbar-chip")).toHaveText("Registering…");
  await expect(page.getByTestId("sidebar-contacts")).toHaveCount(0);
  await expect(page.getByTestId("sidebar-joinDashPay")).toBeVisible();
});

test("toolbar: a contested username shows Pending and keeps Contacts, its bell and badge", async ({ page }) => {
  await open(page, "dashpay-contest-pending");
  await expect(page.getByTestId("toolbar-chip")).toHaveText("@alice · Pending");
  await expect(page.getByTestId("sidebar-joinDashPay")).toHaveCount(0);
  await expect(page.getByTestId("sidebar-contacts-badge")).toBeVisible();
  await expect(page.getByTestId("toolbar-bell-badge")).toBeVisible();
  await page.getByTestId("sidebar-contacts").click();
  await expect(page.getByTestId("contact-row").first()).toBeVisible();
});

test("toolbar: the wallet picker switches the wallet the window shows", async ({ page }) => {
  await open(page, "two-wallets");
  await page.getByTestId("hero").waitFor();
  const picker = page.getByTestId("toolbar-wallet-picker");
  await expect(picker).toHaveValue(FIXTURE_WALLET_ID);
  await picker.selectOption({ label: "Savings" });
  await expect(page.getByTestId("history-empty")).toBeVisible();
});
