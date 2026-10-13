import { expect, test } from "@playwright/test";
import { ERROR_CLASSES, sampleError } from "../src/fixture/errors";
import { seriousAxeViolations } from "./axe";
import { fixtureUrl } from "./helpers";

// One error state per `ErrorData` class: `error-<class>` makes `list_wallets` reject with the
// class's sample, and the shell shows it as an error state, not a crash. Chromium only: the
// error state is one component; the engines are compared on the main screens.
test.describe("error classes", () => {
  test.skip(({ browserName }) => browserName !== "chromium", "one engine is enough for the class matrix");

  for (const cls of ERROR_CLASSES) {
    test(`error-${cls} renders as an error state`, async ({ page }) => {
      const errors: string[] = [];
      page.on("pageerror", (e) => errors.push(e.message));
      await page.goto(fixtureUrl(`error-${cls}`, "light"));
      const state = page.getByTestId("wallets-error");
      await expect(state).toContainText("Could not load the wallets");
      await expect(state).toContainText(sampleError(cls).message);
      // The rest of the shell keeps working.
      await expect(page.getByTestId("toolbar-lock")).toBeVisible();
      expect(await seriousAxeViolations(page)).toEqual([]);
      expect(errors).toEqual([]);
    });
  }

  test("the splash offers a retry when the network does not open", async ({ page }) => {
    await page.goto(fixtureUrl("funded", "light", "&fail=open_network:retryAfter"));
    await expect(page.getByRole("alert")).toContainText("vault.throttled");
    await page.evaluate(() => window.__dwFixture!.fail("open_network", null));
    await page.getByRole("button", { name: "Try again" }).click();
    await page.getByTestId("hero").waitFor();
  });
});
