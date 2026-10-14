import { expect, test, type Page } from "@playwright/test";
import { SCENARIO_NAMES } from "../src/fixture/scenarios";
import { seriousAxeViolations } from "./axe";
import { artifactPath, fixtureUrl, fontsReady, THEMES, trackPageErrors } from "./helpers";

// U-07: every fixture scenario boots on the fixture backend alone, and each state screen the
// scenarios reach renders in light and dark with 0 serious or critical axe issues.

/** A state screen: the scenario (and URL extras) that reach it, then what proves it is up. */
interface State {
  name: string;
  scenario: string;
  extra?: string;
  ready: (page: Page) => Promise<void>;
}

const STATES: State[] = [
  { name: "empty-wallet", scenario: "empty", ready: (p) => p.getByTestId("history-empty").waitFor() },
  { name: "no-wallet", scenario: "no-wallet", ready: (p) => p.getByTestId("onboarding-welcome").waitFor() },
  { name: "locked-at-start", scenario: "locked", ready: (p) => p.getByTestId("lock-field").waitFor() },
  { name: "loading", scenario: "funded", extra: "&delay=600000", ready: (p) => p.getByTestId("splash").waitFor() },
  {
    name: "open-failed",
    scenario: "funded",
    extra: "&fail=open_network:none",
    ready: (p) => p.getByRole("alert").filter({ hasText: "could not open the wallet" }).waitFor(),
  },
  { name: "wallets-error", scenario: "error-duffs", ready: (p) => p.getByTestId("wallets-error").waitFor() },
  {
    name: "history-error",
    scenario: "funded",
    extra: "&fail=history_page:rpc",
    ready: async (p) => {
      await p.getByTestId("sidebar-activity").click();
      await p.getByTestId("tx-error").waitFor();
    },
  },
  {
    name: "lock-report-unresolved",
    scenario: "lock-report-omitted",
    ready: async (p) => {
      await p.getByTestId("toolbar-lock").click();
      await p.getByTestId("lock-report-omitted").waitFor();
    },
  },
];

for (const theme of THEMES) {
  for (const state of STATES) {
    test(`${state.name} ${theme}: renders, 0 serious axe issues`, async ({ page }, info) => {
      const errors = trackPageErrors(page);
      await page.goto(fixtureUrl(state.scenario, theme, state.extra));
      await state.ready(page);
      await fontsReady(page);
      await page.screenshot({ path: artifactPath(info, `${theme}-state-${state.name}.png`) });
      expect(await page.locator("html").getAttribute("data-theme")).toBe(theme);
      expect(await seriousAxeViolations(page)).toEqual([]);
      expect(errors).toEqual([]);
    });
  }
}

for (const scenario of SCENARIO_NAMES) {
  test(`${scenario} boots without an engine: all IPC is the fixture's`, async ({ page }) => {
    const errors = trackPageErrors(page);
    await page.goto(fixtureUrl(scenario, "light"));
    await page.locator("[data-testid=hero], [data-testid=onboarding], [data-testid=lock-screen]").first().waitFor();
    const fixture = await page.evaluate(() => ({ sentinel: window.__dwFixture?.sentinel, scenario: window.__dwFixture?.scenario, calls: window.__dwFixture?.calls }));
    expect(fixture.sentinel).toBe("dw-fixture-backend:U-07");
    expect(fixture.scenario).toBe(scenario);
    expect(fixture.calls).toContain("open_network");
    expect(errors).toEqual([]);
  });
}
