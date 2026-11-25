// The instant username's offer sheet (DP1-09 D4 reversed): Create instant username is the primary,
// focused default, so Enter takes it; No, thanks is secondary. Pinned on the markup here; the e2e
// registration spec presses Enter in a browser.

import { renderToStaticMarkup } from "react-dom/server";
import { createElement } from "react";
import { describe, expect, it } from "vitest";
import { RegistrationWizard } from "../../dashpay/wizard";
import { InstantOfferSheet } from "./Registration";

describe("InstantOfferSheet", () => {
  const wiz = new RegistrationWizard({} as never, "w");
  const html = renderToStaticMarkup(createElement(InstantOfferSheet, { wiz, network: "testnet" }));
  const button = (testId: string) => html.match(new RegExp(`<button[^>]*data-testid="${testId}"[^>]*>`))?.[0] ?? "";

  it("Create instant username: primary, the form's submit (Enter), focused when the sheet opens", () => {
    const create = button("instant-offer-create");
    expect(create).toContain('type="submit"');
    expect(create).toContain("data-autofocus");
    expect(create).toMatch(/filledBlue|filled-blue/);
    expect(html).toContain("Create instant username");
  });

  it("No, thanks: secondary, never the submit", () => {
    const decline = button("instant-offer-decline");
    expect(decline).toContain('type="button"');
    expect(decline).not.toContain("data-autofocus");
    expect(decline).not.toMatch(/filledBlue|filled-blue/);
  });

  it("the title and the body with the network's vote length", () => {
    expect(html).toContain("Create an instant username");
    expect(html).toContain("will be voted on for 1 hour 30 minutes.");
    expect(html.indexOf("instant-offer-decline")).toBeLessThan(html.indexOf("instant-offer-create"));
  });
});
