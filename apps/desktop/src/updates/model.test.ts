import { describe, expect, it } from "vitest";
import type { UpdatesView } from "../api/generated/UpdatesView";
import { availableVersion, checkDue, updateText } from "./model";

const view = (state: UpdatesView["state"], over: Partial<UpdatesView> = {}): UpdatesView => ({ supported: true, enabled: true, state, checkedAt: null, ...over });
const available = view({ kind: "available", release: { version: "0.3.0", publishedAt: null } });

describe("the update check's wording", () => {
  it("says what each state means", () => {
    expect(updateText(view({ kind: "off" }, { enabled: false }))).toBe("Not checked: the update check is off");
    expect(updateText(view({ kind: "notChecked" }))).toBe("Checking for updates…");
    expect(updateText(view({ kind: "current" }))).toBe("Dash Wallet is up to date");
    expect(updateText(available)).toBe("Version 0.3.0 is available");
    expect(updateText(view({ kind: "failed" }))).toBe("Could not check for updates");
    expect(updateText(view({ kind: "off" }, { supported: false }))).toBe("Updates come through Flatpak");
  });

  it("names a version only while one is available", () => {
    expect(availableVersion(available)).toBe("0.3.0");
    for (const kind of ["off", "notChecked", "current", "failed"] as const) expect(availableVersion(view({ kind }))).toBeNull();
    expect(availableVersion(undefined)).toBeNull();
  });

  it("checks at launch only when on, supported and not checked yet", () => {
    expect(checkDue(view({ kind: "notChecked" }))).toBe(true);
    expect(checkDue(view({ kind: "notChecked" }, { supported: false }))).toBe(false);
    expect(checkDue(view({ kind: "notChecked" }, { enabled: false }))).toBe(false);
    for (const kind of ["off", "current", "failed"] as const) expect(checkDue(view({ kind }))).toBe(false);
    expect(checkDue(available)).toBe(false);
    expect(checkDue(undefined)).toBe(false);
  });
});
