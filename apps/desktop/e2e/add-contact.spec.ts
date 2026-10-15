import { expect, test, type Page } from "@playwright/test";
import type { LinkRoute } from "../src/api/generated/LinkRoute";
import { IDENTITY } from "../src/fixture/dashpay";
import { FIXTURE_NOW } from "../src/fixture/wallet";
import { seriousAxeViolations } from "./axe";
import { artifactPath, authorize, fixtureUrl, fontsReady, THEMES, trackPageErrors, type Theme } from "./helpers";

// DP2-08: Add Contact in the Contacts detail slot on the fixture backend, whose eligibility,
// send, verification and scan commands refuse as the engine's do (src/fixture/addContact.test.ts).
// Tests are named by parity ID: IOS-073 (search with eligibility, send, My QR, the verified
// `dashpay://user` link) and IOS-043 (the desktop's scan sources: file, drop, paste, screen).

/** The fixture's clock, in UNIX seconds. */
const NOW = Math.floor(FIXTURE_NOW.getTime() / 1000);
const LINK = (id: string, username: string) => `dashpay://user?id=${id}&username=${username}`;

async function openContacts(page: Page, scenario = "dashpay-registered", theme: Theme = "light") {
  const errors = trackPageErrors(page);
  await page.goto(fixtureUrl(scenario, theme));
  await page.getByTestId("toolbar").waitFor();
  await page.evaluate(() => window.__dwFixture!.shellCommand({ kind: "navigate", route: "contacts" }));
  // The page's title takes focus in the next frame (shellBridge `focusPage`).
  await expect(page.getByTestId("page-contacts").locator("h1")).toBeFocused();
  return errors;
}

async function openAdd(page: Page, scenario = "dashpay-registered", theme: Theme = "light") {
  const errors = await openContacts(page, scenario, theme);
  await page.getByTestId("contacts-add").click();
  await expect(page.getByTestId("add-contact")).toBeVisible();
  return errors;
}

const add = (page: Page) => page.getByTestId("add-contact");
const card = (page: Page) => page.getByTestId("add-contact-candidate");
const tab = (page: Page, id: string) => page.getByTestId(`add-contact-tabs-${id}`);
const field = (page: Page) => page.getByRole("searchbox", { name: "Search username" });
const hit = (page: Page, name: string) => page.getByTestId("add-contact-hit").filter({ has: page.locator(".contact-row-name", { hasText: name }) });
const names = (page: Page, section: string) => page.getByTestId(`contacts-group-${section}`).locator(".contact-row-name");
const deliver = (page: Page, routes: LinkRoute[]) => page.evaluate((r) => window.__dwFixture!.deliverLinks(r), routes);

async function pick(page: Page, prefix: string, name: string) {
  await field(page).fill(prefix);
  await hit(page, name).click();
  await expect(card(page)).toBeVisible();
}

/** A paste into the Scan tab's field: `text`, or a fixture image of `image`'s codes. */
async function paste(page: Page, data: { text: string } | { image: string[] }) {
  await page.getByTestId("add-contact-paste").evaluate((el, d) => {
    const dt = new DataTransfer();
    if ("text" in d) dt.setData("text/plain", d.text);
    else dt.items.add(new File([["fixture-qr:", ...d.image].join("\n")], "pasted.png", { type: "image/png" }));
    el.dispatchEvent(new ClipboardEvent("paste", { clipboardData: dt, bubbles: true, cancelable: true }));
  }, data);
}

/** A file dropped on the Scan tab's drop area: a fixture image of `codes`, or raw `bytes`. */
async function drop(page: Page, content: string) {
  await page.getByTestId("add-contact-drop").evaluate((el, c) => {
    const dt = new DataTransfer();
    dt.items.add(new File([c], "code.png", { type: "image/png" }));
    for (const type of ["dragenter", "dragover", "drop"]) el.dispatchEvent(new DragEvent(type, { dataTransfer: dt, bubbles: true, cancelable: true }));
  }, content);
}

const qrImage = (...codes: string[]) => ["fixture-qr:", ...codes].join("\n");

test("IOS-073 finds a user by username and sends a contact request under a passphrase grant", async ({ page }) => {
  const errors = await openAdd(page);
  await expect(add(page).getByRole("heading", { level: 2 })).toHaveText("Add a New Contact");
  await expect(tab(page, "search")).toHaveAttribute("aria-selected", "true");
  await expect(page.getByTestId("add-contact-search-hint")).toBeVisible();
  // Every hit, with how it relates to the identity (D3).
  await field(page).fill("ma");
  await expect(page.getByTestId("add-contact-hit").locator(".contact-row-name")).toHaveText(["Mallory", "Marta", "mateo", "Maxine", "Mayumi"]);
  await field(page).fill("a");
  await expect(page.getByTestId("add-contact-search-hint")).toBeVisible();
  await pick(page, "mal", "Mallory");
  await expect(page.getByTestId("add-contact-name")).toBeFocused();
  await expect(page.getByTestId("add-contact-username")).toHaveText("@mallory");
  await expect(page.getByTestId("add-contact-verified")).toContainText("Verified on the Dash network");
  await expect(page.getByTestId("add-contact-verified")).toContainText("Found by username search");
  await page.getByTestId("add-contact-send").click();
  await expect(page.getByTestId("add-contact-send")).toHaveText("Sending…");
  await expect(page.getByTestId("grant-sheet")).toContainText("authorize this Dash Platform operation");
  await authorize(page);
  await expect(page.getByTestId("add-contact-outcome")).toHaveText("Contact request sent to Mallory.");
  // The re-read: the card says it went, the list shows it pending, and focus has a place (D5).
  await expect(page.getByTestId("add-contact-line")).toHaveText("Contact request sent.");
  await expect(page.getByTestId("add-contact-send")).toHaveCount(0);
  await expect(page.getByTestId("add-contact-status")).toBeFocused();
  // Newest first.
  await expect(names(page, "pending")).toHaveText(["Mallory", "Frank"]);
  await expect(hit(page, "Mallory").locator(".contact-row-note")).toHaveText("Request sent");
  // Show Request selects it, where DP2-09's pane acts.
  await page.getByTestId("add-contact-show").click();
  await expect(page.getByTestId("contact-detail-relation")).toHaveText("Request sent");
  expect(errors).toEqual([]);
});

test("IOS-073 the eligibility pre-check says why no request can go, with the one next step", async ({ page }) => {
  await openAdd(page);
  const line = page.getByTestId("add-contact-line");
  await pick(page, "bob", "Bob Builder");
  await expect(line).toHaveText("Already in your contacts.");
  await expect(page.getByTestId("add-contact-send")).toHaveCount(0);
  await expect(page.getByTestId("add-contact-show")).toHaveText("Show Contact");
  await pick(page, "grace", "Grace Hopper");
  await expect(line).toHaveText("They sent you a contact request.");
  await expect(page.getByTestId("add-contact-show")).toHaveText("Show Request");
  await pick(page, "frank", "Frank");
  await expect(line).toHaveText("Contact request sent.");
  await pick(page, "ivan", "Ivan");
  await expect(line).toHaveText("You ignored their contact request.");
  await expect(page.getByTestId("add-contact-show")).toHaveText("Show in Hidden");
  await pick(page, "alice", "Alice Liddell");
  await expect(line).toHaveText("This is you.");
  await expect(page.getByTestId("add-contact-show")).toHaveCount(0);
  await pick(page, "mayumi", "Mayumi");
  await expect(line).toHaveText("This user cannot receive contact requests yet.");
  await expect(page.getByTestId("add-contact-send")).toHaveCount(0);
  // Show Contact opens the contact's pane in the slot.
  await pick(page, "bob", "Bob Builder");
  await page.getByTestId("add-contact-show").click();
  await expect(page.getByTestId("contact-detail")).toBeVisible();
  await expect(add(page)).toHaveCount(0);
});

test("IOS-073 without DashPay keys the card offers Enable, not Send", async ({ page }) => {
  await openAdd(page, "dashpay-no-keys");
  await pick(page, "mal", "Mallory");
  await expect(page.getByTestId("add-contact-line")).toHaveText("Enable DashPay to send contact requests.");
  await page.getByTestId("add-contact-enable").click();
  await authorize(page);
  await expect(page.getByTestId("add-contact-send")).toBeVisible();
});

test("D3 a network failure says so with Try again; Close returns focus to Add contact", async ({ page }) => {
  await openAdd(page, "dashpay-search-offline");
  await field(page).fill("mal");
  await expect(page.getByTestId("add-contact-search-error")).toContainText("Could not search the Dash network.");
  await page.evaluate(() => (window.__dwFixture!.world.dashpay.searchOffline = false));
  await page.getByTestId("add-contact-search-error").getByRole("button", { name: "Try again" }).click();
  await expect(hit(page, "Mallory")).toBeVisible();
  await page.getByTestId("add-contact-close").click();
  await expect(add(page)).toHaveCount(0);
  await expect(page.getByTestId("contacts-add")).toBeFocused();
});

test("IOS-073 a Contacts search's network row opens its hit; Show more carries the search", async ({ page }) => {
  await openContacts(page);
  await page.getByRole("searchbox", { name: "Search for a contact" }).fill("ma");
  await page.getByTestId("contacts-network-more").click();
  await expect(field(page)).toHaveValue("ma");
  await expect(hit(page, "Mayumi")).toBeVisible();
  await page.getByTestId("contacts-network").getByTestId("network-row").filter({ hasText: "Marta" }).click();
  await expect(page.getByTestId("add-contact-name")).toHaveText("Marta");
  await expect(page.getByTestId("add-contact-identity")).toContainText(IDENTITY.marta);
});

test("IOS-073 a dashpay: link opens Add Contact verified; a link that does not verify has no Send", async ({ page }) => {
  const errors = await openContacts(page);
  await deliver(page, [{ kind: "addContact", identity: IDENTITY.mallory, username: "Mallory" }]);
  await expect(page.getByTestId("add-contact-name")).toHaveText("Mallory");
  // Platform's spelling, not the link's.
  await expect(page.getByTestId("add-contact-username")).toHaveText("@mallory");
  await expect(page.getByTestId("add-contact-verified")).toContainText("From a link");
  await expect(page.getByTestId("add-contact-send")).toBeVisible();

  // The username is someone else's.
  await deliver(page, [{ kind: "addContact", identity: IDENTITY.marta, username: "mallory" }]);
  await expect(page.getByTestId("add-contact-link-error")).toHaveText("This link couldn't be verified on the Dash network.");
  await expect(page.getByTestId("add-contact-username")).toHaveText("@mallory");
  await expect(card(page)).toHaveCount(0);
  await expect(page.getByTestId("add-contact-send")).toHaveCount(0);

  // An existing contact's link says so.
  await deliver(page, [{ kind: "addContact", identity: IDENTITY.bob, username: "bob" }]);
  await expect(page.getByTestId("add-contact-line")).toHaveText("Already in your contacts.");
  expect(errors).toEqual([]);
});

test("D13 a link while Platform is down says so, with Try again", async ({ page }) => {
  await openContacts(page, "dashpay-search-offline");
  await deliver(page, [{ kind: "addContact", identity: IDENTITY.mallory, username: "mallory" }]);
  const problem = page.getByTestId("add-contact-link-error");
  await expect(problem).toContainText("Could not reach Dash Platform.");
  await page.evaluate(() => (window.__dwFixture!.world.dashpay.searchOffline = false));
  await problem.getByRole("button", { name: "Try again" }).click();
  await expect(page.getByTestId("add-contact-name")).toHaveText("Mallory");
});

test("D13 BL-153 without an identity a link still verifies, and offers Join DashPay", async ({ page }) => {
  await openContacts(page, "dashpay-no-identity");
  await page.evaluate(
    (id) => {
      const dp = window.__dwFixture!.world.dashpay;
      dp.directory.push({ identity: id, username: "mallory", displayName: "Mallory", avatarUrl: null, relation: "stranger", unverified: false });
    },
    IDENTITY.mallory,
  );
  await deliver(page, [{ kind: "addContact", identity: IDENTITY.mallory, username: "mallory" }]);
  await expect(page.getByTestId("add-contact-name")).toHaveText("Mallory");
  await expect(page.getByTestId("add-contact-line")).toHaveText("Join DashPay to send contact requests.");
  await expect(page.getByTestId("add-contact-tabs")).toHaveCount(0);
  await page.getByTestId("add-contact-join").click();
  await expect(page.getByTestId("page-joinDashPay")).toBeVisible();
});

test("IOS-043 a pasted link or code is verified; text that is not one, or does not verify, says so", async ({ page }) => {
  const errors = await openAdd(page);
  await tab(page, "scan").click();
  await paste(page, { text: "hello" });
  await expect(page.getByTestId("add-contact-scan-error")).toHaveText("This is not a DashPay contact code.");
  await expect(page.getByTestId("add-contact-scan-error")).toBeFocused();
  await paste(page, { text: LINK(IDENTITY.marta, "mallory") });
  await expect(page.getByTestId("add-contact-scan-error")).toHaveText("This QR code couldn't be verified on the Dash network.");
  await paste(page, { text: `dash:?du=mallory&dapk=${NOW - 60}` });
  await expect(page.getByTestId("add-contact-scan-error")).toHaveText("This QR code has expired. Ask for a new one.");
  // The text never enters the field (it may carry a key, D16).
  await expect(page.getByTestId("add-contact-paste")).toHaveValue("");
  await paste(page, { text: LINK(IDENTITY.mallory, "mallory") });
  await expect(page.getByTestId("add-contact-name")).toHaveText("Mallory");
  await expect(page.getByTestId("add-contact-verified")).toContainText("From a QR code");
  // A typed link goes on Enter, and the field is cleared.
  await page.getByTestId("add-contact-paste").fill(LINK(IDENTITY.marta, "marta"));
  await page.getByTestId("add-contact-paste").press("Enter");
  await expect(page.getByTestId("add-contact-name")).toHaveText("Marta");
  await expect(page.getByTestId("add-contact-paste")).toHaveValue("");
  expect(errors).toEqual([]);
});

test("IOS-073 D7 an auto-accept code says so, and its send carries the claim once", async ({ page }) => {
  await openAdd(page);
  await tab(page, "scan").click();
  await paste(page, { text: `dash:?du=mallory&dapk=${NOW + 1800}` });
  await expect(page.getByTestId("add-contact-claim")).toHaveText(
    "This code asks them to accept you automatically. They confirm it when your request arrives. It expires in 30 min.",
  );
  // D16: the key went to verification only: not on the page, in its storage or in its URL.
  const kept = await page.evaluate(() => [document.documentElement.outerHTML, JSON.stringify({ ...localStorage }), JSON.stringify({ ...sessionStorage }), location.href].filter((t) => t.includes("dapk")));
  expect(kept).toEqual([]);
  await page.getByTestId("add-contact-send").click();
  await authorize(page);
  await expect(page.getByTestId("add-contact-outcome")).toHaveText("Contact request sent to Mallory.");
  // The send carried the claim: the fixture spent it.
  expect(await page.evaluate(() => window.__dwFixture!.world.dashpay.scans.map((s) => s.spent))).toEqual([true]);
});

test("IOS-043 D8 D9 dropped, pasted and opened images: the first contact code wins, others are said", async ({ page }) => {
  await openAdd(page);
  await tab(page, "scan").click();
  await drop(page, qrImage("dash:XpayRequest", LINK(IDENTITY.mallory, "mallory")));
  await expect(page.getByTestId("add-contact-name")).toHaveText("Mallory");
  await drop(page, "GIF89a");
  await expect(page.getByTestId("add-contact-scan-error")).toHaveText("This image could not be read. Use a PNG, JPEG or BMP file.");
  await drop(page, qrImage());
  await expect(page.getByTestId("add-contact-scan-error")).toHaveText("No QR code was found in this image.");
  await drop(page, qrImage("dash:XpayRequest"));
  await expect(page.getByTestId("add-contact-scan-error")).toHaveText("There is no DashPay contact code in this image.");
  await paste(page, { image: [LINK(IDENTITY.marta, "marta")] });
  await expect(page.getByTestId("add-contact-name")).toHaveText("Marta");
  // Open Image…: the dialog for images, then its decoded codes.
  await page.getByTestId("add-contact-open-image").click();
  await expect(page.getByTestId("add-contact-name")).toHaveText("Marta");
  await page.evaluate((img) => {
    window.__dwFixture!.world.tools.files.open = { name: "qr.png", data: btoa(img) };
  }, qrImage(LINK(IDENTITY.maxine, "maxine")));
  await page.getByTestId("add-contact-open-image").click();
  await expect(page.getByTestId("add-contact-name")).toHaveText("Maxine");
  const dialogs = await page.evaluate(() => window.__dwFixture!.world.tools.files.dialogs.map((d) => d.purpose));
  expect(dialogs).toEqual(["qrImage", "qrImage"]);
});

test("IOS-043 D10 Scan a Screen Region: the picker's region, its cancel, Cancel, and no picker", async ({ page }) => {
  await openAdd(page);
  await tab(page, "scan").click();
  await page.evaluate((img) => (window.__dwFixture!.world.scan.screen = btoa(img)), qrImage(LINK(IDENTITY.mallory, "mallory")));
  await page.getByTestId("add-contact-screen").click();
  await expect(page.getByTestId("add-contact-name")).toHaveText("Mallory");
  // The user still picking: Cancel stops it, nothing changes.
  await page.evaluate(() => (window.__dwFixture!.world.scan.holdScreen = true));
  await page.getByTestId("add-contact-screen").click();
  await expect(page.getByTestId("add-contact-scan-status")).toHaveText("Select a region of the screen…");
  await page.getByTestId("add-contact-screen-cancel").click();
  await expect(page.getByTestId("add-contact-screen")).toBeEnabled();
  await expect(page.getByTestId("add-contact-scan-status")).toHaveCount(0);
  await page.evaluate(() => {
    const scan = window.__dwFixture!.world.scan;
    scan.holdScreen = false;
    scan.screen = "unavailable";
  });
  await page.getByTestId("add-contact-screen").click();
  await expect(page.getByTestId("add-contact-scan-error")).toHaveText("This desktop has no screen capture. Take a screenshot, then open or paste it here.");
});

test("IOS-043 D10 on macOS the Scan tab points to the system's shortcut", async ({ page }) => {
  await openAdd(page, "dashpay-screen-shortcut");
  await tab(page, "scan").click();
  await expect(page.getByTestId("add-contact-screen")).toHaveCount(0);
  await expect(page.getByTestId("add-contact-screen-shortcut")).toHaveText("Press Control-Shift-Command-4 to copy a region of the screen, then paste it here.");
});

test("IOS-073 My QR shows the plain link as a QR code with Copy Link; no username says so", async ({ page }) => {
  await openAdd(page);
  await tab(page, "myQr").click();
  await expect(page.getByTestId("add-contact-qr")).toBeVisible();
  await expect(page.getByTestId("add-contact-qr-link")).toHaveText(LINK(IDENTITY.alice, "alice"));
  await expect(page.getByTestId("add-contact-copy-link")).toBeVisible();
  // The tabs as WAI-ARIA tabs: arrow keys move.
  await tab(page, "myQr").press("ArrowLeft");
  await expect(tab(page, "scan")).toBeFocused();
  await expect(tab(page, "scan")).toHaveAttribute("aria-selected", "true");

  await openAdd(page, "dashpay-no-username");
  await tab(page, "myQr").click();
  await expect(page.getByTestId("add-contact-qr-no-username")).toHaveText("Your username is not registered yet, so there is no link to share.");
});

test("D11 a file dropped outside the drop area does not navigate the page", async ({ page }) => {
  await openAdd(page);
  const url = page.url();
  await page.getByTestId("contacts-list").evaluate((el) => {
    const dt = new DataTransfer();
    dt.items.add(new File(["x"], "x.png", { type: "image/png" }));
    const e = new DragEvent("drop", { dataTransfer: dt, bubbles: true, cancelable: true });
    el.dispatchEvent(e);
    (window as unknown as { dropped: boolean }).dropped = e.defaultPrevented;
  });
  expect(await page.evaluate(() => (window as unknown as { dropped: boolean }).dropped)).toBe(true);
  expect(page.url()).toBe(url);
});

const STATES: Record<string, { scenario: string; go: (page: Page) => Promise<void> }> = {
  search: {
    scenario: "dashpay-registered",
    go: async (page) => {
      await page.getByTestId("contacts-add").click();
      await field(page).fill("ma");
      await hit(page, "Mayumi").waitFor();
    },
  },
  candidate: {
    scenario: "dashpay-registered",
    go: async (page) => {
      await page.getByTestId("contacts-add").click();
      await pick(page, "gra", "Grace Hopper");
      await page.getByTestId("add-contact-line").waitFor();
    },
  },
  send: {
    scenario: "dashpay-registered",
    go: async (page) => {
      await page.getByTestId("contacts-add").click();
      await pick(page, "mal", "Mallory");
      await page.getByTestId("add-contact-send").waitFor();
    },
  },
  "scan-error": {
    scenario: "dashpay-registered",
    go: async (page) => {
      await page.getByTestId("contacts-add").click();
      await tab(page, "scan").click();
      await paste(page, { text: "hello" });
      await page.getByTestId("add-contact-scan-error").waitFor();
    },
  },
  "my-qr": {
    scenario: "dashpay-registered",
    go: async (page) => {
      await page.getByTestId("contacts-add").click();
      await tab(page, "myQr").click();
      await page.getByTestId("add-contact-qr").waitFor();
    },
  },
  "link-join": {
    scenario: "dashpay-no-identity",
    go: async (page) => {
      await deliver(page, [{ kind: "addContact", identity: IDENTITY.mallory, username: "mallory" }]);
      await page.getByTestId("add-contact-link-error").waitFor();
    },
  },
};

for (const theme of THEMES) {
  for (const [name, { scenario, go }] of Object.entries(STATES)) {
    test(`add contact ${name} ${theme}: 0 serious axe issues`, async ({ page }, info) => {
      const errors = await openContacts(page, scenario, theme);
      await go(page);
      await fontsReady(page);
      await page.screenshot({ path: artifactPath(info, `${theme}-add-contact-${name}.png`) });
      expect(await seriousAxeViolations(page)).toEqual([]);
      expect(errors).toEqual([]);
    });
  }
}
