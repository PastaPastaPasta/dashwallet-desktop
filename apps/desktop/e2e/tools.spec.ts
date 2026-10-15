import { expect, test, type Page } from "@playwright/test";
import { seriousAxeViolations } from "./axe";
import { fixtureUrl, fontsReady, THEMES, trackPageErrors, type Theme } from "./helpers";
import { PAYEE } from "./wallet";

// U-05: Tools ▸ Information, Console, Peers, Repair, Address Book, Sign/Verify, PSBT and Coin
// Selection on the fixture backend (src/fixture/tools.ts), with Pay's Create Unsigned and "Pay
// with these coins". Test names lead with the parity rows they cover.

const TABS = ["information", "console", "peers", "repair", "addressBook", "signVerify", "psbt", "coinSelection"] as const;
type ToolsTab = (typeof TABS)[number];

/** A binary PSBT the funded wallet signs (src/fixture/tools.ts FIXTURE_PSBT.unsigned). */
const UNSIGNED = Buffer.from("psbt\xffunsigned", "latin1").toString("base64");
const FOREIGN = Buffer.from("psbt\xffforeign", "latin1").toString("base64");

async function open(page: Page, scenario = "funded", theme: Theme = "light") {
  const errors = trackPageErrors(page);
  await page.goto(fixtureUrl(scenario, theme));
  await page.getByTestId("toolbar").waitFor();
  return errors;
}

async function tools(page: Page, tab: ToolsTab) {
  await page.evaluate((t) => window.__dwFixture!.shellCommand({ kind: "tools", tab: t }), tab);
  await expect(page.getByTestId(`tools-tabs-${tab}`)).toHaveAttribute("aria-selected", "true");
}

const world = <T>(page: Page, fn: (w: NonNullable<Window["__dwFixture"]>["world"]) => T) =>
  page.evaluate(`(${fn.toString()})(window.__dwFixture.world)`) as Promise<T>;

/** The clipboard as a variable: headless engines refuse writes. */
async function fakeClipboard(page: Page) {
  await page.addInitScript(() => {
    Object.defineProperty(navigator, "clipboard", {
      value: { writeText: async (t: string) => void ((window as unknown as { __copied: string }).__copied = t) },
    });
  });
}

const copied = (page: Page) => page.evaluate(() => (window as unknown as { __copied?: string }).__copied);

/** The passphrase sheet of a pending grant, answered. */
async function authorize(page: Page, passphrase = "demo") {
  const sheet = page.getByTestId("grant-sheet");
  await sheet.locator("input[type=password]").fill(passphrase);
  await sheet.locator("input[type=password]").press("Enter");
  await expect(sheet).toHaveCount(0);
}

test.describe("Information", () => {
  test("QT143_QT040: node facts, the most severe warning, and what is not built yet", async ({ page }) => {
    const errors = await open(page, "tools-warnings");
    await tools(page, "information");
    await expect(page.getByTestId("information-banner")).toBeVisible();
    await expect(page.getByTestId("tools-information")).toContainText("Requires full-node data source");
    await expect(page.getByTestId("information-dashpay-not-yet")).toContainText("Not yet (DP6-03).");
    await expect(page.getByTestId("information-traffic-not-yet")).toContainText("Not yet (M6).");
    expect(errors).toEqual([]);
  });
});

test.describe("Console", () => {
  test("QT145: runs a command, refuses a full-node one, keeps history and clears", async ({ page }) => {
    await open(page);
    await tools(page, "console");
    const line = page.getByTestId("console-line");
    await line.fill("getblockcount");
    await line.press("Enter");
    await expect(line).toHaveValue("");
    await expect(page.getByTestId("console-entry-reply").last()).toHaveText(/^\d+$/);
    await line.fill("getblock 1");
    await line.press("Enter");
    await expect(page.getByTestId("console-entry-error").last()).toContainText("Not available in SPV mode");
    await line.press("ArrowUp");
    await expect(line).toHaveValue("getblock 1");
    await line.press("ArrowUp");
    await expect(line).toHaveValue("getblockcount");
    // Tab completes a command name.
    await line.fill("getnetw");
    await line.press("Tab");
    await expect(line).toHaveValue("getnetworkinfo ");
    // Lines and history outlive a tab change (D1).
    await line.fill("");
    await tools(page, "peers");
    await tools(page, "console");
    await expect(page.getByTestId("console-entry-command")).toHaveCount(2);
    await line.press("ArrowUp");
    await expect(line).toHaveValue("getblock 1");
    await line.press("Control+l");
    await expect(page.getByTestId("console-entry-command")).toHaveCount(0);
  });

  test("QT145_D6: a sensitive line is never echoed or kept with its secret", async ({ page }) => {
    await open(page);
    await tools(page, "console");
    const line = page.getByTestId("console-line");
    await line.fill('walletpassphrase "hunter2" 60');
    await line.press("Enter");
    // The echo is there (redacted) before its absence counts; then history gives back only that.
    await expect(page.getByTestId("console-entry-command").last()).toHaveText(/walletpassphrase\(…\)/);
    await expect(line).toHaveValue("");
    await expect(page.getByTestId("console-log")).not.toContainText("hunter2");
    await line.press("ArrowUp");
    await expect(line).toHaveValue("walletpassphrase(…)");
  });

  test("QT145_D7: signmessage gets a signMessage grant (the unlocked vault asks nothing), then answers", async ({ page }) => {
    await open(page);
    await tools(page, "console");
    const address = await world(page, (w) => w.wallets[0].coins[0].address);
    await page.getByTestId("console-line").fill(`signmessage "${address}" "hello"`);
    await page.getByTestId("console-line").press("Enter");
    await expect(page.getByTestId("console-entry-reply").last()).toHaveText(/^[A-Za-z0-9+/]{86}[A-Za-z0-9+/=]{2}$/);
    expect(await world(page, (w) => w.grants.at(-1)?.grant.purpose.kind)).toBe("signMessage");
  });
});

test.describe("Peers", () => {
  test("QT147_IOS023: the peers, a peer's details, Change peers; no administration", async ({ page }) => {
    await open(page);
    await tools(page, "peers");
    const rows = page.getByTestId("peers-table").locator("tbody tr");
    await expect(rows).toHaveCount(8);
    await rows.first().getByRole("button").click();
    await expect(page.getByTestId("peer-detail")).toContainText("/Dash Core:23.1.2/");
    await page.getByTestId("peers-rotate").click();
    await expect(page.getByTestId("peers-notice")).toBeVisible();
    await expect(page.getByTestId("peers-administration")).toContainText("node administration");
  });
});

test.describe("Repair", () => {
  test("QT117: rescans with progress until done; a typed height must be a whole number", async ({ page }) => {
    await open(page);
    await tools(page, "repair");
    await page.getByTestId("repair-rescan-height").fill("12.5");
    await page.getByTestId("repair-rescan-from").click();
    await expect(page.getByTestId("repair-outcome")).toBeVisible();
    await expect(page.getByTestId("repair-progress")).toHaveCount(0);
    await page.getByTestId("repair-rescan-genesis").click();
    await expect(page.getByTestId("repair-progress")).toBeVisible();
    await page.getByTestId("repair-cancel-rescan").click();
    await expect(page.getByTestId("repair-progress")).toHaveCount(0);
    await expect.poll(() => world(page, (w) => w.tools.rescan)).toBeNull();
  });

  test("QT148: removing unconfirmed transactions and resetting chain data ask first", async ({ page }) => {
    await open(page);
    await tools(page, "repair");
    await page.getByTestId("repair-reset-chain").click();
    await page.getByTestId("repair-confirm-cancel").click();
    expect(await world(page, (w) => w.tools.resets)).toBe(0);
    await page.getByTestId("repair-reset-chain").click();
    await page.getByTestId("repair-confirm-ok").click();
    await expect(page.getByTestId("repair-outcome")).toBeVisible();
    expect(await world(page, (w) => w.tools.resets)).toBe(1);
    await page.getByTestId("repair-drop-unconfirmed").click();
    await expect(page.getByTestId("repair-confirm")).toBeVisible();
    await page.getByTestId("repair-confirm-ok").click();
    await expect(page.getByTestId("repair-confirm")).toHaveCount(0);
  });

  test("IOS034: the birth height saves and shows", async ({ page }) => {
    await open(page);
    await tools(page, "repair");
    await page.getByTestId("repair-birth-height").fill("1200000");
    await page.getByTestId("repair-birth-save").click();
    await expect(page.getByTestId("repair-birth")).toContainText("now 1200000");
  });

  test("IOS113_C9: stuck username funding resolves under a spend grant; asset locks are not yet", async ({ page }) => {
    await open(page, "tools-stuck-funding");
    await tools(page, "repair");
    const stuck = page.getByTestId("repair-stuck");
    await expect(stuck).toBeVisible();
    await stuck.getByRole("button").first().click();
    // It asks first (D12), then for the passphrase: the payment setting is on.
    await page.getByTestId("repair-confirm-ok").click();
    await authorize(page);
    await expect(page.getByTestId("repair-outcome")).toBeVisible();
    await expect(page.getByTestId("repair-asset-locks-not-yet")).toContainText("Not yet (DP1-09, DP1-02).");
  });
});

test.describe("Address Book", () => {
  test("QT095_QT098: New checks the address, refuses duplicates and saves; Edit and Delete", async ({ page }) => {
    await open(page);
    await tools(page, "addressBook");
    await page.getByTestId("address-book-new").click();
    await page.getByTestId("address-book-label").fill("Carol");
    await page.getByTestId("address-book-address").fill("nonsense");
    await page.getByTestId("address-book-save").click();
    await expect(page.getByTestId("address-book-error")).toContainText('"nonsense" is not a valid Dash address');
    const alice = page.getByTestId("address-book-table").locator("tr", { hasText: "Alice" });
    const aliceAddress = (await alice.locator("td.mono").textContent())!;
    await page.getByTestId("address-book-address").fill(aliceAddress);
    await page.getByTestId("address-book-save").click();
    await expect(page.getByTestId("address-book-error")).toContainText('with label "Alice"');
    await page.getByTestId("address-book-address").fill(PAYEE);
    await page.getByTestId("address-book-save").click();
    await expect(page.getByTestId("address-book-editor")).toHaveCount(0);
    await expect(page.getByTestId(`address-book-entry-${PAYEE}`)).toContainText("Carol");

    await page.getByTestId(`address-book-edit-${PAYEE}`).click();
    await expect(page.getByTestId("address-book-address")).toHaveAttribute("readonly");
    await page.getByTestId("address-book-label").fill("Carol B");
    await page.getByTestId("address-book-save").click();
    await expect(page.getByTestId(`address-book-entry-${PAYEE}`)).toContainText("Carol B");
    await page.getByTestId(`address-book-delete-${PAYEE}`).click();
    await expect(page.getByTestId(`address-book-entry-${PAYEE}`)).toHaveCount(0);
  });

  test("QT095_QT096: wildcard search, receiving entries without New or Delete, CSV export, QR", async ({ page }) => {
    await fakeClipboard(page);
    await open(page);
    await tools(page, "addressBook");
    const rows = page.getByTestId("address-book-table").locator("tbody tr");
    await expect(rows).toHaveCount(2);
    await page.getByTestId("address-book-search").fill("b?b*");
    await expect(rows).toHaveCount(1);
    await expect(rows.first()).toContainText("Bob's shop");
    await page.getByTestId("address-book-search").fill("");

    await page.getByTestId("address-book-export").click();
    await expect(page.getByTestId("address-book-exported")).toContainText("Sending addresses.csv");
    const saved = await world(page, (w) => w.tools.files.saved.at(-1)!);
    expect(saved.purpose).toBe("csv");
    const csv = Buffer.from(saved.data, "base64").toString("utf8");
    expect(csv.split("\n")[0]).toBe('"Label","Address"');
    expect(csv).toContain('"Bob\'s shop",');

    await page.getByTestId("address-book-receive").click();
    await expect(page.getByTestId("address-book-new")).toHaveCount(0);
    await expect(page.locator("[data-testid^=address-book-delete-]")).toHaveCount(0);
    const payroll = page.getByTestId("address-book-table").locator("tr", { hasText: "Payroll" });
    await payroll.locator("summary").click();
    await payroll.getByRole("button", { name: "Copy address" }).click();
    expect(await copied(page)).toBe((await payroll.locator("td.mono").textContent())!);
    await payroll.getByRole("button", { name: /QR code/ }).click();
    await expect(page.getByTestId("address-book-qr-code")).toBeVisible();
    await expect(page.getByTestId("address-book-qr")).toContainText("label=Payroll");
  });

  test("QT097: Pay's address field chooses from the sending book, with its label", async ({ page }) => {
    await open(page);
    await page.getByTestId("sidebar-pay").click();
    await page.getByTestId("pay-tabs-address").click();
    await page.getByTestId("pay-address-book").click();
    const picker = page.getByTestId("address-book-picker");
    await picker.locator("tr", { hasText: "Bob's shop" }).getByRole("button", { name: /Choose/ }).click();
    await expect(picker).toHaveCount(0);
    await expect(page.getByTestId("pay-address")).toHaveValue(/^y/);
    await expect(page.getByTestId("pay-label")).toHaveValue("Bob's shop");
  });
});

test.describe("Sign/Verify", () => {
  test("QT099_QT100_QT097: sign with a receiving address from the book, then verify", async ({ page }) => {
    await open(page);
    await tools(page, "signVerify");
    await page.getByTestId("sign-address").fill("nonsense");
    await page.getByTestId("sign-sign").click();
    await expect(page.getByTestId("sign-result")).toContainText("The entered address is invalid.");

    await page.getByTestId("sign-choose").click();
    await page.getByTestId("address-book-picker").locator("tr", { hasText: "Payroll" }).getByRole("button", { name: /Choose/ }).click();
    const address = await page.getByTestId("sign-address").inputValue();
    await page.getByTestId("sign-message").fill("hello dash");
    await page.getByTestId("sign-sign").click();
    await expect(page.getByTestId("sign-result")).toHaveText("Message signed.");
    const signature = await page.getByTestId("sign-signature").inputValue();

    await page.getByTestId("verify-address").fill(address);
    await page.getByTestId("verify-message").fill("hello dash");
    await page.getByTestId("verify-signature").fill(signature);
    await page.getByTestId("verify-verify").click();
    await expect(page.getByTestId("verify-result")).toHaveText("Message verified.");
    await expect(page.getByTestId("verify-result")).toHaveClass("form-ok");
    await page.getByTestId("verify-message").fill("hello dash!");
    await page.getByTestId("verify-verify").click();
    await expect(page.getByTestId("verify-result")).toHaveClass("form-problem");
    await page.getByTestId("verify-clear").click();
    await expect(page.getByTestId("verify-result")).toHaveCount(0);
  });
});

test.describe("PSBT", () => {
  test("QT078_QT079: paste, sign under a spend grant, broadcast once", async ({ page }) => {
    await open(page);
    await tools(page, "psbt");
    await page.getByTestId("psbt-paste").fill("not base64!");
    await page.getByTestId("psbt-load-text").click();
    await expect(page.getByTestId("psbt-error")).toHaveText("Unable to decode PSBT (invalid base64)");
    await page.getByTestId("psbt-paste").fill(UNSIGNED);
    await page.getByTestId("psbt-load-text").click();
    await expect(page.getByTestId("psbt-lines")).toContainText("(own address)");
    await expect(page.getByTestId("psbt-lines")).toContainText("Transaction has 2 unsigned inputs.");
    await expect(page.getByTestId("psbt-broadcast")).toBeDisabled();
    await page.getByTestId("psbt-sign").click();
    await authorize(page);
    await expect(page.getByTestId("psbt-message")).toHaveText("Signed transaction successfully. Transaction is ready to broadcast.");
    await page.getByTestId("psbt-broadcast").click();
    await expect(page.getByTestId("psbt-message")).toContainText("Transaction broadcast successfully! Transaction ID:");
    await expect(page.getByTestId("psbt-broadcast")).toBeDisabled();
    expect(await world(page, (w) => w.tools.broadcasts.length)).toBe(1);
  });

  test("QT078_QT077_D2: load from the open dialog, save as a binary .psbt; a cancel does nothing", async ({ page }) => {
    await open(page);
    await tools(page, "psbt");
    await page.getByTestId("psbt-open-file").click();
    await expect(page.getByTestId("psbt-operations")).toHaveCount(0);
    await page.evaluate((data) => (window.__dwFixture!.world.tools.files.open = { name: "foreign.psbt", data }), FOREIGN);
    await page.getByTestId("psbt-open-file").click();
    await expect(page.getByTestId("psbt-status")).toContainText("Transaction still needs signature(s).");
    await expect(page.getByTestId("psbt-sign")).toBeDisabled();
    await page.getByTestId("psbt-save").click();
    await expect(page.getByTestId("psbt-message")).toHaveText("PSBT saved to disk.");
    const saved = await world(page, (w) => w.tools.files.saved.at(-1)!);
    expect(saved).toMatchObject({ purpose: "psbt", data: FOREIGN });
    expect(saved.path).toMatch(/\.psbt$/);
    await expect(page.getByTestId("psbt-external-signer-not-yet")).toContainText("Not yet (M6).");
  });

  test("QT076_QT077_D18: Pay's Create Unsigned copies the PSBT and opens it here", async ({ page }) => {
    await fakeClipboard(page);
    await open(page);
    await page.getByTestId("sidebar-pay").click();
    await page.getByTestId("pay-tabs-address").click();
    await page.getByTestId("pay-address").fill(PAYEE);
    await page.getByTestId("pay-amount").fill("0.5");
    await expect(page.getByTestId("pay-continue")).toBeEnabled();
    await page.getByTestId("pay-continue").click();
    await page.getByTestId("pay-confirm-unsigned").click();
    await expect(page.getByTestId("tools-tabs-psbt")).toHaveAttribute("aria-selected", "true");
    await expect(page.getByTestId("psbt-message")).toHaveText("The PSBT has been copied to the clipboard. You can also save it.");
    await expect(page.getByTestId("psbt-lines")).toContainText(`Sends 0.50000000 tDASH to ${PAYEE}`);
    expect(await copied(page)).toMatch(/^[A-Za-z0-9+/]+=*$/);
    // Nothing was signed or sent.
    expect(await world(page, (w) => w.tools.broadcasts.length)).toBe(0);
  });

  test("QT076_D18: a watch-only wallet's confirm sheet offers Create Unsigned instead of Send", async ({ page }) => {
    await open(page, "watch-only");
    await page.getByTestId("sidebar-pay").click();
    await page.getByTestId("pay-tabs-address").click();
    await page.getByTestId("pay-address").fill(PAYEE);
    await page.getByTestId("pay-amount").fill("0.5");
    await expect(page.getByTestId("pay-continue")).toBeEnabled();
    await page.getByTestId("pay-continue").click();
    await expect(page.getByTestId("pay-confirm-unsigned")).toBeVisible();
    await expect(page.getByTestId("pay-confirm-send")).toHaveCount(0);
    await expect(page.getByTestId("pay-confirm-passphrase")).toHaveCount(0);
  });
});

test.describe("Coin Selection", () => {
  test("QT069_QT071_QT072: Pay's coin table with dash-qt's columns, CoinJoin coins hidden, the summary", async ({ page }) => {
    await open(page, "tools-coinjoin");
    await tools(page, "coinSelection");
    const rows = page.getByTestId("coin-table").locator("tbody tr");
    await expect(rows).toHaveCount(5);
    await page.getByTestId("coins-coinjoin").click();
    await expect(rows).toHaveCount(8);
    await expect(page.getByTestId("coins-coinjoin")).toHaveText("Hide CoinJoin coins");
    await page.getByTestId("coins-coinjoin").click();
    // Sorted by amount, largest first; a header flips it.
    await expect(rows.first()).toContainText("20.00");
    await page.getByTestId("coin-sort-amount").click();
    await expect(rows.first()).toContainText("0.19698287");
    await page.getByTestId("coins-select-all").click();
    await expect(page.getByTestId("coin-summary")).toContainText("Quantity4");
    await expect(page.getByTestId("coins-selection")).toContainText("Can vary +/-");
    await page.getByTestId("coins-mode-tree").click();
    await expect(page.locator("[data-testid^=coin-group-]")).toHaveCount(5);
    // The view and the selection outlive a tab change (D8).
    await tools(page, "psbt");
    await tools(page, "coinSelection");
    await expect(page.getByTestId("coins-mode-tree")).toHaveAttribute("aria-pressed", "true");
    await expect(page.getByTestId("coin-summary")).toContainText("Quantity4");
  });

  test("QT070: lock and unlock from a row's menu and with Toggle lock state; copy the outpoint", async ({ page }) => {
    await fakeClipboard(page);
    await open(page);
    await tools(page, "coinSelection");
    const menu = page.locator("summary[data-testid^=coin-menu-]").first();
    const key = (await menu.getAttribute("data-testid"))!.replace("coin-menu-", "");
    // Escape closes the menu and focus goes back to its button.
    await menu.click();
    await page.getByTestId(`coin-menu-${key}-copy-transaction-id-and-output-index`).focus();
    await page.keyboard.press("Escape");
    await expect(page.getByTestId(`coin-menu-${key}-copy-transaction-id-and-output-index`)).toBeHidden();
    await expect(menu).toBeFocused();
    await menu.click();
    await page.getByTestId(`coin-menu-${key}-copy-transaction-id-and-output-index`).click();
    expect(await copied(page)).toBe(key);
    await menu.click();
    await page.getByTestId(`coin-menu-${key}-lock-unspent`).click();
    await expect(page.getByTestId("coins-locked")).toHaveText("(1 locked)");
    await page.getByTestId("coins-toggle-locks").click();
    await expect(page.getByTestId("coins-locked")).not.toHaveText("(1 locked)");
    await page.getByTestId("coins-toggle-locks").click();
    await expect(page.getByTestId("coins-locked")).toHaveText("(1 locked)");
  });

  test("QT068_D19: Pay with these coins opens Pay with Advanced on that selection", async ({ page }) => {
    await open(page);
    await tools(page, "coinSelection");
    const boxes = page.getByTestId("coin-table").locator("tbody input:enabled");
    await boxes.nth(0).check();
    await boxes.nth(1).check();
    await page.getByTestId("coins-pay").click();
    await expect(page.getByTestId("page-pay")).toBeVisible();
    await expect(page.getByTestId("pay-advanced")).toHaveAttribute("open");
    await expect(page.getByTestId("coin-choice-selected")).toBeChecked();
    await expect(page.getByTestId("coin-table").locator("tbody input:checked")).toHaveCount(2);
  });
});

for (const theme of THEMES) {
  for (const tab of TABS) {
    test(`axe: Tools ▸ ${tab}, ${theme}`, async ({ page }) => {
      await open(page, "tools-stuck-funding", theme);
      await tools(page, tab);
      await page.getByTestId(`tools-${tab}`).waitFor();
      await fontsReady(page);
      expect(await seriousAxeViolations(page), tab).toEqual([]);
    });
  }

  test(`axe: the Tools sheets, ${theme}`, async ({ page }) => {
    // The address book's editor, Repair's question and the grant sheet.
    await open(page, "tools-stuck-funding", theme);
    await fontsReady(page);
    await tools(page, "addressBook");
    await page.getByTestId("address-book-new").click();
    expect(await seriousAxeViolations(page), "address book editor").toEqual([]);
    await page.getByTestId("address-book-cancel").click();
    await tools(page, "repair");
    await page.getByTestId("repair-reset-chain").click();
    expect(await seriousAxeViolations(page), "repair confirm").toEqual([]);
    await page.getByTestId("repair-confirm-cancel").click();
    await page.getByTestId("repair-stuck").getByRole("button").first().click();
    await page.getByTestId("repair-confirm-ok").click();
    await page.getByTestId("grant-sheet").waitFor();
    expect(await seriousAxeViolations(page), "grant sheet").toEqual([]);
  });
}
