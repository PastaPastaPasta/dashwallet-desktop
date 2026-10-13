import { expect, test, type Page } from "@playwright/test";
import { SCENARIO_NAMES } from "../src/fixture/scenarios";
import { seriousAxeViolations } from "./axe";
import { artifactPath, fixtureUrl, fontsReady, THEMES } from "./helpers";

// U-07: every fixture scenario boots on the fixture backend alone, and each state screen the
// scenarios reach renders in light and dark with 0 serious or critical axe issues.

/** A state screen: the URL that reaches it, then what proves it is up. */
interface State {
  name: string;
  url: (theme: "light" | "dark") => string;
  ready: (page: Page) => Promise<void>;
}

const STATES: State[] = [
  { name: "empty-wallet", url: (t) => fixtureUrl("empty", t), ready: (p) => p.getByTestId("history-empty").waitFor() },
  { name: "no-wallet", url: (t) => fixtureUrl("no-wallet", t), ready: (p) => p.getByTestId("no-wallet").waitFor() },
  { name: "locked-at-start", url: (t) => fixtureUrl("locked", t), ready: (p) => p.getByTestId("lock-field").waitFor() },
  { name: "loading", url: (t) => fixtureUrl("funded", t, "&delay=600000"), ready: (p) => p.getByTestId("splash").waitFor() },
  {
    name: "open-failed",
    url: (t) => fixtureUrl("funded", t, "&fail=open_network:none"),
    ready: (p) => p.getByRole("alert").filter({ hasText: "could not open the wallet" }).waitFor(),
  },
  { name: "wallets-error", url: (t) => fixtureUrl("error-duffs", t), ready: (p) => p.getByTestId("wallets-error").waitFor() },
  {
    name: "history-error",
    url: (t) => fixtureUrl("funded", t, "&fail=history_page:rpc"),
    ready: async (p) => {
      await p.getByTestId("sidebar-transactions").click();
      await p.getByTestId("tx-error").waitFor();
    },
  },
  {
    name: "lock-report-unresolved",
    url: (t) => fixtureUrl("lock-report-omitted", t),
    ready: async (p) => {
      await p.getByTestId("toolbar-lock").click();
      await p.getByTestId("lock-report-omitted").waitFor();
    },
  },
];

for (const theme of THEMES) {
  for (const state of STATES) {
    test(`${state.name} ${theme}: renders, 0 serious axe issues`, async ({ page }, info) => {
      const errors: string[] = [];
      page.on("pageerror", (e) => errors.push(e.message));
      await page.goto(state.url(theme));
      await state.ready(page);
      await fontsReady(page);
      await page.screenshot({ path: artifactPath(info, `${theme}-state-${state.name}.png`) });
      expect(await page.locator("html").getAttribute("data-theme")).toBe(theme);
      expect(await seriousAxeViolations(page)).toEqual([]);
      expect(errors).toEqual([]);
    });
  }
}

test("every scenario boots without an engine: all IPC is the fixture's", async ({ page }) => {
  for (const scenario of SCENARIO_NAMES) {
    const errors: string[] = [];
    page.on("pageerror", (e) => errors.push(e.message));
    await page.goto(fixtureUrl(scenario, "light"));
    await page.locator("[data-testid=hero], [data-testid=no-wallet], [data-testid=lock-screen]").first().waitFor();
    const fixture = await page.evaluate(() => ({ sentinel: window.__dwFixture?.sentinel, scenario: window.__dwFixture?.scenario, calls: window.__dwFixture?.calls }));
    expect(fixture.sentinel, scenario).toBe("dw-fixture-backend:U-07");
    expect(fixture.scenario).toBe(scenario);
    expect(fixture.calls).toContain("open_network");
    expect(errors, scenario).toEqual([]);
    page.removeAllListeners("pageerror");
  }
});
