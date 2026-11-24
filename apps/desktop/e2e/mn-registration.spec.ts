import { expect, test, type Page } from "@playwright/test";
import {
  DUP_OPERATOR_KEY,
  DUP_SERVICE,
  EXTERNAL_ADDRESS,
  EXTERNAL_OUTPOINT,
  EXTERNAL_SIGNATURE,
  HOME,
  keyInfo,
  OP_CONFIRMED,
  OP_CONFLICTED,
  OP_DISSOLVE,
  OP_INPUTS_SPENT,
  OP_NEEDS_SIGNATURE,
  OP_NOT_SENT,
  OP_PREPARED,
  OP_SENT,
  OP_SENT_OVERDUE,
  OP_SIGNED,
  OP_UPDATE,
  REG_COIN,
  REG_COIN_LOCKED,
} from "../src/fixture/masternodes";
import { FIXTURE_NOW } from "../src/fixture/wallet";
import { seriousAxeViolations } from "./axe";
import { fixtureUrl, fontsReady, trackPageErrors } from "./helpers";

// MG-10 S2: the registration wizard (hosted and self-run, regular and evonode, FundNew,
// ExistingUtxo and External), the D5 re-read, field errors and refusals by step, the In progress
// group's states, and Release collateral / Lock it again, on the fixture.

async function open(page: Page, scenario: string, theme: "light" | "dark" = "light", network?: "mainnet", extra = "") {
  const errors = trackPageErrors(page);
  await page.clock.setFixedTime(FIXTURE_NOW);
  await page.goto(fixtureUrl(scenario, theme, (network ? `&network=${network}` : "") + extra));
  await page.getByTestId("toolbar").waitFor();
  await page.getByTestId("sidebar-masternodes").click();
  await expect(page.getByTestId("page-masternodes")).toBeVisible();
  return errors;
}

const sheet = (page: Page) => page.getByTestId("reg-sheet");
const step = (page: Page, name: string) => page.getByTestId(`reg-step-${name}`);
const next = (page: Page) => page.getByTestId("reg-continue").click();

/** An operator's BLS public key no masternode of the fixture uses. */
const OPERATOR_KEY = "ab".repeat(48);
const SERVICE = "203.0.113.20:19999";

async function setUp(page: Page) {
  await page.getByTestId("mn-set-up").click();
  await expect(step(page, "type")).toBeVisible();
}

/** Type → Collateral → Operator (hosted: the operator's key and address) → Keys. */
async function hostedToKeys(page: Page, service = SERVICE, operatorKey = OPERATOR_KEY) {
  await next(page);
  await expect(step(page, "collateral")).toBeVisible();
  await next(page);
  await expect(step(page, "operator")).toBeVisible();
  await page.getByTestId("reg-operator-key").fill(operatorKey);
  await page.getByTestId("reg-service").fill(service);
  await next(page);
  await expect(page.getByTestId("reg-owner")).toBeVisible();
}

const SEND_OFF = "You can review this registration, but this version can't send it yet. Nothing has been sent.";

/** Mainnet: Review and Prove end with the send-off line. */
async function expectSendOff(page: Page) {
  await expect(page.getByTestId("reg-send-off")).toHaveText(SEND_OFF);
  for (const gone of ["reg-register", "reg-submit", "reg-passphrase", "reg-error"]) await expect(page.getByTestId(gone)).toHaveCount(0);
}

/** Testnet and the other non-mainnet networks: Review offers Register. The button is enabled once
 * the fee is known; an empty passphrase is refused by the grant, not by disabling it. */
async function expectRegister(page: Page) {
  await expect(page.getByTestId("reg-register")).toBeEnabled();
  await expect(page.getByTestId("reg-passphrase")).toBeVisible();
  await expect(page.getByTestId("reg-send-off")).toHaveCount(0);
}

async function toReview(page: Page) {
  await next(page);
  await expect(page.getByTestId("reg-review")).toBeVisible();
}

test("hosted regular, FundNew: Type → Review, which can register on testnet; 0 serious axe issues", async ({ page }) => {
  const errors = await open(page, "reg");
  await setUp(page);
  await expect(sheet(page)).toContainText("Step 1 of 5");
  await expect(page.getByTestId("reg-kind-regular").locator("input")).toBeChecked();
  await expect(page.getByTestId("reg-runner-hosted").locator("input")).toBeChecked();
  await expect(sheet(page).locator("fieldset, legend")).toHaveCount(0);
  await expect(sheet(page).getByRole("radiogroup", { name: "What are you setting up?" })).toBeVisible();
  await expect(sheet(page).getByRole("radiogroup", { name: "Who runs the server?" })).toBeVisible();
  // Native radios in a role group keep arrow-key selection within their name.
  await page.getByTestId("reg-kind-regular").locator("input").focus();
  await page.keyboard.press("ArrowDown");
  await expect(page.getByTestId("reg-kind-evonode").locator("input")).toBeChecked();
  await page.keyboard.press("ArrowUp");
  await expect(page.getByTestId("reg-kind-regular").locator("input")).toBeChecked();
  await expect(page.getByTestId("reg-runner-hosted").locator("input")).toBeChecked();
  await fontsReady(page);
  expect(await seriousAxeViolations(page)).toEqual([]);
  await next(page);
  await expect(step(page, "collateral")).toContainText("Where is the 1,000 Dash collateral?");
  await expect(page.getByTestId("reg-collateral-fund-new").locator("input")).toBeChecked();
  await expect(sheet(page).getByRole("radiogroup", { name: /^The collateral stays yours\./ })).toBeVisible();
  await next(page);
  await expect(step(page, "operator")).toContainText("Your operator's details");
  await page.getByTestId("reg-operator-key").fill(OPERATOR_KEY);
  await page.getByTestId("reg-service").fill(SERVICE);
  await next(page);
  await expect(page.getByTestId("reg-owner")).toHaveText("This wallet, owner keypair 1");
  for (const name of ["Voting", "Payouts go to"]) await expect(sheet(page).getByRole("radiogroup", { name, exact: true })).toBeVisible();
  await expect(sheet(page).locator("fieldset, legend")).toHaveCount(0);
  await toReview(page);
  const review = page.getByTestId("reg-review");
  await expect(review).toContainText("Masternode, run by your operator");
  await expect(page.getByTestId("reg-review-collateral")).toContainText("sent to a new address in this wallet");
  await expect(review).toContainText(SERVICE);
  await expect(review).toContainText("abababab, from your operator");
  await expect(page.getByTestId("reg-review-fee")).toContainText("0.00000453");
  await expect(sheet(page)).toContainText("The collateral moves to a new address in this wallet; only the network fee leaves it.");
  await fontsReady(page);
  expect(await seriousAxeViolations(page)).toEqual([]);
  await expectRegister(page);
  // Enter before the passphrase asks for it and sends nothing.
  await sheet(page).locator("form").evaluate((f: HTMLFormElement) => f.requestSubmit());
  await expect(page.getByTestId("reg-error")).toHaveText("Enter your passphrase to register.");
  await expect(step(page, "review")).toBeVisible();
  await page.keyboard.press("Escape");
  await page.getByTestId("reg-discard-confirm").click();
  await expect(sheet(page)).toHaveCount(0);
  await expect(page.getByTestId("mn-group-in-progress")).toHaveCount(0);
  expect(errors).toEqual([]);
});

test("self-run evonode with an entered Platform node ID: the intro names only the operator keypair", async ({ page }) => {
  const errors = await open(page, "reg");
  await setUp(page);
  await page.getByTestId("reg-kind-evonode").click();
  await page.getByTestId("reg-runner-self").click();
  await next(page);
  await next(page);
  await expect(step(page, "operator")).toContainText("(operator keypair 3, Platform node keypair 1)");
  await page.getByTestId("reg-node-enter").click();
  await expect(step(page, "operator")).toContainText("its operator key comes from this wallet's masternode keys (keypair 3)");
  await expect(step(page, "operator")).not.toContainText("Platform node keypair");
  expect(errors).toEqual([]);
});

test("self-run evonode: the wallet's operator key and Platform node, to Review", async ({ page }) => {
  const errors = await open(page, "reg");
  await setUp(page);
  await page.getByTestId("reg-kind-evonode").click();
  await page.getByTestId("reg-runner-self").click();
  await next(page);
  await expect(step(page, "collateral")).toContainText("Where is the 4,000 Dash collateral?");
  await next(page);
  await expect(step(page, "operator")).toContainText("Your server's details");
  // The list's masternodes use operator keypairs 1 and 2: the next unused one is the third.
  await expect(step(page, "operator")).toContainText("come from this wallet's masternode keys (operator keypair 3, Platform node keypair 1)");
  await expect(page.getByTestId("reg-operator-wallet-key")).toHaveText(keyInfo("operator", 2).publicKeyHex.slice(0, 8));
  await expect(page.getByTestId("reg-node-wallet")).toHaveText(keyInfo("platform_node", 0).platformNodeId!);
  await expect(page.getByTestId("reg-operator-key")).toHaveCount(0);
  await page.getByTestId("reg-service").fill(SERVICE);
  await page.getByTestId("reg-p2p-port").fill("36656");
  await page.getByTestId("reg-http-port").fill("36656");
  await next(page);
  await expect(page.getByTestId("reg-error-httpPort")).toHaveText("The two Platform ports must differ.");
  await page.getByTestId("reg-http-port").fill("");
  await next(page);
  await expect(page.getByTestId("reg-owner")).toBeVisible();
  await toReview(page);
  const review = page.getByTestId("reg-review");
  await expect(sheet(page).getByRole("heading", { name: "Set up an evonode" })).toBeVisible();
  await expect(review).toContainText("Evonode, run by you");
  await expect(review).toContainText(`${keyInfo("operator", 2).publicKeyHex.slice(0, 8)}, keypair 3 of this wallet`);
  await expect(review).toContainText(`${keyInfo("platform_node", 0).platformNodeId!.slice(0, 8)} · 36656 / 22001`);
  await expect(page.getByTestId("reg-review-fee")).toContainText("0.00000567");
  await expectRegister(page);
  // Sent names both secrets, and Open Masternode keys opens on the operator role.
  await page.getByTestId("reg-passphrase").fill("demo");
  await page.getByTestId("reg-register").click();
  await expect(page.getByTestId("reg-done-body")).toContainText("Copy the operator secret and the Platform node secret from Masternode keys");
  await page.getByTestId("reg-open-keys").click();
  await expect(page.getByTestId("mn-keys")).toBeVisible();
  await expect(page.getByTestId("mn-keys-role-operator")).toHaveAttribute("aria-pressed", "true");
  expect(errors).toEqual([]);
});

test("ExistingUtxo: a locked coin can't be picked, Continue needs a pick, the review names the coin", async ({ page }) => {
  const errors = await open(page, "reg");
  await setUp(page);
  await next(page);
  await page.getByTestId("reg-collateral-existing").click();
  await expect(page.getByTestId(`reg-coin-${REG_COIN}`)).toContainText("42 confirmations");
  await expect(page.getByTestId(`reg-coin-${REG_COIN_LOCKED}`)).toContainText("Locked");
  await expect(page.getByTestId(`reg-coin-${REG_COIN_LOCKED}`).locator("input")).toBeDisabled();
  await next(page);
  await expect(page.getByTestId("reg-error-outpoint")).toHaveText("Choose an output.");
  await page.getByTestId(`reg-coin-${REG_COIN}`).click();
  await next(page);
  await expect(step(page, "operator")).toBeVisible();
  await page.getByTestId("reg-operator-key").fill(OPERATOR_KEY);
  await page.getByTestId("reg-service").fill(SERVICE);
  await next(page);
  await toReview(page);
  await expect(page.getByTestId("reg-review-collateral")).toContainText(`${REG_COIN.slice(0, 8)}:0, 1000.00000000 tDASH in this wallet`);
  await expect(sheet(page)).toContainText("Only the fee leaves this wallet.");
  await expectRegister(page);
  expect(errors).toEqual([]);
});

for (const theme of ["light", "dark"] as const)
  test(`ExistingUtxo ${theme}: while Register waits for the network, the wizard stays on Review (issue 595)`, async ({ page }, info) => {
    const errors = await open(page, "reg", theme);
    await setUp(page);
    await next(page);
    await page.getByTestId("reg-collateral-existing").click();
    await page.getByTestId(`reg-coin-${REG_COIN}`).click();
    await next(page);
    await page.getByTestId("reg-operator-key").fill(OPERATOR_KEY);
    await page.getByTestId("reg-service").fill(SERVICE);
    await next(page);
    await toReview(page);
    await page.getByTestId("reg-passphrase").fill("demo");
    // The send is handed to the network but its answer is late: our own operation already holds the coin.
    const release = await page.evaluateHandle(() => window.__dwFixture!.hold("mn_register"));
    await page.getByTestId("reg-register").click();
    await expect.poll(() => page.evaluate(() => window.__dwFixture!.calls.filter((c) => c === "mn_register").length)).toBe(1);
    await page.waitForTimeout(500);
    await fontsReady(page);
    await page.screenshot({ path: info.outputPath(`reg-sending-${theme}.png`) });
    await expect(step(page, "collateral")).toHaveCount(0);
    await expect(page.getByTestId("reg-error-outpoint")).toHaveCount(0);
    await expect(step(page, "review")).toBeVisible();
    await expect(page.getByTestId("reg-preview-loading")).toHaveCount(0);
    await expect(page.getByTestId("reg-review-total")).toBeVisible();
    await release.evaluate((r) => r());
    await expect(page.getByTestId("reg-done-body")).toBeVisible();
    expect(errors).toEqual([]);
  });

test("External: Review names the other wallet's coin; Continue saves it and opens Prove", async ({ page }) => {
  const errors = await open(page, "reg");
  await setUp(page);
  await next(page);
  await page.getByTestId("reg-collateral-external").click();
  // dash-qt's txid:n form is accepted.
  await page.getByTestId("reg-external-outpoint").fill(EXTERNAL_OUTPOINT.replace(/-(\d+)$/, ":$1"));
  await page.getByTestId("reg-external-address").fill(EXTERNAL_ADDRESS);
  await next(page);
  await page.getByTestId("reg-operator-key").fill(OPERATOR_KEY);
  await page.getByTestId("reg-service").fill(SERVICE);
  await next(page);
  await expect(page.getByTestId("reg-owner")).toBeVisible();
  await toReview(page);
  await expect(page.getByTestId("reg-review-collateral")).toContainText(`${EXTERNAL_OUTPOINT.slice(0, 8)}:0, in another wallet`);
  await expect(page.getByTestId("reg-review-collateral-note")).toHaveText("Keep it unspent in the wallet that holds it. Spending it removes this masternode from the masternode list.");
  await expect(page.getByTestId("reg-continue")).toBeEnabled();
  await expect(page.getByTestId("reg-register")).toHaveCount(0);
  await expect(page.getByTestId("reg-send-off")).toHaveCount(0);
  await fontsReady(page);
  expect(await seriousAxeViolations(page)).toEqual([]);
  await page.getByTestId("reg-continue").click();
  await expect(step(page, "prove")).toBeVisible();
  await expect(page.getByTestId("reg-submit")).toBeVisible();
  await expect(page.getByTestId("reg-send-off")).toHaveCount(0);
  await expect(page.getByTestId("mn-group-in-progress")).toBeVisible();
  expect(errors).toEqual([]);
});

test("D5: Review shows the first figures, then Register's re-read, then sends", async ({ page }) => {
  const errors = await open(page, "reg-preview-changes");
  await setUp(page);
  await hostedToKeys(page);
  await toReview(page);
  // The fixture's fee rises only on a second read, which only Register makes: Review keeps the first.
  await expect(page.getByTestId("reg-review-fee")).toContainText("0.00000453");
  await expect(page.getByTestId("reg-review-total")).toContainText("0.00000453");
  await expect(page.getByTestId("reg-changed")).toHaveCount(0);
  await expectRegister(page);
  await page.getByTestId("reg-passphrase").fill("demo");
  await expect(page.getByTestId("reg-register")).toBeEnabled();
  await page.getByTestId("reg-register").click();
  await expect(page.getByTestId("reg-changed")).toHaveText("The figures changed. Check them again.");
  await expect(page.getByTestId("reg-review-fee")).toContainText("0.00000573");
  await page.getByTestId("reg-passphrase").fill("demo");
  await page.getByTestId("reg-register").click();
  await expect(page.getByTestId("reg-done-body")).toBeVisible();
  // A hosted masternode's secret is the operator's: no link to this wallet's keys.
  await expect(page.getByTestId("reg-open-keys")).toHaveCount(0);
  expect(errors).toEqual([]);
});

test("self-run, Sent: Masternode keys… opens the keys on the operator role to copy the secret", async ({ page }) => {
  const errors = await open(page, "reg");
  await setUp(page);
  await page.getByTestId("reg-runner-self").click();
  await next(page);
  await expect(step(page, "collateral")).toBeVisible();
  await next(page);
  await expect(step(page, "operator")).toBeVisible();
  await page.getByTestId("reg-service").fill(SERVICE);
  await next(page);
  await expect(page.getByTestId("reg-owner")).toBeVisible();
  await toReview(page);
  await page.getByTestId("reg-passphrase").fill("demo");
  await page.getByTestId("reg-register").click();
  await expect(page.getByTestId("reg-done-body")).toContainText("Copy the operator secret from Masternode keys");
  await page.getByTestId("reg-open-keys").click();
  await expect(sheet(page)).toHaveCount(0);
  await expect(page.getByTestId("mn-keys")).toBeVisible();
  await expect(page.getByTestId("mn-keys-role-operator")).toHaveAttribute("aria-pressed", "true");
  expect(errors).toEqual([]);
});

test("self-run with an operator key entered by hand: Sent offers no Masternode keys button", async ({ page }) => {
  const errors = await open(page, "reg");
  await setUp(page);
  await page.getByTestId("reg-runner-self").click();
  await next(page);
  await next(page);
  await page.getByTestId("reg-operator-enter").click();
  await page.getByTestId("reg-operator-key").fill(OPERATOR_KEY);
  await page.getByTestId("reg-service").fill(SERVICE);
  await next(page);
  await expect(page.getByTestId("reg-owner")).toBeVisible();
  await toReview(page);
  await page.getByTestId("reg-passphrase").fill("demo");
  await page.getByTestId("reg-register").click();
  await expect(page.getByTestId("reg-done-body")).toBeVisible();
  await expect(page.getByTestId("reg-open-keys")).toHaveCount(0);
  expect(errors).toEqual([]);
});

test("field errors: an IPv6 or bad IPv4 service, a bad operator key, the voting address set to the owner's", async ({ page }) => {
  const errors = await open(page, "reg");
  await setUp(page);
  await next(page);
  await next(page);
  await next(page);
  await expect(page.getByTestId("reg-error-service")).toHaveText("Enter the server's IP address and port.");
  await expect(page.getByTestId("reg-error-operatorKey")).toHaveText("Enter a BLS public key: 96 hexadecimal characters.");
  await page.getByTestId("reg-operator-key").fill("abc");
  await page.getByTestId("reg-service").fill("203.0.113.300:19999");
  await next(page);
  await expect(page.getByTestId("reg-error-service")).toHaveText("Use an IPv4 address and port, like 203.0.113.7:19999. IPv6 and Tor addresses aren't supported yet.");
  await expect(page.getByTestId("reg-error-operatorKey")).toHaveText("Enter a BLS public key: 96 hexadecimal characters.");
  await expect(page.getByTestId("reg-service")).toHaveAttribute("aria-invalid", "true");
  await page.getByTestId("reg-service").fill("[2001:db8::7]:19999");
  await next(page);
  await expect(page.getByTestId("reg-error-service")).toContainText("Use an IPv4 address and port");
  await page.getByTestId("reg-service").fill("203.0.113.20:9999");
  await next(page);
  await expect(page.getByTestId("reg-error-service")).toHaveText("9999 is the Mainnet port. This wallet is on Testnet, where masternodes use 19999.");
  await page.getByTestId("reg-share").fill("100.5");
  await page.getByTestId("reg-service").fill(SERVICE);
  await page.getByTestId("reg-operator-key").fill(OPERATOR_KEY);
  await next(page);
  await expect(page.getByTestId("reg-error-operatorShare")).toHaveText("Enter a share from 0 to 100%, with at most two decimals.");
  await page.getByTestId("reg-share").fill("2.5");
  await next(page);
  await expect(page.getByTestId("reg-owner")).toBeVisible();
  await page.getByTestId("reg-voting-other").click();
  await page.getByTestId("reg-voting-address").fill(keyInfo("owner", 0).address!);
  await next(page);
  await expect(page.getByTestId("reg-error-votingAddress")).toHaveText("The voting address can't be the owner address.");
  await expect(step(page, "keys")).toBeVisible();
  expect(errors).toEqual([]);
});

test("External: the wallet's own coin and a malformed outpoint are refused before any call", async ({ page }) => {
  const errors = await open(page, "reg");
  await setUp(page);
  await next(page);
  await page.getByTestId("reg-collateral-external").click();
  await page.getByTestId("reg-external-outpoint").fill("5d0e7b42:1");
  await next(page);
  await expect(page.getByTestId("reg-error-externalOutpoint")).toHaveText("Enter the transaction ID and output index, for example 5d0e…7b42:1.");
  await expect(page.getByTestId("reg-error-externalAddress")).toHaveText("Enter an address.");
  await page.getByTestId("reg-external-outpoint").fill(REG_COIN);
  await page.getByTestId("reg-external-address").fill(EXTERNAL_ADDRESS);
  await next(page);
  await expect(page.getByTestId("reg-error-externalOutpoint")).toHaveText("This output is in this wallet: choose Use a 1,000 Dash output already in this wallet.");
  expect(errors).toEqual([]);
});

test("refusals go back to their step and field: a duplicate service, a duplicate operator key", async ({ page }) => {
  const errors = await open(page, "reg");
  await setUp(page);
  await hostedToKeys(page, DUP_SERVICE);
  await next(page);
  // Review's preview is refused: the wizard returns to Operator, on the service field.
  await expect(step(page, "operator")).toBeVisible();
  await expect(page.getByTestId("reg-error-service")).toHaveText("This server address is already registered.");
  await page.getByTestId("reg-service").fill(SERVICE);
  await page.getByTestId("reg-operator-key").fill(DUP_OPERATOR_KEY);
  await next(page);
  await next(page);
  await expect(step(page, "operator")).toBeVisible();
  await expect(page.getByTestId("reg-error-operatorKey")).toHaveText("The operator key is already used by another masternode.");
  await page.getByTestId("reg-operator-key").fill(OPERATOR_KEY);
  await next(page);
  await toReview(page);
  expect(errors).toEqual([]);
});

test("closing after Type asks to discard; Keep editing stays", async ({ page }) => {
  const errors = await open(page, "reg");
  await setUp(page);
  await next(page);
  await page.keyboard.press("Escape");
  await expect(page.getByTestId("reg-discard")).toContainText("Discard this registration? Nothing was sent.");
  await page.getByTestId("reg-keep").click();
  await expect(step(page, "collateral")).toBeVisible();
  await page.keyboard.press("Escape");
  await page.getByTestId("reg-discard-confirm").click();
  await expect(sheet(page)).toHaveCount(0);
  await expect(page.getByTestId("mn-group-in-progress")).toHaveCount(0);
  expect(errors).toEqual([]);
});

test("In progress: each state's row and action; nothing on a conflicted one", async ({ page }) => {
  const errors = await open(page, "mn-ops");
  const group = page.getByTestId("mn-group-in-progress");
  await expect(group).toBeVisible();
  await expect(page.getByTestId(`mn-op-${OP_CONFIRMED}`)).toHaveCount(0);
  // Conflicted: a notice once, no row, no action.
  await expect(page.getByTestId(`mn-op-conflicted-${OP_CONFLICTED}`)).toHaveText("Registration Replaced: a coin it used was spent elsewhere");
  await expect(page.getByTestId(`mn-op-${OP_CONFLICTED}`)).toHaveCount(0);
  await expect(page.getByTestId(`mn-op-conflicted-${OP_CONFLICTED}`).getByRole("button")).toHaveCount(0);
  await expect(page.getByTestId(`mn-op-status-${OP_NEEDS_SIGNATURE}`)).toHaveText("Needs signature");
  await expect(page.getByTestId(`mn-op-status-${OP_INPUTS_SPENT}`)).toHaveText("The coins for the fee were spent");
  await expect(page.getByTestId(`mn-op-status-${OP_NOT_SENT}`)).toContainText("Not sent: ");
  // Testnet: registration and registrar-update rows offer Continue, Get a new message or Send again.
  // Discard stays wherever the state allows it.
  await expect(page.getByTestId(`mn-op-continue-${OP_NEEDS_SIGNATURE}`)).toBeVisible();
  await expect(page.getByTestId(`mn-op-refresh-${OP_INPUTS_SPENT}`)).toBeVisible();
  for (const id of [OP_SIGNED, OP_NOT_SENT, OP_SENT, OP_SENT_OVERDUE, OP_UPDATE]) await expect(page.getByTestId(`mn-op-resend-${id}`)).toBeVisible();
  for (const id of [OP_NEEDS_SIGNATURE, OP_INPUTS_SPENT, OP_SIGNED, OP_NOT_SENT]) await expect(page.getByTestId(`mn-op-more-${id}`)).toBeVisible();
  // Sent: never Discard.
  await expect(page.getByTestId(`mn-op-status-${OP_SENT}`)).toHaveText("Registration · waiting for 1 confirmation");
  await expect(page.getByTestId(`mn-op-more-${OP_SENT}`)).toHaveCount(0);
  await expect(page.getByTestId(`mn-op-overdue-${OP_SENT}`)).toHaveCount(0);
  // Overdue with fee inputs: the D49 line and Open in Activity; a dissolution locks nothing.
  await expect(page.getByTestId(`mn-op-overdue-${OP_SENT_OVERDUE}`)).toHaveText("If this never confirms: abandon its transaction in Activity, then unlock one of its coins and spend it.");
  await expect(page.getByTestId(`mn-op-overdue-${OP_DISSOLVE}`)).toHaveText("It locks none of your coins. It confirms, or another dissolution replaces it.");
  await expect(page.getByTestId(`mn-op-activity-${OP_DISSOLVE}`)).toHaveCount(0);
  await fontsReady(page);
  expect(await seriousAxeViolations(page)).toEqual([]);
  // A dissolution still sends again: the confirm names the row's debit (penalty and fee), and the grant sends it.
  await page.getByTestId(`mn-op-resend-${OP_DISSOLVE}`).click();
  await expect(page.getByTestId("mn-op-resend-text")).toHaveText("Send the same transaction again. It costs at most 25.00000453 tDASH.");
  await page.getByTestId("mn-op-resend-passphrase").fill("demo");
  await page.getByTestId("mn-op-resend-confirm").click();
  await expect(page.getByTestId("mn-op-resend-sheet")).toHaveCount(0);
  // Discard from More.
  await page.getByTestId(`mn-op-more-${OP_NOT_SENT}`).click();
  await page.getByTestId(`mn-op-more-${OP_NOT_SENT}-discard`).click();
  await expect(page.getByTestId("mn-op-discard-sheet")).toContainText("Discard this registration? Its reserved coins are released.");
  await page.getByTestId("mn-op-discard-confirm").click();
  await expect(page.getByTestId(`mn-op-${OP_NOT_SENT}`)).toHaveCount(0);
  // Open in Activity, filtered to the overdue row's transaction.
  await page.getByTestId(`mn-op-activity-${OP_SENT_OVERDUE}`).click();
  await expect(page.getByTestId("page-activity")).toBeVisible();
  await expect(page.getByTestId("tx-route-filter")).toContainText("Registration transaction");
  expect(errors).toEqual([]);
});

test("a saved registration offers Continue, and Prove can submit; Discard still releases it", async ({ page }) => {
  const errors = await open(page, "reg-prepared");
  await expect(page.getByTestId(`mn-op-status-${OP_PREPARED}`)).toHaveText("Needs signature");
  await expect(page.getByTestId(`mn-op-continue-${OP_PREPARED}`)).toBeVisible();
  await page.getByTestId(`mn-op-continue-${OP_PREPARED}`).click();
  await expect(step(page, "prove")).toBeVisible();
  await expect(page.getByTestId("reg-submit")).toBeVisible();
  await expect(page.getByTestId("reg-send-off")).toHaveCount(0);
  await page.getByTestId("reg-cancel").click();
  await expect(sheet(page)).toHaveCount(0);
  await page.getByTestId(`mn-op-more-${OP_PREPARED}`).click();
  await expect(page.getByRole("menuitem")).toHaveText(["Discard"]);
  await page.getByTestId(`mn-op-more-${OP_PREPARED}-discard`).click();
  await page.getByTestId("mn-op-discard-confirm").click();
  await expect(page.getByTestId("mn-group-in-progress")).toHaveCount(0);
  expect(errors).toEqual([]);
});

const revokes = (page: Page) => page.evaluate(() => window.__dwFixture!.calls.filter((c) => c === "revoke_grant").length);
/** The cap of the last masternode grant the wallet issued. */
const lastCap = (page: Page) =>
  page.evaluate(() => {
    const p = window.__dwFixture!.world.grants.map((g) => g.grant.purpose).filter((x) => x.kind === "masternodeOp").at(-1);
    return p && "maxDuffs" in p ? Number(p.maxDuffs) : null;
  });
/** Grants issued and neither redeemed nor revoked: a refusal must leave none. */
const liveGrants = (page: Page) => page.evaluate(() => window.__dwFixture!.world.grants.filter((g) => !g.used).map((g) => g.grant.id));

for (const theme of ["light", "dark"] as const)
  test(`${theme}: a saved registration whose fee coins were spent offers a new message from Prove, then submits (issue 460)`, async ({ page }, info) => {
    const errors = await open(page, "reg-prepared-spent", theme);
    await expect(page.getByTestId(`mn-op-status-${OP_PREPARED}`)).toHaveText("The coins for the fee were spent");
    await page.getByTestId(`mn-op-refresh-${OP_PREPARED}`).click();
    await expect(step(page, "prove")).toBeVisible();
    await page.waitForTimeout(300);
    await fontsReady(page);
    await page.screenshot({ path: info.outputPath(`reg-prove-spent-${theme}.png`) });
    // The engine sends no sign request for a spent row: Prove offers the new message, not an empty step.
    await expect(page.getByTestId("reg-inputs-spent")).toBeVisible();
    await expect(page.getByTestId("reg-sign-request")).toHaveCount(0);
    // No message yet, so no "sign this message": the notice carries the step (issue 641).
    await expect(sheet(page)).not.toContainText("sign this message");
    await expect(page.getByTestId("reg-submit")).toBeDisabled();
    await expect(page.getByTestId("reg-prove-debit")).toContainText("0.00000453");
    await page.getByTestId("reg-new-message").click();
    await expect(page.getByTestId("reg-sign-request")).toBeVisible();
    await expect(page.getByTestId("reg-inputs-spent")).toHaveCount(0);
    await expect(page.getByTestId("reg-prove-text")).toContainText("sign this message");
    // The new message comes with new fee inputs: Prove shows the replanned fee, and the grant is capped at it.
    await expect(page.getByTestId("reg-prove-fee")).toContainText("0.00000573");
    await expect(page.getByTestId("reg-prove-debit")).toContainText("0.00000573");
    await page.getByTestId("reg-signature").fill(EXTERNAL_SIGNATURE);
    await page.getByTestId("reg-passphrase").fill("demo");
    await page.getByTestId("reg-submit").click();
    await expect(page.getByTestId("reg-done-body")).toBeVisible();
    expect(await lastCap(page)).toBe(573);
    expect(errors).toEqual([]);
  });

for (const theme of ["light", "dark"] as const)
  test(`${theme}: Prove names the fee and what leaves this wallet (issue 460)`, async ({ page }, info) => {
    const errors = await open(page, "reg-prepared", theme);
    await expect(page.getByTestId(`mn-op-status-${OP_PREPARED}`)).toHaveText("Needs signature");
    await page.getByTestId(`mn-op-continue-${OP_PREPARED}`).click();
    await expect(step(page, "prove")).toBeVisible();
    await page.waitForTimeout(300);
    await fontsReady(page);
    await page.screenshot({ path: info.outputPath(`reg-prove-${theme}.png`) });
    await expect(page.getByTestId("reg-prove-fee")).toContainText("0.00000453");
    await expect(page.getByTestId("reg-prove-debit")).toContainText("0.00000453");
    expect(errors).toEqual([]);
  });

for (const theme of ["light", "dark"] as const)
  test(`${theme}: External Review asks no passphrase, since Continue signs nothing (issue 460)`, async ({ page }, info) => {
    const errors = await open(page, "reg", theme);
    await setUp(page);
    await next(page);
    await page.getByTestId("reg-collateral-external").click();
    await page.getByTestId("reg-external-outpoint").fill(EXTERNAL_OUTPOINT);
    await page.getByTestId("reg-external-address").fill(EXTERNAL_ADDRESS);
    await next(page);
    await page.getByTestId("reg-operator-key").fill(OPERATOR_KEY);
    await page.getByTestId("reg-service").fill(SERVICE);
    await next(page);
    await toReview(page);
    await fontsReady(page);
    await page.screenshot({ path: info.outputPath(`reg-external-review-${theme}.png`) });
    await expect(page.getByTestId("reg-passphrase")).toHaveCount(0);
    await expect(page.getByTestId("reg-continue")).toBeEnabled();
    await page.getByTestId("reg-continue").click();
    await expect(step(page, "prove")).toBeVisible();
    expect(errors).toEqual([]);
  });

test("a mistyped signature is refused before any signing, and its grant is revoked (issue 460)", async ({ page }) => {
  const errors = await open(page, "reg-prepared");
  await expect(page.getByTestId(`mn-op-status-${OP_PREPARED}`)).toHaveText("Needs signature");
  await page.getByTestId(`mn-op-continue-${OP_PREPARED}`).click();
  await expect(step(page, "prove")).toBeVisible();
  const before = await revokes(page);
  await page.getByTestId("reg-signature").fill(`H${"B".repeat(86)}=`);
  await page.getByTestId("reg-passphrase").fill("demo");
  await page.getByTestId("reg-submit").click();
  await expect.poll(() => revokes(page)).toBe(before + 1);
  // The refused submit's grant is gone: none is left to redeem.
  expect(await liveGrants(page)).toEqual([]);
  await expect(step(page, "prove")).toBeVisible();
  await expect(page.getByTestId("reg-done-body")).toHaveCount(0);
  expect(errors).toEqual([]);
});

for (const theme of ["light", "dark"] as const)
  test(`${theme}: a failed In progress read says so instead of reading as nothing in progress (issue 460)`, async ({ page }, info) => {
    const errors = await open(page, "reg-prepared", theme);
    await expect(page.getByTestId(`mn-op-${OP_PREPARED}`)).toBeVisible();
    await page.evaluate(() => window.__dwFixture!.fail("mn_operations", "rpc"));
    await page.evaluate(() => window.__dwFixture!.emit({ kind: "masternodes", network: window.__dwFixture!.world.launch.network.dirName }));
    await expect.poll(() => page.evaluate(() => window.__dwFixture!.calls.filter((c) => c === "mn_operations").length)).toBeGreaterThan(1);
    await page.waitForTimeout(500);
    await fontsReady(page);
    await page.screenshot({ path: info.outputPath(`mn-ops-failed-${theme}.png`) });
    await expect(page.getByTestId("mn-ops-failed")).toBeVisible();
    // What the last read found stays shown beside the failure.
    await expect(page.getByTestId(`mn-op-${OP_PREPARED}`)).toBeVisible();
    await page.evaluate(() => window.__dwFixture!.fail("mn_operations", null));
    await page.getByTestId("mn-ops-retry").click();
    await expect(page.getByTestId("mn-ops-failed")).toHaveCount(0);
    expect(errors.filter((e) => !e.includes("mn_operations"))).toEqual([]);
  });

test("fee coins spent while Prove is open: the refusal hides the old message and offers a new one (issue 460)", async ({ page }) => {
  const errors = await open(page, "reg-prepared");
  await expect(page.getByTestId(`mn-op-status-${OP_PREPARED}`)).toHaveText("Needs signature");
  await page.getByTestId(`mn-op-continue-${OP_PREPARED}`).click();
  await expect(page.getByTestId("reg-sign-request")).toBeVisible();
  await page.evaluate(() => {
    window.__dwFixture!.world.masternodes.spendInputs = true;
  });
  const before = await revokes(page);
  // The list's re-read stays out, so the row below reads spent from the refusal alone.
  const releaseOps = await page.evaluateHandle(() => window.__dwFixture!.hold("mn_operations"));
  await page.getByTestId("reg-signature").fill(EXTERNAL_SIGNATURE);
  await page.getByTestId("reg-passphrase").fill("demo");
  await page.getByTestId("reg-submit").click();
  await expect(page.getByTestId("reg-inputs-spent")).toBeVisible();
  await expect(page.getByTestId("reg-sign-request")).toHaveCount(0);
  await expect(sheet(page)).not.toContainText("sign this message");
  await expect.poll(() => revokes(page)).toBe(before + 1);
  expect(await liveGrants(page)).toEqual([]);
  // Closed and resumed with no further read: the saved row reads spent too, and Prove asks for a new message.
  await page.getByTestId("reg-cancel").click();
  await expect(page.getByTestId(`mn-op-status-${OP_PREPARED}`)).toHaveText("The coins for the fee were spent");
  await expect(page.getByTestId(`mn-op-continue-${OP_PREPARED}`)).toHaveCount(0);
  await releaseOps.evaluate((release) => release());
  await expect.poll(() => page.evaluate(() => window.__dwFixture!.calls.filter((c) => c === "mn_operations").length)).toBeGreaterThan(1);
  await expect(page.getByTestId(`mn-op-status-${OP_PREPARED}`)).toHaveText("The coins for the fee were spent");
  await page.getByTestId(`mn-op-refresh-${OP_PREPARED}`).click();
  await expect(page.getByTestId("reg-inputs-spent")).toBeVisible();
  await expect(page.getByTestId("reg-sign-request")).toHaveCount(0);
  // The new message, then closed to sign it elsewhere while the list can't be read again (each read
  // fails, so none brings the new row): In progress reopens the row the engine returned, with that
  // message and the signature field.
  await page.evaluate(() => window.__dwFixture!.fail("mn_operations", "rpc"));
  await page.getByTestId("reg-new-message").click();
  await expect(page.getByTestId("reg-sign-request")).toBeVisible();
  const message = await page.getByTestId("reg-sign-message").textContent();
  await page.getByTestId("reg-cancel").click();
  await page.getByTestId(`mn-op-continue-${OP_PREPARED}`).click();
  await expect(page.getByTestId("reg-sign-message")).toHaveText(message!);
  await expect(page.getByTestId("reg-signature")).toBeVisible();
  await expect(page.getByTestId("reg-inputs-spent")).toHaveCount(0);
  await page.evaluate(() => window.__dwFixture!.fail("mn_operations", null));
  expect(errors.filter((e) => !e.includes("rpc"))).toEqual([]);
});

test("a refused Send again from In progress revokes its grant (issue 460)", async ({ page }) => {
  const errors = await open(page, "mn-ops");
  await page.getByTestId(`mn-op-resend-${OP_SIGNED}`).click();
  await page.evaluate(() => window.__dwFixture!.fail("mn_resend_operation", "rpc"));
  const before = await revokes(page);
  await page.getByTestId("mn-op-resend-passphrase").fill("demo");
  await page.getByTestId("mn-op-resend-confirm").click();
  await expect.poll(() => revokes(page)).toBe(before + 1);
  expect(await liveGrants(page)).toEqual([]);
  await expect(page.getByTestId("mn-op-resend-sheet")).toBeVisible();
  expect(errors.filter((e) => !e.includes("rpc"))).toEqual([]);
});

test("a failed masternode list read still shows the work in progress (issue 460)", async ({ page }) => {
  const errors = await open(page, "reg-prepared");
  await expect(page.getByTestId(`mn-op-${OP_PREPARED}`)).toBeVisible();
  await page.evaluate(() => window.__dwFixture!.fail("mn_list", "rpc"));
  await page.evaluate(() => window.__dwFixture!.emit({ kind: "masternodes", network: window.__dwFixture!.world.launch.network.dirName }));
  await expect(page.getByTestId("mn-list-failed")).toBeVisible();
  await expect(page.getByTestId(`mn-op-continue-${OP_PREPARED}`)).toBeVisible();
  expect(errors.filter((e) => !e.includes("rpc"))).toEqual([]);
});

test("a failed new message, then one that works, leaves no stale error (review of issue 460)", async ({ page }) => {
  const errors = await open(page, "reg-prepared-spent");
  await page.getByTestId(`mn-op-refresh-${OP_PREPARED}`).click();
  await expect(page.getByTestId("reg-inputs-spent")).toBeVisible();
  await page.evaluate(() => window.__dwFixture!.fail("mn_refresh_external_registration", "rpc"));
  await page.getByTestId("reg-new-message").click();
  await expect(page.getByTestId("reg-general")).not.toBeEmpty();
  await page.evaluate(() => window.__dwFixture!.fail("mn_refresh_external_registration", null));
  await page.getByTestId("reg-new-message").click();
  await expect(page.getByTestId("reg-sign-request")).toBeVisible();
  await expect(page.getByTestId("reg-general")).toBeEmpty();
  expect(errors.filter((e) => !e.includes("rpc"))).toEqual([]);
});

test("a Discard opened from More returns focus to More, not to the row's Send again (review of issue 460)", async ({ page }) => {
  const errors = await open(page, "mn-ops");
  await expect(page.getByTestId(`mn-op-resend-${OP_NOT_SENT}`)).toBeVisible();
  await page.getByTestId(`mn-op-more-${OP_NOT_SENT}`).click();
  await page.getByTestId(`mn-op-more-${OP_NOT_SENT}-discard`).click();
  await expect(page.getByTestId("mn-op-discard-sheet")).toBeVisible();
  await page.keyboard.press("Escape");
  await expect(page.getByTestId("mn-op-discard-sheet")).toHaveCount(0);
  await expect(page.getByTestId(`mn-op-more-${OP_NOT_SENT}`)).toBeFocused();
  expect(errors).toEqual([]);
});

for (const theme of ["light", "dark"] as const)
  test(`${theme}: In progress shows its loading state while its read is out (review of issue 460)`, async ({ page }, info) => {
    const errors = await open(page, "reg-prepared", theme, undefined, "&hold=mn_operations");
    await expect(page.getByTestId("mn-empty")).toBeVisible();
    await page.waitForTimeout(300);
    await fontsReady(page);
    await page.screenshot({ path: info.outputPath(`mn-ops-loading-${theme}.png`) });
    await expect(page.getByTestId("mn-ops-loading")).toBeVisible();
    await page.evaluate(() => window.__dwFixture!.release("mn_operations"));
    await expect(page.getByTestId(`mn-op-${OP_PREPARED}`)).toBeVisible();
    await expect(page.getByTestId("mn-ops-loading")).toHaveCount(0);
    expect(errors).toEqual([]);
  });

for (const theme of ["light", "dark"] as const)
  test(`${theme}: a list read that recovers keeps an open Send again and its passphrase (review of issue 460)`, async ({ page }, info) => {
    const errors = await open(page, "mn-ops", theme, undefined, "&fail=mn_list:rpc");
    await expect(page.getByTestId("mn-list-failed")).toBeVisible();
    await page.waitForTimeout(300);
    await fontsReady(page);
    await page.screenshot({ path: info.outputPath(`mn-list-failed-${theme}.png`) });
    await expect(page.getByTestId(`mn-op-resend-${OP_SIGNED}`)).toBeVisible();
    await page.getByTestId(`mn-op-resend-${OP_SIGNED}`).click();
    await page.getByTestId("mn-op-resend-passphrase").fill("demo");
    // The list comes back while the confirmation is open: it stays, with what was typed.
    await page.evaluate(() => window.__dwFixture!.fail("mn_list", null));
    await page.evaluate(() => window.__dwFixture!.emit({ kind: "masternodes", network: window.__dwFixture!.world.launch.network.dirName }));
    await expect(page.getByTestId("mn-summary")).toBeVisible();
    await expect(page.getByTestId("mn-op-resend-sheet")).toBeVisible();
    await expect(page.getByTestId("mn-op-resend-passphrase")).toHaveValue("demo");
    // Closed, focus goes back to the row's Send again, not to the button the list read replaced.
    await page.keyboard.press("Escape");
    await expect(page.getByTestId("mn-op-resend-sheet")).toHaveCount(0);
    await expect(page.getByTestId(`mn-op-resend-${OP_SIGNED}`)).toBeFocused();
    expect(errors.filter((e) => !e.includes("rpc"))).toEqual([]);
  });

test("a released collateral offers Lock it again on testnet", async ({ page }) => {
  const errors = await open(page, "mn-released");
  await page.getByTestId(`mn-row-${HOME}`).click();
  await expect(page.getByTestId("mn-released")).toContainText("Collateral released: spending it ends this masternode.");
  await page.getByTestId("mn-more").click();
  await expect(page.getByTestId("mn-more-lock-again")).toBeVisible();
  await expect(page.getByTestId("mn-more-release")).toHaveCount(0);
  await page.keyboard.press("Escape");
  await page.getByTestId("mn-lock-again").click();
  await expect(page.getByTestId("mn-released")).toHaveCount(0);
  expect(errors).toEqual([]);
});

/** All a held wizard reads, from opening to Review. The list comes from the page's own read; the
 * page's governance polls run on their own timers, which the sheet doesn't own. */
const HELD_READS = ["mn_preview_registration", "mn_registration_keys"];
const PAGE_POLLS = ["governance_contested_names", "governance_info", "governance_my_proposals", "governance_upcoming_superblocks"];

for (const theme of ["light", "dark"] as const)
  test(`mainnet ${theme}: the held wizard reads only keys and the preview (issue 635)`, async ({ page }, info) => {
    const errors = await open(page, "reg", theme, "mainnet");
    await expect(page.getByTestId("mn-set-up")).toBeVisible();
    // The page's own polls have started, so a late first one isn't taken for the sheet's.
    await expect
      .poll(() => page.evaluate(() => ["governance_info", "governance_my_proposals"].every((c) => window.__dwFixture!.calls.includes(c))))
      .toBe(true);
    const opened = await page.evaluate(() => [...window.__dwFixture!.calls]);
    const from = opened.length;
    // A poll counts as the page's only if the page made it before the sheet opened.
    const pagePoll = (c: string) => PAGE_POLLS.includes(c) && opened.includes(c);
    await setUp(page);
    await hostedToKeys(page, "203.0.113.20:9999");
    await toReview(page);
    await expectSendOff(page);
    await fontsReady(page);
    await page.screenshot({ path: info.outputPath(`reg-held-review-${theme}.png`) });
    const made = await page.evaluate((n) => window.__dwFixture!.calls.slice(n), from);
    expect([...new Set(made.filter((c) => !pagePoll(c)))].sort()).toEqual(HELD_READS);
    await expect(page.getByTestId("reg-passphrase")).toHaveCount(0);
    expect(errors).toEqual([]);
  });

test("mainnet: opening Set up after a failed list read doesn't read the list again (issue 635)", async ({ page }) => {
  const errors = await open(page, "reg", "light", "mainnet", "&fail=mn_list:rpc");
  await expect(page.getByTestId("mn-list-failed")).toBeVisible();
  const reads = () => page.evaluate(() => window.__dwFixture!.calls.filter((c) => c === "mn_list").length);
  const before = await reads();
  await page.getByTestId("mn-set-up").click();
  await expect(sheet(page)).toBeVisible();
  await page.waitForTimeout(500);
  expect(await reads()).toBe(before);
  expect(errors.filter((e) => !e.includes("rpc"))).toEqual([]);
});

test("mainnet: Review ends with the send-off line and sends nothing", async ({ page }) => {
  const errors = await open(page, "reg", "light", "mainnet");
  await setUp(page);
  await hostedToKeys(page, "203.0.113.20:9999");
  await toReview(page);
  await expectSendOff(page);
  await sheet(page).locator("form").evaluate((f: HTMLFormElement) => f.requestSubmit());
  await expect(step(page, "review")).toBeVisible();
  await page.keyboard.press("Escape");
  await page.getByTestId("reg-discard-confirm").click();
  await expect(sheet(page)).toHaveCount(0);
  await expect(page.getByTestId("mn-group-in-progress")).toHaveCount(0);
  expect(errors).toEqual([]);
});

test("mainnet: an external review sends nothing and saves nothing", async ({ page }) => {
  const errors = await open(page, "reg", "light", "mainnet");
  await setUp(page);
  await next(page);
  await page.getByTestId("reg-collateral-external").click();
  await page.getByTestId("reg-external-outpoint").fill(EXTERNAL_OUTPOINT.replace(/-(\d+)$/, ":$1"));
  await page.getByTestId("reg-external-address").fill(EXTERNAL_ADDRESS);
  await next(page);
  await page.getByTestId("reg-operator-key").fill(OPERATOR_KEY);
  await page.getByTestId("reg-service").fill("203.0.113.20:9999");
  await next(page);
  await toReview(page);
  await expectSendOff(page);
  await expect(page.getByTestId("reg-continue")).toHaveCount(0);
  await sheet(page).locator("form").evaluate((f: HTMLFormElement) => f.requestSubmit());
  await expect(step(page, "review")).toBeVisible();
  await expect(step(page, "prove")).toHaveCount(0);
  await page.keyboard.press("Escape");
  await page.getByTestId("reg-discard-confirm").click();
  await expect(page.getByTestId("mn-group-in-progress")).toHaveCount(0);
  expect(errors).toEqual([]);
});

test("mainnet: registration and registrar-update rows offer no send", async ({ page }) => {
  const errors = await open(page, "mn-ops", "light", "mainnet");
  const held = [OP_NEEDS_SIGNATURE, OP_INPUTS_SPENT, OP_SIGNED, OP_NOT_SENT, OP_SENT, OP_SENT_OVERDUE, OP_UPDATE];
  for (const id of held) for (const a of ["continue", "refresh", "resend"]) await expect(page.getByTestId(`mn-op-${a}-${id}`)).toHaveCount(0);
  for (const id of [OP_NEEDS_SIGNATURE, OP_INPUTS_SPENT, OP_SIGNED, OP_NOT_SENT]) await expect(page.getByTestId(`mn-op-more-${id}`)).toBeVisible();
  await expect(page.getByTestId(`mn-op-resend-${OP_DISSOLVE}`)).toBeVisible();
  expect(errors).toEqual([]);
});

test("mainnet: a saved registration keeps only Discard", async ({ page }) => {
  const errors = await open(page, "reg-prepared", "light", "mainnet");
  await expect(page.getByTestId(`mn-op-status-${OP_PREPARED}`)).toHaveText("Needs signature");
  for (const a of ["continue", "refresh", "resend"]) await expect(page.getByTestId(`mn-op-${a}-${OP_PREPARED}`)).toHaveCount(0);
  await page.getByTestId(`mn-op-more-${OP_PREPARED}`).click();
  await expect(page.getByRole("menuitem")).toHaveText(["Discard"]);
  expect(errors).toEqual([]);
});

test("mainnet: a released collateral has no Lock it again and no Release", async ({ page }) => {
  const errors = await open(page, "mn-released", "light", "mainnet");
  await page.getByTestId(`mn-row-${HOME}`).click();
  await expect(page.getByTestId("mn-released")).toHaveText("Collateral released: spending it ends this masternode.");
  await expect(page.getByTestId("mn-lock-again")).toHaveCount(0);
  await page.getByTestId("mn-more").click();
  await expect(page.getByTestId("mn-more-lock-again")).toHaveCount(0);
  await expect(page.getByTestId("mn-more-release")).toHaveCount(0);
  expect(errors).toEqual([]);
});
