import { expect, test, type Locator, type Page } from "@playwright/test";
import { fileDialogAnswers, WIDE_PNG } from "./editProfile";
import { authorize, fixtureUrl, THEMES, trackPageErrors } from "./helpers";
import { checkArea, PSEUDO } from "./pseudo";

// Localization (MP-14): My Profile and Edit Profile under the two pseudo-locales (./pseudo.ts):
// the page's states (P3–P5, C2–C7, R1–R6) and the sheet's (E1–E9, U3, G1, K1, Q1), reached as
// profile.spec.ts and edit-profile.spec.ts reach them.

/** Amounts, usernames, the identity id, the fixture's profile text and dates. */
const DATA = [
  /^[+-]?[\p{Nd}.,٫٬\s]+( (t?DASH|mDASH|μDASH|duffs))?$/u,
  /^@?[a-z0-9-]+$/,
  /^[1-9A-HJ-NP-Za-km-z]{40,}$/,
  /^(Alice Liddell|Down the rabbit hole)$/,
  /^[A-Z][a-z]{2} \d{1,2}, \d{4}$/,
  /^[a-z]+\.[a-z_]+$/,
];

const command = (page: Page, c: object) => page.evaluate((c) => window.__dwFixture!.shellCommand(c as never), c);

async function toProfile(page: Page) {
  await page.getByTestId("toolbar").waitFor();
  await command(page, { kind: "navigate", route: "profile" });
  await page.getByTestId("page-profile").waitFor();
}

async function toEditor(page: Page) {
  await toProfile(page);
  await expect(page.getByTestId("profile-name")).toBeVisible();
  await page.getByTestId("profile-edit").click();
  await expect(page.getByTestId("edit-profile-body")).toHaveAttribute("data-step", "form");
}

const body = (p: Page) => p.getByTestId("profile-body");
const credits = (p: Page) => p.getByTestId("profile-credits");
const request = (p: Page) => p.getByTestId("profile-request");
const name = (p: Page) => p.getByTestId("edit-profile-name");

interface State {
  scenario: string;
  extra?: string;
  roots: string[];
  go: (page: Page) => Promise<void>;
}

const PAGE = ["page-profile"];
const SHEET = ["edit-profile-sheet"];
const page = (scenario: string, ready: (p: Page) => Promise<void>, extra?: string): State => ({
  scenario,
  extra,
  roots: PAGE,
  go: async (p) => {
    await toProfile(p);
    await ready(p);
  },
});
const sheet = (scenario: string, reach: (p: Page) => Promise<void>, roots = SHEET): State => ({
  scenario,
  roots,
  go: async (p) => {
    await toEditor(p);
    await reach(p);
  },
});

export const STATES: Record<string, State> = {
  P3: page("dashpay-no-identity", (p) => expect(body(p)).toHaveAttribute("data-state", "noProfile")),
  P4: page("dashpay-registered", (p) => expect(p.getByTestId("profile-outcomes")).toBeVisible()),
  P5: page("dashpay-no-username", (p) => expect(p.getByTestId("profile-choose-name")).toBeVisible()),
  "P5-unknown": page("dashpay-names-unknown", (p) => expect(p.getByTestId("profile-username")).toHaveAttribute("data-names", "unknown")),
  C2: page("dashpay-credits-low", (p) => expect(credits(p)).toHaveAttribute("data-level", "low")),
  C3: page("dashpay-low-credits", (p) => expect(credits(p)).toHaveAttribute("data-level", "empty")),
  C4: page("dashpay-credits-unknown", (p) => expect(credits(p)).toHaveAttribute("data-level", "unknown")),
  C6: page("dashpay-top-up-pending", (p) => expect(p.getByTestId("profile-pending-finish")).toBeVisible()),
  "C6-unknown": page("dashpay-top-up-unknown", (p) => expect(p.getByTestId("profile-pending-arrived")).toBeVisible()),
  C7: page("dashpay-top-up-stuck", (p) => expect(p.getByTestId("profile-stuck")).toBeVisible()),
  R1: page("dashpay-contest-pending", (p) => expect(p.getByTestId("profile-tally")).toBeVisible()),
  "R1-instant": page("dashpay-contest-no-temporary", (p) => expect(p.getByTestId("profile-instant")).toBeVisible()),
  R3: page("dashpay-contest-won", (p) => expect(request(p)).toHaveAttribute("data-state", "won")),
  R4: page("dashpay-contest-lost", (p) => expect(request(p)).toHaveAttribute("data-state", "lost")),
  R5: page("dashpay-contest-locked", (p) => expect(request(p)).toHaveAttribute("data-state", "locked")),
  R6: page("dashpay-contest-pending", (p) => expect(p.getByTestId("profile-request-failed")).toBeVisible(), "&fail=contest_status:none"),
  E1: sheet("dashpay-avatar", (p) => expect(p.getByTestId("edit-profile-save")).toBeDisabled()),
  E3: sheet("dashpay-avatar", async (p) => {
    await name(p).fill("x".repeat(26));
    await expect(p.getByTestId("edit-profile-name-count")).toHaveAttribute("class", /is-over/);
  }),
  E6: sheet("dashpay-profile-save-error", async (p) => {
    await name(p).fill("Alice");
    await p.getByTestId("edit-profile-save").click();
    await authorize(p);
    await expect(p.getByTestId("edit-profile-error")).toBeVisible();
  }),
  E9: sheet("dashpay-pictures-off", (p) => expect(p.getByTestId("edit-profile-pictures-off")).toBeVisible()),
  U3: sheet("dashpay-avatar", async (p) => {
    await p.getByTestId("edit-profile-url").click();
    await p.getByTestId("edit-profile-url-field").fill("https://example.org/missing.png");
    await p.getByTestId("edit-profile-url-fetch").click();
    await expect(p.getByTestId("edit-profile-step-error")).toBeVisible();
  }),
  G1: sheet("dashpay-avatar", async (p) => {
    await p.getByTestId("edit-profile-gravatar").click();
    await expect(p.getByTestId("edit-profile-gravatar-field")).toBeVisible();
  }),
  K1: sheet("dashpay-avatar-upload", async (p) => {
    await fileDialogAnswers(p, WIDE_PNG);
    await p.getByTestId("edit-profile-file").click();
    await expect(p.getByTestId("edit-profile-crop-frame")).toHaveAttribute("data-crop", /\d/);
  }),
  Q1: sheet(
    "dashpay-avatar",
    async (p) => {
      await name(p).fill("Alice");
      await p.keyboard.press("Escape");
      await expect(p.getByTestId("edit-profile-discard")).toBeVisible();
    },
    ["edit-profile-discard"],
  ),
};

for (const theme of THEMES)
  for (const pseudo of PSEUDO)
    for (const [name, state] of Object.entries(STATES))
      test(`DashPay profile ${name} shows only catalog text under ${pseudo[0]}, fits, and passes axe, ${theme}`, async ({ page }, info) => {
        test.setTimeout(120_000);
        const errors = trackPageErrors(page);
        await checkArea(page, info, theme, pseudo, {
          name: `dashpay-profile-${name}`,
          roots: state.roots,
          data: DATA,
          open: async (lang) => {
            await page.goto(fixtureUrl(state.scenario, theme, `${state.extra ?? ""}&lang=${lang}`));
            await state.go(page);
          },
        });
        expect(errors).toEqual([]);
      });

test("My Profile and Edit Profile follow a language change with no reload", async ({ page }) => {
  const errors = trackPageErrors(page);
  await page.goto(fixtureUrl("dashpay-registered", "light", "&lang=en"));
  await toProfile(page);
  await expect(page.getByTestId("profile-edit")).toHaveText("Edit Profile");
  await command(page, { kind: "navigate", route: "settings" });
  await page.getByTestId("settings-language").selectOption("en-XA");
  await toEditor(page);
  await expect(page.getByTestId("profile-edit")).toHaveText(/^⟦.+⟧$/);
  await expect(page.getByTestId("edit-profile-name-count")).toHaveText(/^⟦.+⟧$/);
  expect(errors).toEqual([]);
});

/** The left edges of `re`'s two groups in the first text node under `root` that matches it. */
async function lefts(root: Locator, re: RegExp): Promise<[number, number] | null> {
  return root.evaluate(
    (el, [source, flags]) => {
      const re = new RegExp(source, flags);
      const walker = document.createTreeWalker(el, NodeFilter.SHOW_TEXT);
      for (let n = walker.nextNode(); n; n = walker.nextNode()) {
        const m = re.exec(n.textContent ?? "");
        if (!m?.indices) continue;
        const left = (g: number) => {
          const r = document.createRange();
          r.setStart(n!, m.indices![g]![0]);
          r.setEnd(n!, m.indices![g]![0] + 1);
          return r.getBoundingClientRect().left;
        };
        return [left(1), left(2)] as [number, number];
      }
      return null;
    },
    [re.source, `${re.flags}d`] as const,
  );
}

/** A username's "@" and its first letter: the "@" is drawn first. */
const AT_NAME = /(@)([a-z])/;

async function inOrder(root: Locator, what: string, re: RegExp = AT_NAME) {
  await expect(root).toContainText("@");
  const at = await lefts(root, re);
  expect(at, what).not.toBeNull();
  expect(at![0], what).toBeLessThan(at![1]);
}

test.describe("usernames and amounts keep their order under ar-XB", () => {
  const open = (page: Page, scenario: string) => page.goto(fixtureUrl(scenario, "light", "&lang=ar-XB"));

  test("the header's username", async ({ page }) => {
    await open(page, "dashpay-registered");
    await toProfile(page);
    await inOrder(page.getByTestId("profile-username"), "the header");
  });

  test("the Usernames row", async ({ page }) => {
    await open(page, "dashpay-registered");
    await toProfile(page);
    await inOrder(page.getByTestId("profile-usernames"), "the Usernames row");
  });

  test("the header's requested name while its vote runs", async ({ page }) => {
    await open(page, "dashpay-contest-no-temporary");
    await toProfile(page);
    await expect(page.getByTestId("profile-instant")).toBeVisible();
    await inOrder(page.getByTestId("profile-username"), "the requested name");
  });

  test("the request card's title once the vote is won", async ({ page }) => {
    await open(page, "dashpay-contest-won");
    await toProfile(page);
    await expect(page.getByTestId("profile-request")).toHaveAttribute("data-state", "won");
    await inOrder(page.getByTestId("profile-request").locator(".card-title"), "the won title");
  });

  test("a username voting result", async ({ page }) => {
    await open(page, "dashpay-registered");
    await toProfile(page);
    await inOrder(page.getByTestId("profile-outcomes").locator("li").first(), "the result");
  });

  test("Settings' DashPay card", async ({ page }) => {
    await open(page, "dashpay-registered");
    await page.getByTestId("toolbar").waitFor();
    await command(page, { kind: "navigate", route: "settings" });
    await inOrder(page.getByTestId("settings-dashpay-who"), "the settings card");
  });

  test("the credit balance's amount comes before its unit", async ({ page }) => {
    await open(page, "dashpay-registered");
    await toProfile(page);
    const figure = page.getByTestId("profile-credits-figure");
    await expect(figure).toContainText(/\p{Nd}/u);
    const at = await lefts(figure, /(\p{Nd})[\p{Nd}.,٫٬]* (t?DASH)/u);
    expect(at, "the balance").not.toBeNull();
    expect(at![0], "the balance").toBeLessThan(at![1]);
  });

  // Regression pin: the count is one value, so the typed count stays before the limit.
  test("Edit Profile's count reads typed/limit", async ({ page }) => {
    await open(page, "dashpay-avatar");
    await toEditor(page);
    await name(page).fill("Alice");
    const count = page.getByTestId("edit-profile-name-count");
    await expect(count).toContainText("5");
    const at = await lefts(count, /(5)\/(2)/);
    expect(at, "the count").not.toBeNull();
    expect(at![0], "the count").toBeLessThan(at![1]);
  });
});
