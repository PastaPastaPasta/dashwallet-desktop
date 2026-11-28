import { readFileSync } from "node:fs";
import { expect, test, type Page } from "@playwright/test";
import type { Route } from "../src/api/generated/Route";
import { fixtureUrl, fontsReady } from "./helpers";

const config = JSON.parse(readFileSync(new URL("../../../rust/crates/dw-app/tauri.conf.json", import.meta.url), "utf8")) as {
  app: { windows: { label: string; minWidth: number; minHeight: number }[] };
};
const mainWindow = config.app.windows.find((window) => window.label === "main");
if (!mainWindow) throw new Error("Missing Tauri main window configuration");
const { minWidth, minHeight } = mainWindow;
const widths = [...new Set([minWidth, 900, 1100, 1440])];

// The shell's generated route contract includes routes that are not in the sidebar.
const routeSource = readFileSync(new URL("../src/api/generated/Route.ts", import.meta.url), "utf8");
const routeUnion = /^export type Route\s*=\s*("[\w-]+"(?:\s*\|\s*"[\w-]+")*)\s*;/m.exec(routeSource)?.[1];
if (!routeUnion) throw new Error("Expected Route to be a union of string literals");
const routes = routeUnion.split("|").map((literal) => JSON.parse(literal) as Route);

async function open(page: Page, route: Route) {
  const scenario = route === "masternodes" ? "governance" : route === "receive" ? "fiat-contacts" : "dashpay-registered";
  await page.goto(fixtureUrl(scenario, "light"));
  await page.getByTestId("sidebar").waitFor();
  await page.evaluate((route) => window.__dwFixture!.shellCommand({ kind: "navigate", route }), route);
  await page.getByTestId(`page-${route}`).waitFor();
  if (route === "receive") {
    await page.getByTestId("receive-qr").waitFor();
    await page.getByTestId("request-amount-toggle").waitFor();
  }
  if (route === "contacts") await page.getByRole("button", { name: "Accept Grace Hopper" }).waitFor();
  await fontsReady(page);
}

function horizontalOverflow(page: Page) {
  return page.evaluate(() => {
    const root = document.documentElement;
    const problems: string[] = [];
    const name = (el: Element) => el.getAttribute("data-testid") ?? `${el.localName}.${[...el.classList].join(".")}`;
    const scrolls = (overflowX: string) => overflowX === "auto" || overflowX === "scroll";
    const intentionalScroller = (el: Element) => scrolls(getComputedStyle(el).overflowX) && el.matches([
      ".table-card", // Data tables keep their columns usable by scrolling inside the card.
      ".identity-keys-wrap", // Key tables scroll within their own wrapper.
    ].join(","));

    if (root.scrollWidth > root.clientWidth) {
      problems.push(`document scrolls: ${root.scrollWidth} > ${root.clientWidth}`);
    }

    // Measure HTML and whole SVGs. SVG drawing primitives are clipped by their viewBox.
    for (const el of document.querySelectorAll("body *, body")) {
      if (!(el instanceof HTMLElement || el instanceof SVGSVGElement)) continue;
      if (el.closest(".sr-only") || !el.checkVisibility({ visibilityProperty: true })) continue;
      const box = el.getBoundingClientRect();
      if (!box.width || !box.height) continue;

      const overflowX = getComputedStyle(el).overflowX;
      if (scrolls(overflowX) && !intentionalScroller(el) && el.scrollWidth > el.clientWidth + 1) {
        problems.push(`${name(el)} scrolls: ${el.scrollWidth} > ${el.clientWidth}`);
      }

      for (let container = el.parentElement; container; container = container.parentElement) {
        if (getComputedStyle(container).overflowX === "visible" && container !== root) continue;
        if (intentionalScroller(container)) break;
        const left = container === root ? 0 : container.getBoundingClientRect().left + container.clientLeft;
        const right = left + container.clientWidth;
        if (box.left < left - 1 || box.right > right + 1) {
          problems.push(`${name(el)} [${box.left.toFixed(1)}, ${box.right.toFixed(1)}] exceeds ${name(container)} [${left}, ${right}]`);
          break;
        }
      }
    }
    return problems;
  });
}

for (const width of widths) {
  test.describe(`${width}px`, () => {
    test.use({ viewport: { width, height: minHeight } });

    for (const route of routes) {
      test(`${route} fits horizontally`, async ({ page }) => {
        await open(page, route);
        expect(await horizontalOverflow(page)).toEqual([]);
      });
    }
  });
}

test("Receive request controls fit onscreen at 900px with the sidebar visible", async ({ page }) => {
  await page.setViewportSize({ width: 900, height: minHeight });
  await open(page, "receive");
  const viewport = (await page.getByTestId("page-receive").boundingBox())!;
  const address = (await page.getByTestId("receive-card").boundingBox())!;
  const form = (await page.getByTestId("request-form").boundingBox())!;
  expect.soft(form.y).toBeGreaterThanOrEqual(address.y + address.height);
  expect.soft(form.x + form.width).toBeLessThanOrEqual(viewport.x + viewport.width);
  for (const id of ["request-label", "request-message", "request-amount-toggle", "request-create"]) {
    const control = page.getByTestId(id);
    await expect(control).toBeVisible();
    const box = (await control.boundingBox())!;
    expect.soft(box.x + box.width, id).toBeLessThanOrEqual(Math.min(form.x + form.width, viewport.x + viewport.width));
  }
});
