import { expect, test, type Locator, type Page } from "@playwright/test";
import { MAIN_PHRASE } from "../src/fixture/vault";
import { CARDS } from "./dashpay";
import { authorize, fixtureUrl, THEMES, trackPageErrors } from "./helpers";
import { checkArea, overflow, PANES, PSEUDO, untranslated } from "./pseudo";

// Localization (MP-14): DashPay's join and registration under the two pseudo-locales (./pseudo.ts):
// every state of the DashPay card, the Join page and its sheets, each screen of the registration
// wizard, the welcome and Share my username, and the invitation claim's states.

/** Amounts, usernames, identities, a profile's display name and the engine's codes. */
const DATA = [
  /^[+-]?[\p{Nd}.,٫٬\s]+( (t?DASH|mDASH|μDASH|duffs))?$/u,
  /^(t?DASH)$/,
  /^@?[a-z0-9-]+$/,
  /^[1-9A-HJ-NP-Za-km-z]{40,}$/,
  /^Bob Builder$/,
  /^[a-z]+\.[a-z_]+$/,
  /^›$/,
  /^https?:\/\/\S+$/,
  /^dashpay:\/\/\S+$/,
];

const LABEL = "alice-77";

const toolbar = (page: Page) => page.getByTestId("toolbar").waitFor();
const screen = (page: Page) => page.getByTestId("registration-screen");
const command = (page: Page, c: object) => page.evaluate((c) => window.__dwFixture!.shellCommand(c as never), c);
const draftOf = (page: Page) => page.evaluate(() => window.__dwFixture!.world.dashpay.registrations.at(-1)!.draft);
const complete = (page: Page, draft: string) =>
  page.evaluate((d) => {
    while (window.__dwFixture!.advanceRegistration(d));
  }, draft);

async function toJoin(page: Page) {
  await toolbar(page);
  await command(page, { kind: "navigate", route: "joinDashPay" });
  await page.getByTestId("page-joinDashPay").waitFor();
}

async function nameStep(page: Page) {
  await toJoin(page);
  await page.getByTestId("join-create").click();
  await page.getByTestId("registration-name").waitFor();
}

async function typed(page: Page, label: string, availability: RegExp) {
  await nameStep(page);
  await page.getByTestId("registration-name-field").fill(label);
  await expect(page.getByTestId("registration-availability")).toHaveAttribute("data-tone", availability);
}

async function confirm(page: Page, label: string) {
  await nameStep(page);
  await page.getByTestId("registration-name-field").fill(label);
  await expect(page.getByTestId("registration-next")).toBeEnabled();
  await page.getByTestId("registration-next").click();
  await page.getByTestId("registration-confirm").waitFor();
}

async function started(page: Page, label = LABEL) {
  await confirm(page, label);
  if (label === "alice") await page.getByTestId("registration-accept").check();
  await page.getByTestId("registration-pay").click();
  await authorize(page);
}

async function onScreen(page: Page, kind: string) {
  await toJoin(page);
  await expect(screen(page)).toHaveAttribute("data-kind", kind);
}

/** The launch's "Links you opened", its invitation opened: the claim screen. */
async function claimScreen(page: Page, state: string, status?: object) {
  await page.getByTestId("saved-links").waitFor();
  if (status)
    await page.evaluate((s) => {
      const w = window.__dwFixture!.world;
      w.invitations = { [w.stash[0].info.id]: s as never };
    }, status);
  await page.getByTestId("saved-link").filter({ has: page.getByTestId("saved-link-open") }).first().getByTestId("saved-link-open").click();
  await expect(page.getByTestId("claim")).toHaveAttribute("data-state", state);
}

interface State {
  scenario: string;
  extra?: string;
  roots: string[];
  go: (page: Page) => Promise<void>;
}

const card = (name: string): State => ({
  scenario: CARDS[name].scenario,
  extra: CARDS[name].extra,
  roots: ["dashpay-card"],
  go: async (page) => {
    await toolbar(page);
    await expect(page.getByTestId("dashpay-card")).toHaveAttribute("data-state", name === "votingNoEnd" ? "voting" : name);
  },
});

export const STATES: Record<string, State> = {
  ...Object.fromEntries(Object.keys(CARDS).map((name) => [`card-${name}`, card(name)])),
  join: { scenario: "dashpay-no-identity", roots: ["page-joinDashPay"], go: (page) => toJoin(page).then(() => page.getByTestId("join-cost").waitFor()) },
  "join-shortfall": { scenario: "empty", roots: ["page-joinDashPay"], go: (page) => toJoin(page).then(() => page.getByTestId("join-shortfall").waitFor()) },
  "join-watch-only": { scenario: "dashpay-watch-only", roots: ["page-joinDashPay"], go: (page) => toJoin(page).then(() => page.getByTestId("join-watch-only").waitFor()) },
  "join-joined": { scenario: "dashpay-registered", roots: ["page-joinDashPay"], go: (page) => toJoin(page).then(() => page.getByTestId("join-joined").waitFor()) },
  "join-card": { scenario: "dashpay-not-started", roots: ["page-joinDashPay"], go: (page) => toJoin(page).then(() => page.getByTestId("dashpay-card").waitFor()) },
  "voting-info": {
    scenario: "dashpay-no-identity",
    roots: ["voting-info"],
    go: async (page) => {
      await toJoin(page);
      await page.getByTestId("join-voting-info").click();
      await page.getByTestId("voting-info").waitFor();
    },
  },
  faq: {
    scenario: "dashpay-no-identity",
    roots: ["dashpay-faq"],
    go: async (page) => {
      await toJoin(page);
      await page.getByTestId("join-faq").click();
      await page.getByTestId("dashpay-faq").waitFor();
    },
  },
  "paste-refused": {
    scenario: "dashpay-no-identity",
    roots: ["claim-paste"],
    go: async (page) => {
      await toJoin(page);
      await page.getByTestId("join-invitation").click();
      await page.getByTestId("claim-paste-field").fill("https://example.com/not-an-invitation");
      await page.getByTestId("claim-paste-ok").click();
      await page.getByTestId("claim-paste-error").waitFor();
    },
  },
  "pay-with": {
    scenario: "dashpay-no-identity",
    roots: ["page-joinDashPay"],
    go: async (page) => {
      await toJoin(page);
      await page.getByTestId("join-pay-another").click();
      await page.getByTestId("pay-with-shielded-reason").waitFor();
      await page.getByTestId("pay-with-core-balance").waitFor();
    },
  },
  "name-rules": {
    scenario: "dashpay-no-identity",
    roots: ["page-joinDashPay"],
    go: async (page) => {
      await nameStep(page);
      await page.getByTestId("registration-name-field").fill("a-");
      await expect(page.getByTestId("registration-name-field")).toHaveAttribute("aria-invalid", "true");
    },
  },
  "name-available-contested": { scenario: "dashpay-no-identity", roots: ["page-joinDashPay"], go: (page) => typed(page, "alice", /ok/) },
  "name-contest-open": { scenario: "dashpay-no-identity", roots: ["page-joinDashPay"], go: (page) => typed(page, "carol", /neutral/) },
  "name-taken": { scenario: "dashpay-no-identity", roots: ["page-joinDashPay"], go: (page) => typed(page, "b0b", /problem/) },
  confirm: { scenario: "dashpay-no-identity", roots: ["page-joinDashPay"], go: (page) => confirm(page, LABEL) },
  "confirm-contested": { scenario: "dashpay-no-identity", roots: ["page-joinDashPay"], go: (page) => confirm(page, "alice") },
  "confirm-platform": {
    scenario: "dashpay-no-identity",
    roots: ["page-joinDashPay"],
    go: async (page) => {
      await toJoin(page);
      await page.getByTestId("join-pay-another").click();
      await page.getByTestId("pay-with-platform").getByRole("radio").check();
      await page.getByTestId("registration-next").click();
      await page.getByTestId("registration-name-field").fill(LABEL);
      await expect(page.getByTestId("registration-next")).toBeEnabled();
      await page.getByTestId("registration-next").click();
      await page.getByTestId("registration-fee").waitFor();
    },
  },
  "confirm-shortfall": {
    scenario: "empty",
    roots: ["page-joinDashPay"],
    go: (page) => confirm(page, LABEL).then(() => page.getByTestId("registration-shortfall").waitFor()),
  },
  "start-refused": {
    scenario: "dashpay-no-identity",
    extra: "&fail=start_registration:draftWait",
    roots: ["page-joinDashPay"],
    go: async (page) => {
      await started(page);
      await page.getByTestId("registration-error").waitFor();
    },
  },
  progress: { scenario: "dashpay-registering", roots: ["page-joinDashPay"], go: (page) => onScreen(page, "progress") },
  paused: { scenario: "dashpay-registration-paused", roots: ["page-joinDashPay"], go: (page) => onScreen(page, "paused") },
  unlock: { scenario: "dashpay-registering-unlock", roots: ["page-joinDashPay"], go: (page) => onScreen(page, "unlock") },
  retry: { scenario: "dashpay-registering-paused", roots: ["page-joinDashPay"], go: (page) => onScreen(page, "retry") },
  held: { scenario: "dashpay-registering-held", roots: ["page-joinDashPay"], go: (page) => onScreen(page, "held") },
  repair: { scenario: "dashpay-registration-needs-finish", roots: ["page-joinDashPay"], go: (page) => onScreen(page, "repair") },
  "repair-unknown": { scenario: "dashpay-registration-failed-unknown", roots: ["page-joinDashPay"], go: (page) => onScreen(page, "repair") },
  stopped: { scenario: "dashpay-registration-stopped", roots: ["page-joinDashPay"], go: (page) => onScreen(page, "stopped") },
  finished: {
    scenario: "dashpay-registration-needs-finish",
    roots: ["page-joinDashPay"],
    go: async (page) => {
      await onScreen(page, "repair");
      await page.getByTestId("registration-finish").click();
      await authorize(page);
      await page.getByTestId("registration-finished").waitFor();
    },
  },
  welcome: {
    scenario: "dashpay-no-identity",
    roots: ["page-joinDashPay"],
    go: async (page) => {
      await started(page);
      await complete(page, await draftOf(page));
      await expect(screen(page)).toHaveAttribute("data-kind", "done");
    },
  },
  "welcome-share": {
    scenario: "dashpay-no-identity",
    roots: ["welcome-share-sheet"],
    go: async (page) => {
      await started(page);
      await complete(page, await draftOf(page));
      await expect(page.getByTestId("welcome-share")).toBeEnabled();
      await page.getByTestId("welcome-share").click();
      await page.getByTestId("welcome-share-sheet").waitFor();
    },
  },
  "voting-begun": {
    scenario: "dashpay-no-identity",
    roots: ["page-joinDashPay"],
    go: async (page) => {
      await started(page, "alice");
      await complete(page, await draftOf(page));
      await expect(screen(page)).toHaveAttribute("data-kind", "voting");
    },
  },
  "name-failed": {
    scenario: "dashpay-no-identity",
    roots: ["page-joinDashPay"],
    go: async (page) => {
      await started(page);
      const draft = await draftOf(page);
      await page.evaluate((d) => {
        window.__dwFixture!.advanceRegistration(d);
        window.__dwFixture!.advanceRegistration(d);
        window.__dwFixture!.failRegistration(d, "name.taken", false);
      }, draft);
      await expect(screen(page)).toHaveAttribute("data-kind", "nameFailed");
    },
  },
  "confirm-credits": {
    scenario: "dashpay-no-identity",
    roots: ["page-joinDashPay"],
    go: async (page) => {
      await STATES["name-failed"].go(page);
      await page.getByTestId("registration-another").click();
      await page.getByTestId("registration-name-field").fill("alice-78");
      await expect(page.getByTestId("registration-next")).toBeEnabled();
      await page.getByTestId("registration-next").click();
      await expect(page.getByTestId("registration-cost")).toContainText(/\p{Nd}/u);
    },
  },
  "claim-invited": { scenario: "invitation-valid", roots: ["claim"], go: (page) => claimScreen(page, "invited") },
  "claim-claimed": { scenario: "invitation-claimed", roots: ["claim"], go: (page) => claimScreen(page, "claimed") },
  "claim-expired": { scenario: "invitation-valid", roots: ["claim"], go: (page) => claimScreen(page, "expired", { kind: "expired" }) },
  "claim-invalid": { scenario: "invitation-invalid", roots: ["claim"], go: (page) => claimScreen(page, "invalid") },
  "claim-not-found": { scenario: "invitation-valid", roots: ["claim"], go: (page) => claimScreen(page, "invalid", { kind: "invalid", reason: "assetLockNotFound" }) },
  "claim-wrong-network": { scenario: "invitation-valid", roots: ["claim"], go: (page) => claimScreen(page, "invalid", { kind: "invalid", reason: "wrongNetwork" }) },
  "claim-has-identity": {
    scenario: "dashpay-registered",
    roots: ["claim"],
    go: async (page) => {
      await toolbar(page);
      await page.evaluate(() => window.__dwFixture!.deliverLinks([{ kind: "invitation", linkId: "" }]));
      await expect(page.getByTestId("claim")).toHaveAttribute("data-state", "hasIdentity");
    },
  },
  "claim-name-refused": {
    scenario: "invitation-valid",
    roots: ["page-joinDashPay"],
    go: async (page) => {
      await claimScreen(page, "invited");
      await page.getByTestId("claim-accept").click();
      await page.getByTestId("registration-name-field").fill("carol");
      await page.getByTestId("registration-instant-only").waitFor();
      await expect(page.getByTestId("registration-availability")).not.toHaveAttribute("data-tone", "pending");
    },
  },
  "claim-confirm": {
    scenario: "invitation-valid",
    roots: ["page-joinDashPay"],
    go: async (page) => {
      await claimScreen(page, "invited");
      await page.getByTestId("claim-accept").click();
      await page.getByTestId("registration-name-field").fill("carol-77");
      await expect(page.getByTestId("registration-next")).toBeEnabled();
      await page.getByTestId("registration-next").click();
      await page.getByTestId("claim-recipient-text").waitFor();
    },
  },
  "claim-welcome": {
    scenario: "no-wallet-invitation",
    roots: ["claim"],
    go: async (page) => {
      await page.getByTestId("onboarding-restore").click();
      await page.getByTestId("restore-input").fill(MAIN_PHRASE);
      await page.getByTestId("restore-continue").click();
      await page.getByTestId("restore-finish").click();
      await page.getByTestId("onboarding-open").click();
      await expect(page.getByTestId("claim")).toHaveAttribute("data-entry", "welcome");
    },
  },
};

for (const theme of THEMES)
  for (const pseudo of PSEUDO)
    for (const [name, state] of Object.entries(STATES))
      test(`DashPay ${name} shows only catalog text under ${pseudo[0]}, fits, and passes axe, ${theme}`, async ({ page }, info) => {
        test.setTimeout(120_000);
        const errors = trackPageErrors(page);
        await checkArea(page, info, theme, pseudo, {
          name: `dashpay-${name}`,
          roots: state.roots,
          data: DATA,
          open: async (lang) => {
            await page.goto(fixtureUrl(state.scenario, theme, `${state.extra ?? ""}&lang=${lang}`));
            await state.go(page);
          },
        });
        expect(errors).toEqual([]);
      });

// The Join page while the wallet syncs. Its text is checked like the states above, but not with
// axe: the sync note fails colour contrast in the light theme, in English too (issue 326).
for (const theme of THEMES)
  for (const [lang, kind] of PSEUDO)
    test(`DashPay join-waiting shows only catalog text under ${lang} and fits, ${theme}`, async ({ page }) => {
      const errors = trackPageErrors(page);
      await page.goto(fixtureUrl("dashpay-waiting-for-sync", theme, `&lang=${lang}`));
      await toJoin(page);
      await page.getByTestId("join-waiting").waitFor();
      expect(await untranslated(page, ["page-joinDashPay"], kind, DATA)).toEqual([]);
      expect(await overflow(page, PANES)).toEqual([]);
      expect(errors).toEqual([]);
    });

// The FAQ's answers, each opened. The sheet's body scrolls once they all are, so only their text
// is checked here; the closed sheet is checked above.
for (const [lang, kind] of PSEUDO)
  test(`DashPay faq answers show only catalog text under ${lang}`, async ({ page }) => {
    const errors = trackPageErrors(page);
    await page.goto(fixtureUrl("dashpay-no-identity", "light", `&lang=${lang}`));
    await toJoin(page);
    await page.getByTestId("join-faq").click();
    await page.getByTestId("dashpay-faq").waitFor();
    await page.getByTestId("dashpay-faq-item").evaluateAll((items) => items.forEach((d) => ((d as HTMLDetailsElement).open = true)));
    await expect(page.getByTestId("dashpay-faq-item").last().locator("p")).toBeVisible();
    expect(await untranslated(page, ["dashpay-faq"], kind, DATA)).toEqual([]);
    expect(errors).toEqual([]);
  });

test("the DashPay card and the Join page follow a language change with no reload", async ({ page }) => {
  const errors = trackPageErrors(page);
  await page.goto(fixtureUrl("dashpay-no-identity", "light", "&lang=en"));
  await toolbar(page);
  await expect(page.getByTestId("dashpay-card").locator("h2")).toHaveText("Get a DashPay username");
  const language = async (tag: string) => {
    await command(page, { kind: "navigate", route: "settings" });
    await page.getByTestId("settings-language").selectOption(tag);
  };
  await language("en-XA");
  await page.getByTestId("sidebar-home").click();
  await expect(page.getByTestId("dashpay-card").locator("h2")).toHaveText(/^⟦.+⟧$/);
  await expect(page.getByTestId("dashpay-card-action")).toHaveText(/^⟦.+⟧$/);
  await toJoin(page);
  await expect(page.locator(".join-feature-title").first()).toHaveText(/^⟦.+⟧$/);
  await expect(page.getByTestId("join-cost")).toHaveText(/^⟦.+⟧$/);
  await language("");
  await toJoin(page);
  await expect(page.locator(".join-feature-title").first()).toHaveText("Pay to usernames");
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

test.describe("numbers keep their order under ar-XB", () => {
  test("a registration's step reads 1/3, its number before the three", async ({ page }) => {
    await page.goto(fixtureUrl("dashpay-registering", "light", "&lang=ar-XB"));
    await toolbar(page);
    const step = page.getByTestId("dashpay-card-step");
    await expect(step).toContainText("3");
    const at = await lefts(step, /(1)\/(3)/);
    expect(at, "the step").not.toBeNull();
    expect(at![0], "the step").toBeLessThan(at![1]);
  });
});
