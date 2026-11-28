import { expect, test, type Page } from "@playwright/test";
import { FIXTURE_PASSPHRASE, type World } from "../src/fixture/scenarios";
import { FIXTURE_NOW } from "../src/fixture/wallet";
import { fixtureUrl, trackPageErrors } from "./helpers";
import { PAYEE } from "./wallet";

// A locked window holds nothing of the wallet: the lock screen stands in place of the pages, not
// over them, so the page's text and markup carry no balance, wallet name, history row, contact or
// address until the unlock, and the renderer reads none of them meanwhile.

const BALANCE = "31.84698287";

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
    f.emit({ kind: "newTransactions", network: f.world.launch.network.dirName, walletId: f.world.wallets[0].summary.walletId, txids: [f.world.wallets[0].rows[0].txid], live: [true], count: 1, catchUp: false });
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

const SEED = "0f".repeat(32);
/** What the shell was last told: whether a view that shows or takes a secret is open. */
const secretShown = (page: Page) => world(page, (w) => w.shellSync?.secretShown ?? false);

/** Settings ▸ Backups ▸ Import keys after the unlock, an HD seed typed in, the window protected. */
async function typeKey(page: Page) {
  await page.goto(fixtureUrl("locked", "light"));
  await unlock(page);
  await page.evaluate(() => window.__dwFixture!.shellCommand({ kind: "navigate", route: "settings" }));
  await page.getByTestId("settings-tabs-backups").click();
  await page.getByTestId("settings-import-keys").click();
  await page.getByTestId("settings-keys-hdSeed").click();
  await page.getByTestId("settings-keys-text").fill(SEED);
  await expect.poll(() => secretShown(page)).toBe(true);
}

test("Import keys with a key typed and not sent goes with the lock, and the shell is told no secret is shown", async ({ page }) => {
  const errors = trackPageErrors(page);
  await typeKey(page);
  await page.evaluate(() => window.__dwFixture!.shellCommand({ kind: "lock" }));
  await expect(page.getByTestId("lock-field")).toBeVisible();
  await expect(page.getByTestId("settings-import-keys-sheet")).toHaveCount(0);
  await expect.poll(() => secretShown(page)).toBe(false);
  expect(await page.evaluate(() => [...document.querySelectorAll("textarea, input")].map((e) => (e as HTMLInputElement).value))).not.toContain(SEED);
  await unlock(page);
  await expect(page.getByTestId("settings-import-keys")).toBeVisible();
  await expect(page.getByTestId("settings-import-keys-sheet")).toHaveCount(0);
  expect(await secretShown(page)).toBe(false);
  expect(errors).toEqual([]);
});

for (const gate of ["pending", "refused"] as const) {
  test(`Import keys caught working by the lock opens again behind the capture-protection gate (${gate}), and the shell is told again`, async ({ page }) => {
    const errors = trackPageErrors(page);
    await typeKey(page);
    const release = await page.evaluateHandle(() => window.__dwFixture!.hold("import_key_material"));
    await page.getByTestId("settings-keys-import").click();
    await expect.poll(async () => (await calls(page)).includes("import_key_material")).toBe(true);
    await page.evaluate(() => window.__dwFixture!.shellCommand({ kind: "lock" }));
    await expect(page.getByTestId("lock-field")).toBeVisible();
    await expect(page.getByTestId("settings-import-keys-sheet")).toHaveCount(0);
    await expect.poll(() => secretShown(page)).toBe(false);
    await release.evaluate((r) => r());
    await page.evaluate(() => new Promise((r) => setTimeout(r, 300)));
    // The shell's answer is held, or refused, from the unlock on.
    const shell = gate === "pending" ? await page.evaluateHandle(() => window.__dwFixture!.hold("shell_sync")) : null;
    if (gate === "refused") await page.evaluate(() => window.__dwFixture!.fail("shell_sync", "none"));
    await unlock(page);
    await expect(page.getByTestId("settings-import-keys-sheet")).toBeVisible();
    await expect(page.getByTestId(gate === "pending" ? "protection-pending" : "protection-failed")).toBeVisible();
    await expect(page.getByTestId("settings-import-done-0")).toHaveCount(0);
    await expect(page.getByTestId("settings-import-close")).toHaveCount(0);
    if (shell) await shell.evaluate((r) => r());
    else {
      await page.evaluate(() => window.__dwFixture!.fail("shell_sync", null));
      await page.getByTestId("protection-retry").click();
    }
    await expect(page.getByTestId("settings-import-done-0")).toBeVisible();
    await expect.poll(() => secretShown(page)).toBe(true);
    expect(errors).toEqual([]);
  });
}

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

/** Masternodes ▸ Governance ▸ Create a proposal named `name`, up to its review with the
 * passphrase entered. */
async function fillProposal(page: Page, name: string) {
  await page.getByTestId("sidebar-masternodes").click();
  await page.getByTestId("governance-create").click();
  await page.getByTestId("create-name").fill(name);
  await page.getByTestId("create-url").fill(`https://www.dashcentral.org/p/${name}`);
  await page.getByTestId("create-continue").click();
  await page.getByTestId("create-amount").fill("12.5");
  await page.getByTestId("create-continue").click();
  await expect(page.getByTestId("create-fee-warning")).toBeVisible();
  await page.getByTestId("create-passphrase").fill(FIXTURE_PASSPHRASE);
}

const lockWindow = async (page: Page) => {
  await page.evaluate(() => window.__dwFixture!.shellCommand({ kind: "lock" }));
  await expect(page.getByTestId("lock-field")).toBeVisible();
};

test("a lock takes the proposal sheet down: nothing of its draft, its review, its passphrase or Mine's rows is left", async ({ page }) => {
  const errors = trackPageErrors(page);
  await page.clock.setFixedTime(FIXTURE_NOW);
  await page.goto(fixtureUrl("governance", "light"));
  await page.getByTestId("toolbar").waitFor();
  await fillProposal(page, "desktop-translations");
  await page.getByTestId("create-pay").click();
  await expect(page.getByTestId("mine-chip-desktop-translations")).toBeVisible();
  const made = await world(page, (w) => Object.values(w.governance.mine).flat()[0]);
  await fillProposal(page, "desktop-docs");
  const review = await page.getByTestId("create-sheet").innerText();
  expect(review).toContain("desktop-docs");
  await lockWindow(page);
  await expect(page.getByTestId("create-sheet")).toHaveCount(0);
  await expectHoldsNothing(page, ["desktop-translations", "desktop-docs", "dashcentral", FIXTURE_PASSPHRASE], [made.hash, made.feeTxid, made.paymentAddress]);
  await unlock(page);
  await expect(page.getByTestId("create-sheet")).toHaveCount(0);
  await page.getByTestId("governance-create").click();
  await expect(page.getByTestId("create-name")).toHaveValue("");
  expect(errors).toEqual([]);
});

const since = async (page: Page, n: number) => (await calls(page)).slice(n);
const leavePage = async (page: Page) => {
  // The shell is inert under the sheet: the menu's Home leaves the page.
  await page.evaluate(() => window.__dwFixture!.shellCommand({ kind: "navigate", route: "home" }));
  await expect(page.getByTestId("create-sheet")).toHaveCount(0);
};

// Nothing a departed sheet started may pay: an await between Pay and the fee's call that answers
// once the sheet is gone (its page left by the menu, or the vault locked, and unlocked again before
// the answer) ends there, and a grant it was given is revoked.
for (const held of ["governance_preview_proposal", "authorize"] as const) {
  for (const gone of ["left", "locked", "locked and unlocked"] as const) {
    test(`Pay's ${held === "authorize" ? "grant" : "fee re-read"} answering after the sheet was ${gone} pays nothing${held === "authorize" ? ", and the grant is revoked" : ""}`, async ({ page }) => {
      const errors = trackPageErrors(page);
      await page.clock.setFixedTime(FIXTURE_NOW);
      await page.goto(fixtureUrl("governance", "light"));
      await page.getByTestId("toolbar").waitFor();
      await fillProposal(page, "desktop-translations");
      const n = (await calls(page)).length;
      const proposals = () => world(page, (w) => Object.values(w.governance.mine).flat().length);
      const before = await proposals();
      const release = await page.evaluateHandle((c) => window.__dwFixture!.hold(c), held);
      await page.getByTestId("create-pay").click();
      await expect.poll(() => since(page, n)).toContain(held);
      if (gone === "left") await leavePage(page);
      else await lockWindow(page);
      if (gone === "locked and unlocked") await unlock(page);
      await release.evaluate((r) => r());
      if (held === "authorize") {
        await expect.poll(() => since(page, n)).toContain("revoke_grant");
        if (gone === "left") expect(await world(page, (w) => w.grants.map((g) => g.used))).toEqual([true]);
      } else {
        await page.evaluate(() => new Promise((r) => setTimeout(r, 300)));
        expect(await since(page, n)).not.toContain("authorize");
      }
      expect(await since(page, n)).not.toContain("governance_create_proposal");
      expect(await proposals()).toBe(before);
      expect(errors).toEqual([]);
    });
  }
}

test("Pay's grant answering after the review's draft moved (its first superblock passed) pays nothing, and the grant is revoked", async ({ page }) => {
  const errors = trackPageErrors(page);
  await page.clock.setFixedTime(FIXTURE_NOW);
  await page.goto(fixtureUrl("governance", "light"));
  await page.getByTestId("toolbar").waitFor();
  await fillProposal(page, "desktop-translations");
  const n = (await calls(page)).length;
  const release = await page.evaluateHandle(() => window.__dwFixture!.hold("authorize"));
  await page.getByTestId("create-pay").click();
  await expect.poll(() => since(page, n)).toContain("authorize");
  await page.evaluate(() => {
    const f = window.__dwFixture!;
    f.world.governance.superblocksPassed = 2;
    f.emit({ kind: "governance", network: f.world.launch.network.dirName });
  });
  await expect(page.getByTestId("create-first-moved")).toBeVisible();
  await release.evaluate((r) => r());
  await expect.poll(() => since(page, n)).toContain("revoke_grant");
  expect(await world(page, (w) => w.grants.map((g) => g.used))).toEqual([true]);
  expect(await since(page, n)).not.toContain("governance_create_proposal");
  expect(errors).toEqual([]);
});

// A fee call already handed to the engine is the page's to report, wherever the sheet went: its
// toast shows once the window is open, and nothing while locked.
for (const gone of ["left", "locked and unlocked", "locked"] as const) {
  test(`a fee call that succeeds after the sheet was ${gone} ${gone === "locked" ? "shows nothing while locked, and Mine has it after the unlock" : "says the fee was paid"}`, async ({ page }) => {
    const errors = trackPageErrors(page);
    await page.clock.setFixedTime(FIXTURE_NOW);
    await page.goto(fixtureUrl("governance", "light"));
    await page.getByTestId("toolbar").waitFor();
    await fillProposal(page, "desktop-translations");
    const release = await page.evaluateHandle(() => window.__dwFixture!.hold("governance_create_proposal"));
    await page.getByTestId("create-pay").click();
    await expect.poll(async () => (await calls(page)).includes("governance_create_proposal")).toBe(true);
    const toast = page.locator(".toast", { hasText: "desktop-translations: fee paid" });
    if (gone === "left") await leavePage(page);
    else await lockWindow(page);
    if (gone === "locked and unlocked") await unlock(page);
    await release.evaluate((r) => r());
    if (gone === "locked") {
      await page.evaluate(() => new Promise((r) => setTimeout(r, 300)));
      await expect(page.locator(".toast")).toHaveCount(0);
      await unlock(page);
      await page.getByTestId("governance-filter-mine").click();
      await expect(page.getByTestId("mine-chip-desktop-translations")).toBeVisible();
      await expect(page.locator(".toast")).toHaveCount(0);
    } else {
      await expect(toast).toBeVisible();
    }
    expect(errors).toEqual([]);
  });
}

// A refusal (the name in progress) or a failure that answers once the sheet has gone: the sheet
// reads nothing more and sets nothing; its page is not there to show it.
for (const answer of ["refused", "failed"] as const) {
  test(`a fee call ${answer} after the sheet was left reads and shows nothing more`, async ({ page }) => {
    const errors = trackPageErrors(page);
    await page.clock.setFixedTime(FIXTURE_NOW);
    await page.goto(fixtureUrl("governance", "light"));
    await page.getByTestId("toolbar").waitFor();
    if (answer === "refused") {
      await fillProposal(page, "desktop-translations");
      await page.getByTestId("create-pay").click();
      await expect(page.getByTestId("create-sheet")).toHaveCount(0);
    } else {
      await page.evaluate(() => window.__dwFixture!.fail("governance_create_proposal", "none"));
    }
    await fillProposal(page, "desktop-translations");
    const release = await page.evaluateHandle(() => window.__dwFixture!.hold("governance_create_proposal"));
    const n = (await calls(page)).length;
    await page.getByTestId("create-pay").click();
    await expect.poll(() => since(page, n)).toContain("governance_create_proposal");
    await leavePage(page);
    const m = (await calls(page)).length;
    await release.evaluate((r) => r());
    await page.evaluate(() => new Promise((r) => setTimeout(r, 500)));
    expect(await since(page, m)).not.toContain("governance_my_proposals");
    expect(errors).toEqual([]);
  });
}

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
