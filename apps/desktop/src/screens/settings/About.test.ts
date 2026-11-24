// Settings ▸ About's links: the warning under Report a problem, and a failed open's link to copy.

import { createElement } from "react";
import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it } from "vitest";
import type { ProjectLinkName } from "../../api/generated/ProjectLinkName";
import { HANDLERS } from "../../fixture/handlers";
import { t } from "../../i18n";
import { failedLinkUrl, LinkOpener, ProjectLinksView } from "./About";

const render = (failed: { url: string | null } | null) => renderToStaticMarkup(createElement(ProjectLinksView, { failed, onOpen: () => {} }));

describe("About's links", () => {
  it("warns under Report a problem that reports are public", () => {
    const html = render(null);
    const issues = html.indexOf('data-testid="settings-link-issues"');
    const hint = html.indexOf('data-testid="settings-link-issues-hint"');
    expect(issues).toBeGreaterThan(-1);
    expect(hint).toBeGreaterThan(issues);
    expect(hint).toBeLessThan(html.indexOf('data-testid="settings-link-sourceCode"'));
    expect(html).toContain(t("settings.about.links.issuesHint"));
    expect(html).not.toContain('data-testid="settings-link-failed"');
  });

  it.each([
    ["issues", "https://github.com/PastaPastaPasta/dashwallet-desktop/issues"],
    ["sourceCode", "https://github.com/PastaPastaPasta/dashwallet-desktop"],
    ["website", "https://www.dash.org"],
  ] as [ProjectLinkName, string][])("gives the %s link to copy when the browser doesn't open", (link, url) => {
    const world = { linkOpenFails: true };
    let error: unknown = null;
    try {
      (HANDLERS.open_project_link as (args: { link: ProjectLinkName }, ctx: { world: typeof world }) => null)({ link }, { world });
    } catch (e) {
      error = e;
    }
    expect(failedLinkUrl(error)).toBe(url);
    const html = render({ url });
    expect(html).toContain("The browser couldn&#x27;t be opened. Copy the link instead: ");
    expect(html).toContain(`<span>${url}</span>`);
    expect(html).toContain(`aria-label="${t("receive.dash.copyLink")}"`);
  });

  it("tells screen readers about the warning on Report a problem", () => {
    const html = render(null);
    const id = /id="([^"]+)"[^>]*data-testid="settings-link-issues-hint"/.exec(html)?.[1];
    expect(id).toBeTruthy();
    expect(html).toMatch(new RegExp(`aria-describedby="${id}"[^>]*data-testid="settings-link-issues"|data-testid="settings-link-issues"[^>]*aria-describedby="${id}"`));
  });

  it("shows only the latest click's failure", async () => {
    const answers: ((ok: boolean) => void)[] = [];
    const opener = new LinkOpener({
      openProjectLink: () =>
        new Promise<null>((resolve, reject) =>
          answers.push((ok) => (ok ? resolve(null) : reject({ code: "app.browser_open", message: "", data: { kind: "link", url: "https://www.dash.org" } }))),
        ),
    });
    const first = opener.open("website");
    const second = opener.open("sourceCode");
    answers[1](true);
    await second;
    answers[0](false);
    await first;
    expect(opener.failed).toBeNull();
    const third = opener.open("website");
    answers[2](false);
    await third;
    expect(opener.failed).toEqual({ url: "https://www.dash.org" });
  });

  it("falls back to the generic line for a failure without a link", () => {
    expect(failedLinkUrl(new Error("x"))).toBeNull();
    expect(render({ url: null })).toContain(t("onboarding.error.generic"));
  });
});
