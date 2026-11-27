import { expect, test, type Page } from "@playwright/test";
import type { NetworkKind } from "../src/api/generated/NetworkKind";
import { shortAddress } from "../src/format/tx";
import { seriousAxeViolations } from "./axe";
import { fixtureUrl, THEMES, trackPageErrors, type Theme } from "./helpers";

// A transaction's sheet says who, what, how much and when in the activity row's words, opens
// Insight on mainnet and testnet through the app, and keeps the txid, fee and inputs folded until asked.

type Kind = "sent" | "received" | "contact" | "unconfirmed";

function pick(kind: Kind) {
  const rows = window.__dwFixture!.world.wallets[0].rows;
  const row =
    kind === "sent"
      ? rows.find((r) => r.txType === "sendToAddress" && r.label === "Alice" && r.status.kind === "confirmed" && !r.counterparty)
      : kind === "received"
        ? rows.find((r) => r.category === "received" && r.amount > 0 && !r.label && !r.counterparty && (r.status.kind === "confirmed" || r.status.kind === "confirming"))
        : kind === "contact"
          ? rows.find((r) => r.counterparty?.displayName === "Bob Builder" && !r.label)
          : rows.find((r) => r.status.kind === "unconfirmed" && !r.status.instantLocked);
  if (!row) return null;
  return { txid: row.txid, address: row.address, amount: row.amount };
}

async function openKind(page: Page, kind: Kind, theme: Theme, network?: NetworkKind) {
  const scenario = kind === "contact" ? "dashpay-history-contacts" : "funded";
  await page.goto(fixtureUrl(scenario, theme, network ? `&network=${network}` : ""));
  await page.getByTestId("hero").waitFor();
  await page.getByTestId("sidebar-activity").click();
  await page.getByTestId("tx-list").waitFor();
  const row = await page.evaluate(pick, kind);
  expect(row).toBeTruthy();
  await page.locator(`.tx-row[data-txid="${row!.txid}"]`).first().click();
  await page.getByTestId("tx-detail-facts").waitFor();
  return row!;
}

const explorerCalls = (page: Page) => page.evaluate(() => window.__dwFixture!.calls.filter((c) => c === "open_tx_in_explorer"));
const explorerOpened = (page: Page) => page.evaluate(() => window.__dwFixture!.world.explorerOpened ?? []);

for (const theme of THEMES) {
  test(`plain words for a sent, a received, a contact and an unconfirmed payment, ${theme}`, async ({ page }) => {
    const errors = trackPageErrors(page);

    const sent = await openKind(page, "sent", theme);
    await expect(page.getByTestId("tx-detail-who")).toHaveText("Alice");
    await expect(page.getByTestId("tx-detail-title")).toHaveText("Sent");
    await expect(page.getByTestId("tx-detail-amount")).toContainText("-");
    await expect(page.getByTestId("tx-detail-when")).toContainText(" at ");
    await expect(page.getByTestId("tx-detail-status")).toHaveText("Confirmed");
    await expect(page.getByTestId("tx-detail")).not.toContainText("0/unconfirmed");
    const explorer = page.getByTestId("tx-detail-explorer");
    await expect(explorer).toHaveRole("button");
    await expect(explorer).toHaveText("View in explorer");
    await explorer.click();
    await expect.poll(() => explorerCalls(page)).toEqual(["open_tx_in_explorer"]);
    expect(await explorerOpened(page)).toEqual([sent.txid]);
    await expect(page.getByTestId("tx-detail-explorer-error")).toHaveCount(0);
    await page.evaluate(() => window.__dwFixture!.fail("open_tx_in_explorer", "none"));
    await explorer.click();
    await expect(page.getByTestId("tx-detail-explorer-error")).toHaveText("Could not open the explorer.");
    const folded = page.getByTestId("tx-detail-technical");
    await expect(folded).not.toHaveAttribute("open");
    await expect(page.getByTestId("tx-detail-txid")).toBeHidden();
    await expect(page.getByTestId("tx-detail-inputs")).toBeHidden();
    await expect(page.getByTestId("tx-detail-outputs")).toBeHidden();
    await folded.locator("summary").click();
    const txid = page.getByTestId("tx-detail-txid");
    const identifier = txid.locator("bdi[title]");
    await expect(identifier).toHaveAttribute("title", sent.txid);
    expect(await identifier.evaluate((value) => value.getClientRects().length)).toBe(1);
    await expect(txid).toContainText(`${sent.txid.slice(0, 8)}…${sent.txid.slice(-7)}`);
    await expect(folded).toContainText("Network fee");
    await expect(page.getByTestId("tx-detail-inputs")).toBeVisible();
    await expect(page.getByTestId("tx-detail-outputs")).toBeVisible();
    const raw = page.getByTestId("tx-detail-raw");
    await expect(raw).toContainText("Raw transaction");
    const copyRaw = raw.getByRole("button", { name: "Copy raw transaction" });
    await expect(copyRaw).toBeVisible();
    // The row shows the start; the button copies the whole hex (clipboard reads: Chromium only).
    if (test.info().project.name === "chromium") {
      await page.context().grantPermissions(["clipboard-read", "clipboard-write"]);
      await txid.getByRole("button", { name: "Copy transaction ID" }).click();
      expect(await page.evaluate(() => navigator.clipboard.readText())).toBe(sent.txid);
      await copyRaw.click();
      const copied = await page.evaluate(() => navigator.clipboard.readText());
      // The whole transaction, not the preview: two hex digits per byte of its Size.
      const size = Number((await folded.getByText(/^\d+ bytes$/).textContent())!.split(" ")[0]);
      expect(copied).toMatch(/^[0-9a-f]+$/);
      expect(copied.length).toBe(size * 2);
      await expect(raw).toContainText(`${copied.slice(0, 32)}…`);
    }

    const received = await openKind(page, "received", theme);
    await expect(page.getByTestId("tx-detail-title")).toHaveText("Received");
    await expect(page.getByTestId("tx-detail-who")).toHaveText(shortAddress(received.address!));
    await expect(page.getByTestId("tx-detail-amount")).toContainText("+");
    await expect(page.getByTestId("tx-detail-status")).toHaveText("Confirmed");
    await expect(page.getByTestId("tx-detail-technical")).not.toHaveAttribute("open");

    await openKind(page, "contact", theme);
    await expect(page.getByTestId("tx-detail-who")).toContainText("Bob Builder");
    await expect(page.getByTestId("tx-detail-who")).toContainText("@bob");
    // Bob's confirmed payment is a masternode reward: the activity row names him, and the sheet
    // says that with the same type title the row would use on its own.
    await expect(page.getByTestId("tx-detail-title")).toHaveText("Masternode reward");
    await expect(page.getByTestId("tx-detail-contact")).toContainText("Bob Builder");
    await expect(page.getByTestId("tx-detail-amount")).toContainText("+");
    await expect(page.getByTestId("tx-detail-status")).toHaveText("Confirmed");
    await expect(page.getByTestId("tx-detail-explorer")).toBeVisible();
    await expect(page.getByTestId("tx-detail-technical")).not.toHaveAttribute("open");

    await openKind(page, "unconfirmed", theme);
    await expect(page.getByTestId("tx-detail-status")).toHaveText("Waiting for confirmation");
    await expect(page.getByTestId("tx-detail-status")).not.toContainText("unconfirmed");
    await expect(page.getByTestId("tx-detail")).not.toContainText("0/unconfirmed");
    await expect(page.getByTestId("tx-detail-technical")).not.toHaveAttribute("open");

    expect(errors).toEqual([]);
  });

  test(`axe on the plain-words sheet, closed and opened, ${theme}`, async ({ page }) => {
    await openKind(page, "contact", theme);
    expect(await seriousAxeViolations(page)).toEqual([]);
    await page.getByTestId("tx-detail-technical").locator("summary").click();
    await page.getByTestId("tx-detail-txid").waitFor();
    expect(await seriousAxeViolations(page)).toEqual([]);
    await openKind(page, "unconfirmed", theme);
    expect(await seriousAxeViolations(page)).toEqual([]);
  });
}

test("the explorer button follows the network and is absent on regtest", async ({ page }) => {
  const main = await openKind(page, "sent", "light", "mainnet");
  await page.getByTestId("tx-detail-explorer").click();
  await expect.poll(() => explorerCalls(page)).toEqual(["open_tx_in_explorer"]);
  expect(await explorerOpened(page)).toEqual([main.txid]);
  expect(await page.evaluate(() => window.__dwFixture!.world.launch.network.kind)).toBe("mainnet");

  const testnet = await openKind(page, "received", "light", "testnet");
  await page.getByTestId("tx-detail-explorer").click();
  await expect.poll(() => explorerCalls(page)).toEqual(["open_tx_in_explorer"]);
  expect(await explorerOpened(page)).toEqual([testnet.txid]);

  await openKind(page, "sent", "light", "regtest");
  await expect(page.getByTestId("tx-detail-explorer")).toHaveCount(0);
  await expect(page.getByTestId("tx-detail-title")).toHaveText("Sent");
});

test("View in explorer asks once while the browser is opening", async ({ page }) => {
  const sent = await openKind(page, "sent", "light", "mainnet");
  const explorer = page.getByTestId("tx-detail-explorer");
  await page.evaluate(() => {
    (window as unknown as { __release: () => void }).__release = window.__dwFixture!.hold("open_tx_in_explorer");
  });
  await explorer.click();
  await expect(explorer).toBeDisabled();
  await expect(explorer).toHaveText("Opening the explorer…");
  await explorer.click({ force: true });
  await page.evaluate(() => (window as unknown as { __release: () => void }).__release());
  await expect(explorer).toBeEnabled();
  await expect(explorer).toHaveText("View in explorer");
  expect(await explorerCalls(page)).toEqual(["open_tx_in_explorer"]);
  expect(await explorerOpened(page)).toEqual([sent.txid]);
});
