import { expect, test, type Page } from "@playwright/test";
import { FIXTURE_PASSPHRASE, FIXTURE_WALLET_ID, type World } from "../src/fixture/scenarios";
import { FIXTURE_NOW } from "../src/fixture/wallet";
import { fixtureUrl, trackPageErrors } from "./helpers";
import { PAYEE } from "./wallet";

// A locked window holds nothing of the wallet: the lock screen stands in place of the pages, not
// over them, so the page's text and markup carry no balance, wallet name, history row, contact or
// address until the unlock, and the renderer reads none of them meanwhile.

const BALANCE = "32.04698287";

/** What the renderer may ask the engine while locked: the window, the lock and its report, the menus. */
const LOCKED_CALLS = new Set(["launch_info", "ui_settings", "open_network", "lock_status", "last_lock_report", "leases", "dispatch_status", "shell_sync", "take_links"]);

const world = <T>(page: Page, read: (w: World) => T) => page.evaluate(`(${read.toString()})(window.__dwFixture.world)`) as Promise<T>;
const calls = (page: Page) => page.evaluate(() => [...window.__dwFixture!.calls]);

/** The wallet's own words the fixture knows: its name, its contacts' names, its addresses and labels. */
const walletWords = (page: Page) =>
  world(page, (w) => {
    const wallet = w.wallets[0];
    const words = [wallet.summary.name];
    for (const c of w.dashpay.contacts) words.push(c.summary.username ?? "", c.profile?.displayName ?? "");
    for (const r of wallet.rows) words.push(r.address ?? "", r.label ?? "");
    return words.filter((s) => s.length >= 3);
  });

const escape = (s: string) => s.replace(/[.*+?^${}()|[\]\\]/g, "\\$&");

/** The whole document as text and markup: what an assistive tool or a capture could read. */
const pageHolds = (page: Page) => page.evaluate(() => `${document.body.innerText}\n${document.documentElement.outerHTML}\n${document.title}`);

async function expectHoldsNothing(page: Page, words: string[], rows: string[]) {
  await expect(page.getByTestId("lock-field")).toBeVisible();
  await expect(page.locator(".app-chrome")).toHaveCount(0);
  const held = await pageHolds(page);
  expect(held).not.toContain(BALANCE);
  for (const w of words) expect(held, w).not.toMatch(new RegExp(`\\b${escape(w)}\\b`));
  for (const r of rows) expect(held, r).not.toContain(r);
}

async function unlock(page: Page) {
  await page.getByTestId("lock-field").fill(FIXTURE_PASSPHRASE);
  await page.getByTestId("lock-field").press("Enter");
  await expect(page.getByTestId("lock-screen")).toHaveCount(0);
}

/** The text of the first few history rows on screen. */
const rowTexts = (page: Page) => page.locator("[data-txid]").evaluateAll((els) => els.slice(0, 3).map((e) => (e as HTMLElement).innerText.split("\n").find((l) => /\d/.test(l)) ?? ""));

test("a launch while locked holds and reads nothing of the wallet until the unlock", async ({ page }) => {
  const errors = trackPageErrors(page);
  await page.goto(fixtureUrl("locked", "light"));
  await page.getByTestId("lock-field").waitFor();
  const words = await walletWords(page);
  expect(words.length).toBeGreaterThan(3);
  await expectHoldsNothing(page, words, []);
  const before = await page.evaluate(() => document.documentElement.outerHTML);
  const asked = await calls(page);
  expect(asked.filter((c) => !LOCKED_CALLS.has(c))).toEqual([]);

  await unlock(page);
  await expect(page.getByTestId("hero")).toContainText(BALANCE);
  await expect(page.locator(".toolbar-wallet")).toHaveText("Main");
  await expect(page.locator("[data-txid]").first()).toBeVisible();
  const rows = (await rowTexts(page)).filter(Boolean);
  expect(rows.length).toBeGreaterThan(0);
  for (const r of rows) expect(before).not.toContain(r);
  expect(errors).toEqual([]);
});

test("a lock while in use takes the pages down, and the unlock lands on the page it was on", async ({ page }) => {
  const errors = trackPageErrors(page);
  await page.goto(fixtureUrl("locked", "light"));
  await page.getByTestId("lock-field").waitFor();
  const words = await walletWords(page);
  await unlock(page);
  await page.getByTestId("sidebar-activity").click();
  await page.getByTestId("tx-list").waitFor();
  const rows = (await rowTexts(page)).filter(Boolean);
  expect(rows.length).toBeGreaterThan(0);

  await page.getByTestId("toolbar-lock").click();
  const asked = (await calls(page)).length;
  await expectHoldsNothing(page, words, rows);
  // Nothing mounted reads the wallet while locked, whatever the engine signals.
  await page.evaluate(() => window.__dwFixture!.emit({ kind: "balances", network: "testnet", walletId: window.__dwFixture!.world.wallets[0].summary.walletId, balances: null }));
  await page.evaluate(() => new Promise((r) => setTimeout(r, 300)));
  expect((await calls(page)).slice(asked).filter((c) => !LOCKED_CALLS.has(c))).toEqual([]);
  await expectHoldsNothing(page, words, rows);

  await unlock(page);
  await expect(page.getByTestId("tx-list")).toBeVisible();
  await expect(page.getByTestId("sidebar-activity")).toHaveAttribute("aria-current", "page");
  // Focus is not left on the page's body: it lands on the page's title, as a menu leaves it.
  await expect(page.locator(".app-body > main h1")).toBeFocused();
  await expect(page.locator("[data-txid]").first()).toBeVisible();
  expect((await rowTexts(page)).filter(Boolean)).toEqual(rows);
  expect(errors).toEqual([]);
});

/** The engine locks on its own (auto-lock, the tray, another window): its event comes first, and
 * the status read it prompts is held, so the window sees the event and an old status. */
async function engineLocks(page: Page) {
  await page.evaluate(() => {
    const f = window.__dwFixture!;
    f.world.lock = { ...f.world.lock, state: "locked" };
    f.emit({ kind: "vaultLockState", network: f.world.launch.network.dirName, state: "locked" });
  });
}

test("from the lock event on, before the status says locked, the window holds and asks nothing of the wallet", async ({ page }) => {
  const errors = trackPageErrors(page);
  await page.goto(fixtureUrl("locked", "light"));
  await page.getByTestId("lock-field").waitFor();
  const words = await walletWords(page);
  await unlock(page);
  await page.getByTestId("sidebar-activity").click();
  await page.getByTestId("tx-list").waitFor();
  const rows = (await rowTexts(page)).filter(Boolean);

  const release = await page.evaluateHandle(() => window.__dwFixture!.hold("lock_status"));
  const asked = (await calls(page)).length;
  await engineLocks(page);
  await expectHoldsNothing(page, words, rows);
  await page.evaluate(() => {
    const f = window.__dwFixture!;
    f.emit({ kind: "balances", network: f.world.launch.network.dirName, walletId: f.world.wallets[0].summary.walletId, balances: null });
  });
  await page.evaluate(() => new Promise((r) => setTimeout(r, 300)));
  expect((await calls(page)).slice(asked).filter((c) => !LOCKED_CALLS.has(c))).toEqual([]);
  await expectHoldsNothing(page, words, rows);
  await release.evaluate((r) => r());
  await expectHoldsNothing(page, words, rows);

  await unlock(page);
  await expect(page.getByTestId("tx-list")).toBeVisible();
  expect(errors).toEqual([]);
});

test("a payment that arrives between the lock event and the status read is looked up only after the unlock", async ({ page }) => {
  const errors = trackPageErrors(page);
  await page.goto(fixtureUrl("locked", "light"));
  await unlock(page);
  await expect(page.getByTestId("hero")).toContainText(BALANCE);

  const release = await page.evaluateHandle(() => window.__dwFixture!.hold("lock_status"));
  const asked = (await calls(page)).length;
  await engineLocks(page);
  await page.evaluate(() => {
    const f = window.__dwFixture!;
    f.emit({ kind: "newTransactions", network: f.world.launch.network.dirName, walletId: f.world.wallets[0].summary.walletId, txids: [f.world.wallets[0].rows[0].txid], count: 1, catchUp: false });
  });
  await page.evaluate(() => new Promise((r) => setTimeout(r, 300)));
  const meanwhile = (await calls(page)).slice(asked);
  expect(meanwhile).not.toContain("tx_notices");
  expect(meanwhile.filter((c) => !LOCKED_CALLS.has(c))).toEqual([]);
  await release.evaluate((r) => r());

  await unlock(page);
  await expect.poll(async () => (await calls(page)).slice(asked).filter((c) => c === "tx_notices")).toEqual(["tx_notices"]);
  expect(errors).toEqual([]);
});

/** Pay ▸ To an address, filled by a payment link and confirmed with the passphrase, its broadcast
 * held: returns its txid and the release. */
async function payHeld(page: Page) {
  await page.evaluate((address) => window.__dwFixture!.deliverLinks([{ kind: "pay", uri: { address, amount: 50_000_000, label: null, message: null }, username: null }]), PAYEE);
  await expect(page.getByTestId("pay-address")).toHaveValue(PAYEE);
  await expect(page.getByTestId("pay-amount")).toHaveValue("0.50000000");
  await expect(page.getByTestId("pay-continue")).toBeEnabled();
  await page.getByTestId("pay-continue").click();
  await page.getByTestId("pay-confirm-passphrase").fill(FIXTURE_PASSPHRASE);
  const release = await page.evaluateHandle(() => window.__dwFixture!.hold("draft_broadcast"));
  await page.getByTestId("pay-confirm-send").click();
  await expect.poll(async () => (await calls(page)).includes("draft_broadcast")).toBe(true);
  const txid = await world(page, (w) => Object.values(w.send.prepared).at(-1)?.summary.txid ?? w.wallets[0].rows[0].txid);
  return { txid, release };
}

/** The link's payment is not offered again as unsent: not under the result, not once it closes,
 * not on Pay opened again. */
async function expectNotResubmittable(page: Page) {
  await expect(page.getByTestId("pay-address")).toHaveValue("");
  await expect(page.getByTestId("pay-amount")).toHaveValue("");
  await page.getByTestId("pay-result-close").click();
  await expect(page.getByTestId("pay-result")).toHaveCount(0);
  await expect(page.getByTestId("pay-address")).toHaveValue("");
  await expect(page.getByTestId("pay-continue")).toBeDisabled();
  await page.getByTestId("sidebar-home").click();
  await page.getByTestId("sidebar-pay").click();
  await page.getByTestId("pay-tabs-address").click();
  await expect(page.getByTestId("pay-address")).toHaveValue("");
  await expect(page.getByTestId("pay-amount")).toHaveValue("");
}

for (const answer of ["sent", "unknown"] as const)
  test(`a lock during a payment's broadcast keeps its result (${answer}) for the unlock, and the form cannot pay it again`, async ({ page }) => {
    const errors = trackPageErrors(page);
    await page.goto(fixtureUrl("locked", "light"));
    await unlock(page);
    if (answer === "unknown") await world(page, (w) => (w.send.broadcast = "unknown"));
    const { txid, release } = await payHeld(page);
    await page.evaluate(() => window.__dwFixture!.shellCommand({ kind: "lock" }));
    await expect(page.getByTestId("lock-field")).toBeVisible();
    await expect(page.locator(".app-chrome")).toHaveCount(0);
    await release.evaluate((r) => r());
    await page.evaluate(() => new Promise((r) => setTimeout(r, 300)));

    await unlock(page);
    await expect(page.getByTestId("page-pay")).toBeVisible();
    await expect(page.getByTestId(`pay-result-${answer}`)).toBeVisible();
    await expect(page.getByTestId("pay-result-txid")).toContainText(txid);
    if (answer === "unknown") {
      await expect(page.getByTestId("pay-result-text")).toContainText("Check Activity before you pay again.");
      await expect(page.getByTestId("pay-result-retry")).toHaveCount(0);
    }
    await expectNotResubmittable(page);
    expect(errors).toEqual([]);
  });

test("an unlock before the broadcast answers shows the payment being sent, then its result", async ({ page }) => {
  const errors = trackPageErrors(page);
  await page.goto(fixtureUrl("locked", "light"));
  await unlock(page);
  const { txid, release } = await payHeld(page);
  await page.evaluate(() => window.__dwFixture!.shellCommand({ kind: "lock" }));
  await expect(page.getByTestId("lock-field")).toBeVisible();

  await unlock(page);
  await expect(page.getByTestId("pay-sending")).toBeVisible();
  await expect(page.getByTestId("pay-sending")).toBeFocused();
  await expect(page.getByTestId("pay-sending")).toContainText("0.50000000");
  await expect(page.getByTestId("pay-address")).toHaveValue("");
  await release.evaluate((r) => r());
  await expect(page.getByTestId("pay-result-sent")).toBeVisible();
  await expect(page.getByTestId("pay-sending")).toHaveCount(0);
  await expect(page.getByTestId("pay-result-txid")).toContainText(txid);
  await expectNotResubmittable(page);
  expect(errors).toEqual([]);
});

test("a Try again running as the vault locks keeps the result busy across the unlock, then shows its answer, with no retry of a closed draft", async ({ page }) => {
  const errors = trackPageErrors(page);
  await page.goto(fixtureUrl("locked", "light"));
  await unlock(page);
  await world(page, (w) => (w.send.broadcast = "unknown"));
  const { release: first } = await payHeld(page);
  await first.evaluate((r) => r());
  await expect(page.getByTestId("pay-result-unknown")).toBeVisible();
  await world(page, (w) => (w.send.broadcast = "ok"));
  const release = await page.evaluateHandle(() => window.__dwFixture!.hold("draft_broadcast"));
  const asked = (await calls(page)).length;
  await page.getByTestId("pay-result-retry").click();
  await expect.poll(async () => (await calls(page)).slice(asked).includes("draft_broadcast")).toBe(true);
  await page.evaluate(() => window.__dwFixture!.shellCommand({ kind: "lock" }));
  await expect(page.getByTestId("lock-field")).toBeVisible();

  await unlock(page);
  await expect(page.getByTestId("pay-result-unknown")).toBeVisible();
  await expect(page.getByTestId("pay-result-close")).toBeDisabled();
  await expect(page.getByTestId("pay-result-retry")).toHaveCount(0);
  await release.evaluate((r) => r());
  await expect(page.getByTestId("pay-result-sent")).toBeVisible();
  await expectNotResubmittable(page);
  expect(errors).toEqual([]);
});

test("an export the lock caught while it wrote the file says where it went after the unlock", async ({ page }) => {
  const errors = trackPageErrors(page);
  await page.goto(fixtureUrl("locked", "light"));
  await unlock(page);
  await page.evaluate(() => window.__dwFixture!.shellCommand({ kind: "navigate", route: "settings" }));
  await page.getByTestId("settings-tabs-backups").click();
  await page.getByTestId("settings-export").click();
  await page.getByTestId("settings-export-descriptors").click();
  await page.getByTestId("settings-export-passphrase-field").fill(FIXTURE_PASSPHRASE);
  const release = await page.evaluateHandle(() => window.__dwFixture!.hold("export_for_core"));
  await page.getByTestId("settings-export-passphrase-submit").click();
  await expect.poll(async () => (await calls(page)).includes("export_for_core")).toBe(true);
  await page.evaluate(() => window.__dwFixture!.shellCommand({ kind: "lock" }));
  await expect(page.getByTestId("lock-field")).toBeVisible();
  await expect(page.getByTestId("settings-export-sheet")).toHaveCount(0);
  await release.evaluate((r) => r());
  await page.evaluate(() => new Promise((r) => setTimeout(r, 300)));
  await expect(page.getByTestId("settings-export-sheet")).toHaveCount(0);

  await unlock(page);
  await expect(page.getByTestId("settings-export-done")).toBeVisible();
  await expect(page.getByTestId("settings-export-path")).toContainText("-descriptors.json");
  expect(errors).toEqual([]);
});

test("a payment's lookup that answers after the vault locked again is read again at the next unlock", async ({ page }) => {
  const errors = trackPageErrors(page);
  await page.goto(fixtureUrl("locked", "light"));
  await page.getByTestId("lock-field").waitFor();
  const words = await walletWords(page);
  await unlock(page);
  await page.evaluate(() => window.__dwFixture!.shellCommand({ kind: "lock" }));
  await page.getByTestId("lock-field").waitFor();
  const paidTo = await page.evaluate(() => window.__dwFixture!.receivePayment(123_456_789));
  const release = await page.evaluateHandle(() => window.__dwFixture!.hold("tx_notices"));
  await unlock(page);
  await expect.poll(async () => (await calls(page)).filter((c) => c === "tx_notices")).toEqual(["tx_notices"]);
  await page.evaluate(() => window.__dwFixture!.shellCommand({ kind: "lock" }));
  await page.getByTestId("lock-field").waitFor();
  await release.evaluate((r) => r());
  await page.evaluate(() => new Promise((r) => setTimeout(r, 300)));
  await expectHoldsNothing(page, words, ["1.23456789", paidTo]);

  await unlock(page);
  await expect(page.getByTestId("toolbar-bell-badge")).toHaveText("1");
  await page.getByTestId("toolbar-bell").click();
  await expect(page.getByTestId("bell-payment")).toHaveCount(1);
  await expect(page.getByTestId("bell-payment")).toContainText("1.23");
  expect(errors).toEqual([]);
});

/** Masternodes ▸ Governance ▸ Create a proposal, up to its review with the passphrase entered.
 * `to`: an address typed in, rather than a new one of the wallet's. */
async function fillProposal(page: Page, to?: string) {
  await page.getByTestId("sidebar-masternodes").click();
  await page.getByTestId("governance-create").click();
  await page.getByTestId("create-name").fill("desktop-translations");
  await page.getByTestId("create-url").fill("https://www.dashcentral.org/p/desktop-translations");
  await page.getByTestId("create-continue").click();
  await page.getByTestId("create-amount").fill("12.5");
  if (to) {
    await page.getByTestId("create-pay-to").selectOption("other");
    await page.getByTestId("create-address").fill(to);
  }
  await page.getByTestId("create-continue").click();
  await page.getByTestId("create-passphrase").fill(FIXTURE_PASSPHRASE);
}

const createCalls = async (page: Page) => (await calls(page)).filter((c) => c === "governance_create_proposal").length;

/** As `fillProposal`, then Pay pressed, its `create_proposal` held. */
async function payProposalHeld(page: Page, to?: string) {
  await fillProposal(page, to);
  const paid = await createCalls(page);
  const release = await page.evaluateHandle(() => window.__dwFixture!.hold("governance_create_proposal"));
  await page.getByTestId("create-pay").click();
  await expect.poll(() => createCalls(page)).toBe(paid + 1);
  return release;
}

/** From now on Mine's reads leave out the proposals of the draft, as an engine that has not
 * shown a stored proposal yet; `showProposals` ends it. */
const hideProposals = (page: Page) =>
  page.evaluate(() => {
    type Row = { name: string };
    const ipc = (window as unknown as { __TAURI_INTERNALS__: { invoke: (cmd: string, ...rest: unknown[]) => Promise<unknown> } }).__TAURI_INTERNALS__;
    const invoke = ipc.invoke.bind(ipc);
    const w = window as unknown as { hiding: boolean };
    w.hiding = true;
    ipc.invoke = async (cmd, ...rest) => {
      const answer = await invoke(cmd, ...rest);
      return cmd === "governance_my_proposals" && w.hiding ? (answer as Row[]).filter((p) => p.name !== "desktop-translations") : answer;
    };
  });

/** The proposal the last call made takes a hash of its own (the engine's covers the time it was
 * made, the fixture's only the draft), so a second call for the draft can store another. */
const rehashNewestProposal = (page: Page) =>
  page.evaluate((w) => Object.assign(window.__dwFixture!.world.governance.mine[w][0], { hash: "e1".repeat(32), feeTxid: "f1".repeat(32) }), FIXTURE_WALLET_ID);

/** Mine's reads show every proposal again, and are read again. */
const showProposals = (page: Page) =>
  page.evaluate(() => {
    (window as unknown as { hiding: boolean }).hiding = false;
    const f = window.__dwFixture!;
    f.emit({ kind: "governance", network: f.world.launch.network.dirName });
  });

/** A fee that went out, its call failing while locked and its proposal not in Mine yet: after the
 * unlock the notice says Mine does not show it, and is dismissed. */
async function paidNotShown(page: Page, to: string) {
  await page.clock.setFixedTime(FIXTURE_NOW);
  await page.goto(fixtureUrl("governance", "light"));
  await page.getByTestId("toolbar").waitFor();
  await world(page, (w) => (w.governance.createFailsAfterPaying = true));
  await hideProposals(page);
  const first = await payProposalHeld(page, to);
  await lockWindow(page);
  await rehashNewestProposal(page);
  await first.evaluate((r) => r());
  await page.evaluate(() => new Promise((r) => setTimeout(r, 300)));
  await unlock(page);
  await expect(keptText(page)).toHaveAttribute("data-check", "absent");
  await page.getByTestId("create-kept-dismiss").click();
  await expect(page.getByTestId("governance-create")).toBeFocused();
}

const lockWindow = async (page: Page) => {
  await page.evaluate(() => window.__dwFixture!.shellCommand({ kind: "lock" }));
  await expect(page.getByTestId("lock-field")).toBeVisible();
};

/** Masternodes after the unlock, its proposal's call answered while locked: `answer` "stored" made
 * the proposal and then failed, "none" failed with nothing made. */
async function proposalUnsure(page: Page, answer: "stored" | "none", mine: "readable" | "unreadable" = "readable") {
  await page.clock.setFixedTime(FIXTURE_NOW);
  await page.goto(fixtureUrl("governance", "light"));
  await page.getByTestId("toolbar").waitFor();
  if (answer === "stored") await world(page, (w) => (w.governance.createFailsAfterPaying = true));
  else await page.evaluate(() => window.__dwFixture!.fail("governance_create_proposal", "none"));
  const release = await payProposalHeld(page);
  await page.evaluate(() => window.__dwFixture!.shellCommand({ kind: "lock" }));
  await expect(page.getByTestId("lock-field")).toBeVisible();
  if (mine === "unreadable") await page.evaluate(() => window.__dwFixture!.fail("governance_my_proposals", "none"));
  await release.evaluate((r) => r());
  await page.evaluate(() => new Promise((r) => setTimeout(r, 300)));
  await unlock(page);
  await expect(page.getByTestId("page-masternodes")).toBeVisible();
}

const keptText = (page: Page) => page.getByTestId("create-kept-text");

test("a proposal's fee paid as the vault locks ends as made once Mine shows it, though its call never answers", async ({ page }) => {
  const errors = trackPageErrors(page);
  await page.clock.setFixedTime(FIXTURE_NOW);
  await page.goto(fixtureUrl("governance", "light"));
  await page.getByTestId("toolbar").waitFor();
  const release = await payProposalHeld(page);
  await page.evaluate(() => window.__dwFixture!.shellCommand({ kind: "lock" }));
  await expect(page.getByTestId("lock-field")).toBeVisible();
  await unlock(page);
  await expect(page.getByTestId("mine-chip-desktop-translations")).toBeVisible();
  await expect(page.getByTestId("create-kept-paying")).toHaveCount(0);
  await expect(page.getByTestId("governance-create")).toBeEnabled();
  await release.evaluate((r) => r());
  await page.evaluate(() => new Promise((r) => setTimeout(r, 300)));
  await expect(page.getByTestId("create-kept-paying")).toHaveCount(0);
  await expect(page.getByTestId("create-kept-unsure")).toHaveCount(0);
  await expect(page.getByTestId("governance-create")).toBeEnabled();
  expect(errors).toEqual([]);
});

test("a proposal's fee still being paid after the unlock holds Create back until its call answers", async ({ page }) => {
  const errors = trackPageErrors(page);
  await page.clock.setFixedTime(FIXTURE_NOW);
  await page.goto(fixtureUrl("governance", "light"));
  await page.getByTestId("toolbar").waitFor();
  await page.evaluate(() => window.__dwFixture!.fail("governance_create_proposal", "none"));
  const release = await payProposalHeld(page);
  await page.evaluate(() => window.__dwFixture!.shellCommand({ kind: "lock" }));
  await unlock(page);
  await expect(page.getByTestId("create-kept-paying")).toBeVisible();
  await expect(page.getByTestId("governance-create")).toBeDisabled();
  await release.evaluate((r) => r());
  await expect(keptText(page)).toHaveAttribute("data-check", "absent");
  await expect(page.getByTestId("governance-create")).toBeEnabled();
  expect(errors).toEqual([]);
});

test("unsure after the lock, with the proposal in Mine: Create stays off and the notice points to it until dismissed", async ({ page }) => {
  const errors = trackPageErrors(page);
  await proposalUnsure(page, "stored");
  await expect(keptText(page)).toHaveAttribute("data-check", "found");
  await expect(keptText(page)).toContainText("desktop-translations");
  await expect(keptText(page)).toContainText("Mine");
  await expect(keptText(page)).toContainText("Activity");
  await expect(page.getByTestId("governance-create")).toBeDisabled();
  await expect(page.getByTestId("governance-create")).toHaveAttribute("title", /dismiss the notice/);
  await page.getByTestId("create-kept-show-mine").click();
  await expect(page.getByTestId("mine-chip-desktop-translations")).toBeVisible();
  await expect(page.getByTestId("create-kept-unsure")).toBeVisible();
  await page.getByTestId("create-kept-dismiss").click();
  await expect(page.getByTestId("create-kept-unsure")).toHaveCount(0);
  await expect(page.getByTestId("governance-create")).toBeEnabled();
  await expect(page.getByTestId("governance-create")).toBeFocused();
  expect(errors).toEqual([]);
});

test("unsure after the lock, with no proposal in Mine: Create comes back after the read and the notice names both places", async ({ page }) => {
  const errors = trackPageErrors(page);
  await proposalUnsure(page, "none");
  await expect(keptText(page)).toHaveAttribute("data-check", "absent");
  await expect(keptText(page)).toContainText("check Mine, and the fee payment in Activity, before you pay again");
  await expect(page.getByTestId("governance-create")).toBeEnabled();
  await expect(page.getByTestId("create-kept-unsure")).toBeVisible();
  await page.getByTestId("create-kept-dismiss").click();
  await expect(page.getByTestId("create-kept-unsure")).toHaveCount(0);
  await expect(page.getByTestId("governance-create")).toBeFocused();
  expect(errors).toEqual([]);
});

test("unsure after the lock, Mine unreadable: Create stays off, the notice offers to read again, and the read decides", async ({ page }) => {
  const errors = trackPageErrors(page);
  await proposalUnsure(page, "none", "unreadable");
  await expect(keptText(page)).toHaveAttribute("data-check", "failed");
  await expect(page.getByTestId("governance-create")).toBeDisabled();
  await expect(page.getByTestId("create-kept-dismiss")).toHaveCount(0);
  await page.getByTestId("create-kept-read-again").click();
  await expect(keptText(page)).toHaveAttribute("data-check", "failed");
  await expect(page.getByTestId("governance-create")).toBeDisabled();
  await page.evaluate(() => window.__dwFixture!.fail("governance_my_proposals", null));
  await page.getByTestId("create-kept-read-again").click();
  await expect(keptText(page)).toHaveAttribute("data-check", "absent");
  await expect(page.getByTestId("governance-create")).toBeEnabled();
  expect(errors).toEqual([]);
});

test("a fee that went out with its proposal not in Mine yet: the next Pay for the draft reads Mine first and pays nothing once it shows", async ({ page }) => {
  const errors = trackPageErrors(page);
  const to = "yXdjeVZqmWp4ZcVJ2Hh8hcFhUzFE8wD1Cd";
  await paidNotShown(page, to);
  // The second sheet is filled in; the first proposal shows in Mine meanwhile.
  await fillProposal(page, to);
  await showProposals(page);
  const paid = await createCalls(page);
  await page.getByTestId("create-pay").click();
  await expect(page.getByTestId("create-error")).toContainText("An earlier payment for desktop-translations may have made it");
  expect(await createCalls(page)).toBe(paid);
  await page.keyboard.press("Escape");
  await expect(keptText(page)).toHaveAttribute("data-check", "found");
  await expect(page.getByTestId("governance-create")).toBeDisabled();
  expect(errors).toEqual([]);
});

test("two payments for one draft that both paid a fee, the first not in Mine when the second paid: neither is credited, and the notice says more than one may have paid", async ({ page }) => {
  const errors = trackPageErrors(page);
  const to = "yXdjeVZqmWp4ZcVJ2Hh8hcFhUzFE8wD1Cd";
  await paidNotShown(page, to);
  // Mine shows the second's proposal at once, and still not the first's.
  await page.evaluate(() => {
    const w = window as unknown as { hiding: boolean };
    w.hiding = false;
  });
  await page.evaluate((w) => {
    const f = window.__dwFixture!;
    const mine = f.world.governance.mine[w];
    (window as unknown as { first: unknown }).first = mine.shift();
  }, FIXTURE_WALLET_ID);
  // The second pays its fee too, and its call fails as well.
  await fillProposal(page, to);
  await page.getByTestId("create-prior-fee").waitFor();
  const paid = await createCalls(page);
  await page.getByTestId("create-pay").click();
  await expect(page.getByTestId("create-twin")).toContainText("more than one attempt may have paid a fee");
  await expect(page.getByTestId("create-pay")).toBeDisabled();
  expect(await createCalls(page)).toBe(paid + 1);
  await page.evaluate((w) => {
    const f = window.__dwFixture!;
    f.world.governance.mine[w].push((window as unknown as { first: (typeof f.world.governance.mine)[string][number] }).first);
    f.emit({ kind: "governance", network: f.world.launch.network.dirName });
  }, FIXTURE_WALLET_ID);
  await page.keyboard.press("Escape");
  await expect(keptText(page)).toHaveAttribute("data-check", "uncertain");
  await expect(page.getByTestId("governance-create")).toBeDisabled();
  await expect(page.getByTestId("governance-create")).toHaveAttribute("title", /More than one attempt may have paid a fee/);
  await page.getByTestId("create-kept-show-mine").click();
  await expect(page.getByTestId("mine-chip-desktop-translations")).toHaveCount(2);
  await page.getByTestId("create-kept-dismiss").click();
  await expect(page.getByTestId("create-kept-unsure")).toHaveCount(0);
  await expect(page.getByTestId("governance-create")).toBeEnabled();
  await expect(page.getByTestId("governance-create")).toBeFocused();
  expect(errors).toEqual([]);
});

test("a call that never answers: the user can go on with a different proposal, and its own draft stays held", async ({ page }) => {
  const errors = trackPageErrors(page);
  await page.clock.setFixedTime(FIXTURE_NOW);
  await page.goto(fixtureUrl("governance", "light"));
  await page.getByTestId("toolbar").waitFor();
  await page.evaluate(() => window.__dwFixture!.fail("governance_create_proposal", "none"));
  const release = await payProposalHeld(page);
  await lockWindow(page);
  await unlock(page);
  await expect(page.getByTestId("create-kept-paying")).toBeVisible();
  await expect(page.getByTestId("governance-create")).toBeDisabled();
  await page.getByTestId("create-kept-go-on").click();
  await expect(page.getByTestId("create-kept-paying")).toContainText("You can create a different proposal");
  await expect(page.getByTestId("governance-create")).toBeEnabled();
  await page.evaluate(() => window.__dwFixture!.fail("governance_create_proposal", null));
  const paid = await createCalls(page);
  await fillProposal(page);
  await page.getByTestId("create-pay").click();
  await expect(page.getByTestId("create-error")).toContainText("An earlier payment for desktop-translations may have made it");
  expect(await createCalls(page)).toBe(paid);
  await release.evaluate((r) => r());
  expect(errors).toEqual([]);
});

test("lost events while in use show the lock screen while the status read never answers, then the same page", async ({ page }) => {
  test.setTimeout(60_000);
  const errors = trackPageErrors(page);
  await page.goto(fixtureUrl("locked", "light"));
  await unlock(page);
  await page.getByTestId("sidebar-activity").click();
  await expect(page.getByTestId("page-activity")).toBeVisible();
  // The confirming read is held and never answers; every other status read fails.
  const release = await page.evaluateHandle(() => {
    const f = window.__dwFixture!;
    const r = f.hold("lock_status");
    f.fail("lock_status", "none");
    f.emit({ kind: "eventsOverflowed" });
    return r;
  });
  await expect(page.getByTestId("lock-screen")).toBeVisible();
  await expect(page.getByTestId("page-activity")).toHaveCount(0);
  expect(await page.evaluate(() => document.body.innerText)).not.toContain(BALANCE);
  // Past the read's deadline and the wait after it: still closed.
  await page.evaluate(() => new Promise((r) => setTimeout(r, 12_000)));
  await expect(page.getByTestId("lock-screen")).toBeVisible();
  await page.evaluate(() => window.__dwFixture!.fail("lock_status", null));
  await expect(page.getByTestId("page-activity")).toBeVisible({ timeout: 15_000 });
  await expect(page.getByTestId("lock-screen")).toHaveCount(0);
  // The held read's late answer counts for nothing.
  await release.evaluate((r) => r());
  await page.evaluate(() => new Promise((r) => setTimeout(r, 300)));
  await expect(page.getByTestId("page-activity")).toBeVisible();
  expect(errors).toEqual([]);
});

test("Home after the unlock has its hidden title focused, with no ring", async ({ page }) => {
  await page.goto(fixtureUrl("locked", "light"));
  await unlock(page);
  await expect(page.getByTestId("hero")).toBeVisible();
  const title = page.locator(".app-body > main h1");
  await expect(title).toHaveText("Home");
  await expect(title).toBeFocused();
  expect(await title.evaluate((e) => getComputedStyle(e).outlineStyle)).toBe("none");
  expect(await page.getByTestId("page-home").evaluate((e) => getComputedStyle(e).outlineStyle)).toBe("none");
});
