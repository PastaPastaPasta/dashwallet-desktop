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
  REG_COIN,
  REG_COIN_LOCKED,
} from "../src/fixture/masternodes";
import { FIXTURE_NOW } from "../src/fixture/wallet";
import { seriousAxeViolations } from "./axe";
import { fixtureUrl, fontsReady, trackPageErrors } from "./helpers";

// MG-10 S2: the registration wizard (hosted and self-run, regular and evonode, FundNew,
// ExistingUtxo and External), the D5 re-read, field errors and refusals by step, the In progress
// group's states, and Release collateral / Lock it again, on the fixture.

async function open(page: Page, scenario: string, theme: "light" | "dark" = "light") {
  const errors = trackPageErrors(page);
  await page.clock.setFixedTime(FIXTURE_NOW);
  await page.goto(fixtureUrl(scenario, theme));
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

async function toReview(page: Page) {
  await next(page);
  await expect(page.getByTestId("reg-review")).toBeVisible();
}

test("hosted regular, FundNew: Type → Review → Register → Sent; 0 serious axe issues", async ({ page }) => {
  const errors = await open(page, "reg");
  await setUp(page);
  await expect(sheet(page)).toContainText("Step 1 of 5");
  await expect(page.getByTestId("reg-kind-regular").locator("input")).toBeChecked();
  await expect(page.getByTestId("reg-runner-hosted").locator("input")).toBeChecked();
  await fontsReady(page);
  expect(await seriousAxeViolations(page)).toEqual([]);
  await next(page);
  await expect(step(page, "collateral")).toContainText("Where is the 1,000 Dash collateral?");
  await expect(page.getByTestId("reg-collateral-fund-new").locator("input")).toBeChecked();
  await next(page);
  await expect(step(page, "operator")).toContainText("Your operator's details");
  await page.getByTestId("reg-operator-key").fill(OPERATOR_KEY);
  await page.getByTestId("reg-service").fill(SERVICE);
  await next(page);
  await expect(page.getByTestId("reg-owner")).toHaveText("This wallet, owner keypair 1");
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
  // The passphrase is asked before anything is signed.
  await page.getByTestId("reg-register").click();
  await expect(page.getByTestId("reg-error")).toHaveText("Enter your passphrase to register.");
  await page.getByTestId("reg-passphrase").fill("demo");
  await page.getByTestId("reg-register").click();
  await expect(step(page, "done")).toContainText("Registration sent");
  await expect(page.getByTestId("reg-done-body")).toContainText("waiting for 1 confirmation");
  await expect(page.getByTestId("reg-done-body")).toContainText("Send your operator the ProTx hash.");
  await expect(page.getByTestId("reg-hash")).toHaveText(/^[0-9a-f]{64}$/);
  await fontsReady(page);
  expect(await seriousAxeViolations(page)).toEqual([]);
  await page.getByTestId("reg-done").click();
  await expect(sheet(page)).toHaveCount(0);
  // The sent registration waits in In progress, and its masternode is listed unconfirmed.
  await expect(page.getByTestId("mn-group-in-progress")).toContainText("Registration · waiting for 1 confirmation");
  await expect(page.getByTestId("mn-group-mine")).toContainText(SERVICE);
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
  await expect(step(page, "operator")).toContainText("come from this wallet's masternode keys (keypair 3)");
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
  await expect(review).toContainText(`${keyInfo("platform_node", 0).platformNodeId!.slice(0, 8)} · 36656 / 1443`);
  await expect(page.getByTestId("reg-review-fee")).toContainText("0.00000567");
  await expect(page.getByTestId("reg-register")).toBeVisible();
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
  await page.getByTestId("reg-passphrase").fill("demo");
  await page.getByTestId("reg-register").click();
  await expect(page.getByTestId("reg-done-body")).toBeVisible();
  expect(errors).toEqual([]);
});

test("External: Continue at Review saves it, Prove shows the message, reopening lands on Prove, a signature sends it", async ({ page }) => {
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
  await expect(sheet(page)).toContainText("Step 5 of 6");
  await expect(page.getByTestId("reg-review-collateral")).toContainText(`${EXTERNAL_OUTPOINT.slice(0, 8)}:0, in another wallet`);
  await expect(sheet(page)).toContainText("Next, you sign a message with the wallet that holds the collateral.");
  await expect(page.getByTestId("reg-register")).toHaveCount(0);
  await page.getByTestId("reg-continue").click();
  await expect(step(page, "prove")).toContainText("Prove the collateral");
  await expect(sheet(page)).toContainText("Step 6 of 6");
  await expect(page.getByTestId("reg-sign-address")).toHaveText(EXTERNAL_ADDRESS);
  await expect(page.getByTestId("reg-sign-message")).toHaveText(/^y\S+\|0\|y\S+\|y\S+\|5e[0-9a-f]+$/);
  await expect(page.getByTestId("reg-prove-saved")).toHaveText("Saved in In progress. You can close this and continue later.");
  await expect(page.getByTestId("reg-back")).toBeDisabled();
  await fontsReady(page);
  expect(await seriousAxeViolations(page)).toEqual([]);
  const message = await page.getByTestId("reg-sign-message").innerText();
  // Closing at Prove asks nothing: the registration is saved.
  await page.keyboard.press("Escape");
  await expect(sheet(page)).toHaveCount(0);
  await expect(page.getByTestId("reg-discard")).toHaveCount(0);
  const group = page.getByTestId("mn-group-in-progress");
  await expect(group).toContainText("Needs signature");
  await group.locator("[data-testid^=mn-op-continue-]").click();
  await expect(step(page, "prove")).toBeVisible();
  await expect(page.getByTestId("reg-sign-message")).toHaveText(message);
  await expect(page.getByTestId("reg-cancel")).toBeVisible();
  // A signature that isn't the collateral key's stays on Prove, on the field.
  await page.getByTestId("reg-signature").fill(`H${"B".repeat(86)}=`);
  await page.getByTestId("reg-passphrase").fill("demo");
  await page.getByTestId("reg-submit").click();
  await expect(page.getByTestId("reg-error-signature")).toHaveText("This signature isn't from the collateral's address over this message.");
  await expect(step(page, "prove")).toBeVisible();
  await page.getByTestId("reg-signature").fill(EXTERNAL_SIGNATURE);
  await page.getByTestId("reg-passphrase").fill("demo");
  await page.getByTestId("reg-submit").click();
  await expect(page.getByTestId("reg-done-body")).toContainText("waiting for 1 confirmation");
  await page.getByTestId("reg-done").click();
  await expect(group).toContainText("Registration · waiting for 1 confirmation");
  await expect(group).not.toContainText("Needs signature");
  expect(errors).toEqual([]);
});

test("a saved External registration reopens at Prove from In progress ▸ Continue", async ({ page }) => {
  const errors = await open(page, "reg-prepared");
  await page.getByTestId(`mn-op-continue-${OP_PREPARED}`).click();
  await expect(step(page, "prove")).toBeVisible();
  await expect(page.getByTestId("reg-sign-address")).toHaveText(EXTERNAL_ADDRESS);
  await page.getByTestId("reg-submit").click();
  await expect(page.getByTestId("reg-error-signature")).toHaveText("Paste the signature from the other wallet.");
  await page.getByTestId("reg-cancel").click();
  await expect(sheet(page)).toHaveCount(0);
  expect(errors).toEqual([]);
});

test("D5: Register re-reads the preview; changed figures show and need a second press", async ({ page }) => {
  const errors = await open(page, "reg-preview-changes");
  await setUp(page);
  await hostedToKeys(page);
  await toReview(page);
  await expect(page.getByTestId("reg-review-fee")).toContainText("0.00000453");
  await page.getByTestId("reg-passphrase").fill("demo");
  await page.getByTestId("reg-register").click();
  await expect(page.getByTestId("reg-changed")).toHaveText("The figures changed. Check them again.");
  await expect(page.getByTestId("reg-review-fee")).toContainText("0.00000573");
  await expect(page.getByTestId("reg-review-total")).toContainText("0.00000573");
  await expect(step(page, "review")).toBeVisible();
  await expect(page.getByTestId("reg-done-body")).toHaveCount(0);
  await page.getByTestId("reg-register").click();
  await expect(page.getByTestId("reg-done-body")).toBeVisible();
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
  await expect(page.getByTestId(`mn-op-continue-${OP_NEEDS_SIGNATURE}`)).toHaveText("Continue");
  await expect(page.getByTestId(`mn-op-status-${OP_INPUTS_SPENT}`)).toHaveText("The coins for the fee were spent");
  await expect(page.getByTestId(`mn-op-refresh-${OP_INPUTS_SPENT}`)).toHaveText("Get a new message");
  await expect(page.getByTestId(`mn-op-status-${OP_NOT_SENT}`)).toContainText("Not sent: ");
  await expect(page.getByTestId(`mn-op-resend-${OP_NOT_SENT}`)).toHaveText("Send again");
  // Sent: Send again, never Discard.
  await expect(page.getByTestId(`mn-op-status-${OP_SENT}`)).toHaveText("Registration · waiting for 1 confirmation");
  await expect(page.getByTestId(`mn-op-more-${OP_SENT}`)).toHaveCount(0);
  await expect(page.getByTestId(`mn-op-overdue-${OP_SENT}`)).toHaveCount(0);
  // Overdue with fee inputs: the D49 line and Open in Activity; a dissolution locks nothing.
  await expect(page.getByTestId(`mn-op-overdue-${OP_SENT_OVERDUE}`)).toHaveText("If this never confirms: abandon its transaction in Activity, then unlock one of its coins and spend it.");
  await expect(page.getByTestId(`mn-op-overdue-${OP_DISSOLVE}`)).toHaveText("It locks none of your coins. It confirms, or another dissolution replaces it.");
  await expect(page.getByTestId(`mn-op-activity-${OP_DISSOLVE}`)).toHaveCount(0);
  await fontsReady(page);
  expect(await seriousAxeViolations(page)).toEqual([]);
  // Send again on a sent row: the confirm names the row's debit, and the grant sends it.
  await page.getByTestId(`mn-op-resend-${OP_SENT}`).click();
  await expect(page.getByTestId("mn-op-resend-text")).toHaveText("Send the same transaction again. It costs at most 0.00000453 tDASH.");
  await page.getByTestId("mn-op-resend-passphrase").fill("demo");
  await page.getByTestId("mn-op-resend-confirm").click();
  await expect(page.getByTestId("mn-op-resend-sheet")).toHaveCount(0);
  await page.getByTestId(`mn-op-resend-${OP_NOT_SENT}`).click();
  await expect(page.getByTestId("mn-op-resend-text")).toHaveText("Send it again. It costs at most 0.00000453 tDASH.");
  await page.keyboard.press("Escape");
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

test("In progress ▸ Continue reopens a needs-signature row at Prove; an inputs-spent row gets a new message", async ({ page }) => {
  const errors = await open(page, "mn-ops");
  await page.getByTestId(`mn-op-continue-${OP_NEEDS_SIGNATURE}`).click();
  await expect(step(page, "prove")).toBeVisible();
  await page.getByTestId("reg-cancel").click();
  await expect(sheet(page)).toHaveCount(0);
  await page.getByTestId(`mn-op-refresh-${OP_INPUTS_SPENT}`).click();
  await expect(page.getByTestId("reg-inputs-spent")).toHaveText(/The coins for the fee were spent\. Sign the new message\./);
  await expect(page.getByTestId("reg-submit")).toBeDisabled();
  const before = await page.getByTestId("reg-sign-message").innerText();
  await page.getByTestId("reg-new-message").click();
  await expect(page.getByTestId("reg-inputs-spent")).toHaveCount(0);
  await expect(page.getByTestId("reg-sign-message")).not.toHaveText(before);
  await expect(page.getByTestId("reg-signature")).toBeVisible();
  expect(errors).toEqual([]);
});

test("Lock it again, then Release collateral, on Home node", async ({ page }) => {
  const errors = await open(page, "mn-released");
  await page.getByTestId(`mn-row-${HOME}`).click();
  await expect(page.getByTestId("mn-released")).toContainText("Collateral released: spending it ends this masternode.");
  await page.getByTestId("mn-lock-again").click();
  await expect(page.getByTestId("mn-released")).toHaveCount(0);
  await page.getByTestId("mn-more").click();
  await expect(page.getByTestId("mn-more-release")).toHaveText("Release collateral…");
  await page.getByTestId("mn-more-release").click();
  const release = page.getByTestId("mn-release-sheet");
  await expect(release).toContainText("Home node's 1,000 Dash collateral is locked");
  await expect(page.getByTestId("mn-release-warning")).toBeVisible();
  await expect(page.getByTestId("mn-release-confirm")).toBeDisabled();
  await fontsReady(page);
  expect(await seriousAxeViolations(page)).toEqual([]);
  await page.getByTestId("mn-release-understood").check();
  await page.getByTestId("mn-release-confirm").click();
  await expect(release).toHaveCount(0);
  await expect(page.getByTestId("mn-released")).toBeVisible();
  await page.getByTestId("mn-more").click();
  await expect(page.getByTestId("mn-more-lock-again")).toHaveText("Lock it again");
  expect(errors).toEqual([]);
});
