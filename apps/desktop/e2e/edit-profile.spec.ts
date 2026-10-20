import { expect, test, type Page } from "@playwright/test";
import { seriousAxeViolations } from "./axe";
import { authorize, fixtureUrl, fontsReady, THEMES, trackPageErrors, type Theme } from "./helpers";

// DP4-03: Edit Profile over My Profile on the fixture backend, by parity ID: IOS-076 (edit the
// display name, about text and picture), Android's AND-095 (the counters and the unsaved-changes
// prompt), AND-096 (the picture sources: public URL, Gravatar, a file with its square crop) and
// AND-097 (a shared image; here a dropped or pasted one). The design note's states: E1–E10,
// U1–U3, G1–G2, K1–K3 and Q1.

/** A 60 × 40 PNG, blue on the left half and orange on the right: the crop's file. */
const WIDE_PNG =
  "iVBORw0KGgoAAAANSUhEUgAAADwAAAAoCAIAAAAt2Q6oAAAAQUlEQVR42u3OMQ0AMAwDsAApxL1jN06lMBT5LBmAM/eV7ElJpKWlpaWlpaWlpaWlpaWlpaWlpaWlpaWlpaWlm+kPlHd0NRnvUxsAAAAASUVORK5CYII=";

async function openEditor(page: Page, scenario = "dashpay-avatar", theme: Theme = "light", extra = "") {
  const errors = trackPageErrors(page);
  await page.goto(fixtureUrl(scenario, theme, extra));
  await page.getByTestId("toolbar").waitFor();
  await page.evaluate(() => window.__dwFixture!.shellCommand({ kind: "navigate", route: "profile" }));
  await expect(page.getByTestId("profile-name")).toBeVisible();
  await page.getByTestId("profile-edit").click();
  await expect(body(page)).toHaveAttribute("data-step", "form");
  return errors;
}

const body = (page: Page) => page.getByTestId("edit-profile-body");
const sheet = (page: Page) => page.getByTestId("edit-profile-sheet");
const name = (page: Page) => page.getByTestId("edit-profile-name");
const save = (page: Page) => page.getByTestId("edit-profile-save");
const calls = (page: Page, cmd: string) => page.evaluate((c) => window.__dwFixture!.calls.filter((x) => x === c).length, cmd);

/** Puts the file the next native dialog answers with. */
const fileDialogAnswers = (page: Page, data: string) =>
  page.evaluate((data) => (window.__dwFixture!.world.tools.files.open = { name: "me.png", data }), data);

/** Each sheet state and how to reach it from the open form. */
const STATES: Record<string, { scenario?: string; extra?: string; reach: (page: Page) => Promise<void> }> = {
  E1: { reach: async (p) => expect(save(p)).toBeDisabled() },
  "E1-no-picture": { scenario: "dashpay-registered", reach: async (p) => expect(p.getByTestId("edit-profile-remove")).toHaveCount(0) },
  E3: {
    reach: async (p) => {
      await name(p).fill("x".repeat(26));
      await expect(p.getByTestId("edit-profile-name-count")).toHaveText("26/25 characters. At most 25 characters.");
    },
  },
  E4: {
    reach: async (p) => {
      await p.getByTestId("edit-profile-remove").click();
      await expect(p.getByTestId("edit-profile-picture")).toHaveAttribute("data-choice", "remove");
    },
  },
  E6: {
    scenario: "dashpay-profile-save-error",
    reach: async (p) => {
      await name(p).fill("Alice");
      await save(p).click();
      await authorize(p);
      await expect(p.getByTestId("edit-profile-error")).toBeVisible();
    },
  },
  E9: { scenario: "dashpay-pictures-off", reach: async (p) => expect(p.getByTestId("edit-profile-pictures-off")).toBeVisible() },
  U1: {
    reach: async (p) => {
      await p.getByTestId("edit-profile-url").click();
      await expect(body(p)).toHaveAttribute("data-step", "url");
    },
  },
  U3: {
    reach: async (p) => {
      await p.getByTestId("edit-profile-url").click();
      await p.getByTestId("edit-profile-url-field").fill("https://example.org/missing.png");
      await p.getByTestId("edit-profile-url-fetch").click();
      await expect(p.getByTestId("edit-profile-step-error")).toHaveText("Unable to fetch image. Please enter a valid image URL.");
    },
  },
  G1: {
    reach: async (p) => {
      await p.getByTestId("edit-profile-gravatar").click();
      await expect(p.getByTestId("edit-profile-gravatar-field")).toBeVisible();
    },
  },
  K1: {
    scenario: "dashpay-avatar-upload",
    reach: async (p) => {
      await fileDialogAnswers(p, WIDE_PNG);
      await p.getByTestId("edit-profile-file").click();
      await expect(p.getByTestId("edit-profile-crop-frame")).toHaveAttribute("data-crop", "10,0,40");
    },
  },
  Q1: {
    reach: async (p) => {
      await name(p).fill("Alice");
      await p.keyboard.press("Escape");
      await expect(p.getByTestId("edit-profile-discard")).toBeVisible();
    },
  },
};

for (const theme of THEMES) {
  for (const [state, { scenario, extra, reach }] of Object.entries(STATES)) {
    test(`IOS-076 / AND-095…097 Edit Profile ${state} ${theme}: renders, 0 serious axe issues`, async ({ page }) => {
      const errors = await openEditor(page, scenario, theme, extra);
      await reach(page);
      await fontsReady(page);
      expect(await seriousAxeViolations(page)).toEqual([]);
      expect(errors).toEqual([]);
    });
  }
}

test("IOS-076 saves the name and about text under a grant; My Profile shows them", async ({ page }) => {
  const errors = await openEditor(page);
  await expect(name(page)).toHaveValue("Alice Liddell");
  await name(page).fill("  Alice L. ");
  await page.getByTestId("edit-profile-message").fill("Through the looking glass");
  await save(page).click();
  await authorize(page);
  await expect(sheet(page)).toHaveCount(0);
  await expect(page.locator(".toast")).toHaveText("Your profile was saved.");
  await expect(page.getByTestId("profile-name")).toHaveText("Alice L.");
  await expect(page.getByTestId("profile-header")).toContainText("Through the looking glass");
  // The picture was kept.
  await expect(page.getByTestId("profile-header").locator(".contact-avatar img")).toHaveCount(1);
  expect(errors).toEqual([]);
});

test("E5: a save in flight cannot be submitted twice, nor edited or closed", async ({ page }) => {
  await openEditor(page);
  await name(page).fill("Alice");
  const release = await page.evaluateHandle(() => window.__dwFixture!.hold("dashpay_update_profile"));
  await save(page).click();
  await authorize(page);
  await expect(body(page)).toHaveAttribute("data-saving", "publish");
  await expect(page.getByTestId("edit-profile-saving")).toHaveText("Saving…");
  await expect(save(page)).toBeDisabled();
  await expect(name(page)).toBeDisabled();
  await page.keyboard.press("Escape");
  await expect(sheet(page)).toBeVisible();
  await expect(page.getByTestId("edit-profile-discard")).toHaveCount(0);
  await release.evaluate((r) => r());
  await expect(sheet(page)).toHaveCount(0);
  expect(await calls(page, "dashpay_update_profile")).toBe(1);
});

test("AND-096 E7: a failed save keeps the edits and says the engine's reason; a retry saves", async ({ page }) => {
  await openEditor(page, "dashpay-profile-save-error");
  await name(page).fill("Alice");
  await page.getByTestId("edit-profile-remove").click();
  await save(page).click();
  await authorize(page);
  await expect(page.getByTestId("edit-profile-error")).toHaveText("Could not reach Dash Platform. Try again.");
  await expect(name(page)).toHaveValue("Alice");
  await expect(page.getByTestId("edit-profile-picture")).toHaveAttribute("data-choice", "remove");
  await save(page).click();
  await authorize(page);
  await expect(sheet(page)).toHaveCount(0);
  await expect(page.getByTestId("profile-name")).toHaveText("Alice");
  await expect(page.getByTestId("profile-header").locator(".contact-avatar img")).toHaveCount(0);
});

test("D10 E8: a profile changed on another device is said; the next save replaces it", async ({ page }) => {
  await openEditor(page, "dashpay-avatar");
  // Another device changed the about text after the sheet opened.
  await page.evaluate(() => {
    const me = window.__dwFixture!.world.dashpay.identities[0];
    me.profile = { ...me.profile!, publicMessage: "Edited elsewhere" };
  });
  await name(page).fill("Alice");
  await save(page).click();
  await authorize(page);
  await expect(page.getByTestId("edit-profile-error")).toContainText("changed on another device");
  await expect(name(page)).toHaveValue("Alice");
  await save(page).click();
  await authorize(page);
  await expect(sheet(page)).toHaveCount(0);
  await expect(page.getByTestId("profile-name")).toHaveText("Alice");
  await expect(page.getByTestId("profile-header")).toContainText("Down the rabbit hole");
});

test("AND-095 the counters follow the text; over a limit or empty cannot be saved", async ({ page }) => {
  await openEditor(page, "dashpay-registered");
  await name(page).fill("😀".repeat(25));
  await expect(page.getByTestId("edit-profile-name-count")).toHaveText("25/25 characters");
  await expect(save(page)).toBeEnabled();
  await name(page).fill("😀".repeat(26));
  await expect(name(page)).toHaveAttribute("aria-invalid", "true");
  await expect(save(page)).toBeDisabled();
  await name(page).fill("");
  await page.getByTestId("edit-profile-message").fill("");
  await expect(page.getByTestId("edit-profile-empty")).toBeVisible();
  await expect(save(page)).toBeDisabled();
});

test("AND-095 Q1: closing with changes asks; Keep Editing keeps them, Discard drops them", async ({ page }) => {
  await openEditor(page);
  await page.getByTestId("edit-profile-cancel").click();
  await expect(sheet(page)).toHaveCount(0);
  await page.getByTestId("profile-edit").click();
  await name(page).fill("Alice");
  await page.getByTestId("edit-profile-cancel").click();
  await page.getByTestId("edit-profile-keep").click();
  await expect(name(page)).toHaveValue("Alice");
  await expect(name(page)).toBeVisible();
  await page.keyboard.press("Escape");
  await page.getByTestId("edit-profile-discard-confirm").click();
  await expect(sheet(page)).toHaveCount(0);
  await expect(page.getByTestId("profile-name")).toHaveText("Alice Liddell");
  await page.getByTestId("profile-edit").click();
  await expect(name(page)).toHaveValue("Alice Liddell");
});

test("AND-095 D11: leaving My Profile with changes asks first, from a shortcut or the menu", async ({ page }) => {
  await openEditor(page);
  await name(page).fill("Alice");
  await page.keyboard.press("Control+5");
  await expect(page.getByTestId("edit-profile-discard")).toBeVisible();
  await page.getByTestId("edit-profile-keep").click();
  await expect(page.getByTestId("page-profile")).toBeVisible();
  await page.evaluate(() => window.__dwFixture!.shellCommand({ kind: "navigate", route: "pay" }));
  await page.getByTestId("edit-profile-discard-confirm").click();
  await expect(page.getByTestId("page-pay")).toBeVisible();
  // Without changes the sheet just closes as the page goes.
  await page.evaluate(() => window.__dwFixture!.shellCommand({ kind: "navigate", route: "profile" }));
  await page.getByTestId("profile-edit").click();
  await page.evaluate(() => window.__dwFixture!.shellCommand({ kind: "navigate", route: "pay" }));
  await expect(page.getByTestId("page-pay")).toBeVisible();
  await expect(sheet(page)).toHaveCount(0);
});

test("AND-096 U1–U3: a public URL's picture is previewed, then published", async ({ page }) => {
  await openEditor(page, "dashpay-registered");
  await page.getByTestId("edit-profile-url").click();
  await page.getByTestId("edit-profile-url-field").fill("https://example.org/a.webp");
  await page.getByTestId("edit-profile-url-field").press("Enter");
  await expect(page.getByTestId("edit-profile-step-error")).toHaveText("Use a PNG, JPEG or GIF picture.");
  await page.getByTestId("edit-profile-url-field").fill("https://example.org/me.png");
  await page.getByTestId("edit-profile-url-fetch").click();
  await expect(body(page)).toHaveAttribute("data-step", "form");
  await expect(page.getByTestId("edit-profile-picture")).toHaveAttribute("data-choice", "new");
  await expect(page.getByTestId("edit-profile-picture").locator("img")).toHaveAttribute("src", /^data:image\/png;base64,/);
  await save(page).click();
  await authorize(page);
  await expect(sheet(page)).toHaveCount(0);
  await expect(page.getByTestId("profile-header").locator(".contact-avatar img")).toHaveCount(1);
  const profile = await page.evaluate(() => window.__dwFixture!.world.dashpay.identities[0].profile);
  expect(profile?.avatarUrl).toBe("https://example.org/me.png");
});

test("AND-096 G1–G2 D15: the Gravatar address leaves the field when the fetch starts", async ({ page }) => {
  await openEditor(page, "dashpay-registered");
  await page.getByTestId("edit-profile-gravatar").click();
  await page.getByTestId("edit-profile-gravatar-field").fill("nobody@example.org");
  await page.getByTestId("edit-profile-gravatar-fetch").click();
  await expect(page.getByTestId("edit-profile-step-error")).toHaveText("Unable to fetch your Gravatar. Please enter a valid Gravatar email ID.");
  await expect(page.getByTestId("edit-profile-gravatar-field")).toHaveValue("");
  await page.getByTestId("edit-profile-gravatar-field").fill("alice@example.org");
  await page.getByTestId("edit-profile-gravatar-fetch").click();
  await expect(page.getByTestId("edit-profile-picture")).toHaveAttribute("data-choice", "new");
  await page.getByTestId("edit-profile-undo").click();
  await expect(page.getByTestId("edit-profile-picture")).toHaveAttribute("data-choice", "keep");
  await expect(save(page)).toBeDisabled();
});

test("AND-096 K1–K3 D4/D5: a file is cropped square; only the crop is prepared, uploaded once, then published", async ({ page }) => {
  await openEditor(page, "dashpay-avatar-upload");
  await expect(page.getByTestId("edit-profile-file-hint")).toHaveText("Drop or paste a picture here");
  await fileDialogAnswers(page, WIDE_PNG);
  await page.getByTestId("edit-profile-file").click();
  const frame = page.getByTestId("edit-profile-crop-frame");
  await expect(frame).toHaveAttribute("data-crop", "10,0,40");
  await expect(frame).toBeFocused();
  // A tenth of the frame to the left, by key; then a drag back to the right edge.
  await frame.press("ArrowLeft");
  await expect(frame).toHaveAttribute("data-crop", "6,0,40");
  const box = (await frame.boundingBox())!;
  await page.mouse.move(box.x + 120, box.y + 120);
  await page.mouse.down();
  await page.mouse.move(box.x - 300, box.y + 120, { steps: 4 });
  await page.mouse.up();
  await expect(frame).toHaveAttribute("data-crop", "20,0,40");
  await frame.press("+");
  await expect(frame).toHaveAttribute("data-crop", "24,4,32");
  await page.getByTestId("edit-profile-crop-use").click();
  await expect(page.getByTestId("edit-profile-picture")).toHaveAttribute("data-choice", "new");
  const prepared = await page.evaluate(() => window.__dwFixture!.world.dashpay.pictures!.prepares);
  expect(prepared).toEqual([{ kind: "file", bytes: WIDE_PNG, crop: { x: 24, y: 4, width: 32, height: 32 } }]);
  await save(page).click();
  await authorize(page);
  await expect(sheet(page)).toHaveCount(0);
  const order = await page.evaluate(() => window.__dwFixture!.calls.filter((c) => c === "dashpay_upload_avatar" || c === "dashpay_update_profile"));
  expect(order).toEqual(["dashpay_upload_avatar", "dashpay_update_profile"]);
  const profile = await page.evaluate(() => window.__dwFixture!.world.dashpay.identities[0].profile);
  expect(profile?.avatarUrl).toMatch(/^https:\/\/i\.imgur\.com\//);
});

test("AND-097 D2: a picture dropped on the picture block opens the crop", async ({ page }) => {
  await openEditor(page, "dashpay-avatar-upload");
  await page.getByTestId("edit-profile-picture").evaluate((el, b64) => {
    const bytes = Uint8Array.from(atob(b64), (c) => c.charCodeAt(0));
    const data = new DataTransfer();
    data.items.add(new File([bytes], "dropped.png", { type: "image/png" }));
    for (const type of ["dragover", "drop"]) el.dispatchEvent(new DragEvent(type, { bubbles: true, cancelable: true, dataTransfer: data }));
  }, WIDE_PNG);
  await expect(page.getByTestId("edit-profile-crop-frame")).toHaveAttribute("data-crop", "10,0,40");
});

test("D16 a file that is not a picture is refused in the form, before any crop", async ({ page }) => {
  await openEditor(page, "dashpay-avatar-upload");
  await fileDialogAnswers(page, btoa("RIFF....WEBP"));
  await page.getByTestId("edit-profile-file").click();
  await expect(page.getByTestId("edit-profile-picture-error")).toHaveText("Use a PNG, JPEG or GIF picture.");
  await expect(body(page)).toHaveAttribute("data-step", "form");
  expect(await calls(page, "dashpay_prepare_avatar")).toBe(0);
});

test("D5 without an upload host a file cannot be chosen", async ({ page }) => {
  await openEditor(page, "dashpay-registered");
  await expect(page.getByTestId("edit-profile-file")).toBeDisabled();
  await expect(page.getByTestId("edit-profile-file-hint")).toContainText("need an upload service");
});

test("D13 E9: with pictures off no source is offered and nothing is fetched; Open Settings goes there", async ({ page }) => {
  await openEditor(page, "dashpay-pictures-off");
  await expect(page.getByTestId("edit-profile-url")).toHaveCount(0);
  await expect(page.getByTestId("edit-profile-gravatar")).toHaveCount(0);
  // The own picture still shows, from the engine's cache.
  await expect(page.getByTestId("edit-profile-picture").locator("img")).toHaveCount(1);
  await page.getByTestId("edit-profile-open-settings").click();
  await expect(page.getByTestId("settings-contact-pictures")).toBeVisible();
  expect(await calls(page, "dashpay_prepare_avatar")).toBe(0);
});
