import { createElement, isValidElement, type ReactElement, type ReactNode } from "react";
import { renderToStaticMarkup } from "react-dom/server";
import { beforeEach, describe, expect, it, vi } from "vitest";
import type { IdentitySummary } from "../../api/generated/IdentitySummary";
import { ui } from "../../app/ui";
import { editProfileFor, type EditProfileApi } from "../../dashpay/editProfile";
import { namedModel, type ScreenActionKind } from "../../dashpay/registration";
import type { StatusInputs } from "../../dashpay/status";
import { findFriends, openEditProfile } from "../../dashpay/welcome";
import type { RegistrationWizard } from "../../dashpay/wizard";
import { GrantModel } from "../../tools/grant";
import { RegistrationScreen } from "./Registration";
import { ShareSheet, WelcomeSteps } from "./Welcome";

const LINK = "dashpay://user?du=alice-77";
vi.mock("../../onboarding/model", async (original) => ({
  ...(await original<typeof import("../../onboarding/model")>()),
  useModel: (model: unknown) => model,
}));
vi.mock("../../contacts/queries", async (original) => ({
  ...(await original<typeof import("../../contacts/queries")>()),
  useMyUserLink: () => ({ data: LINK, error: null, isError: false, refetch: () => undefined }),
}));
vi.mock("../../api/queries", async (original) => ({
  ...(await original<typeof import("../../api/queries")>()),
  useQrMatrix: () => ({ data: undefined }),
}));

const api = {} as EditProfileApi;
const wiz = { running: null, finished: null, error: null, mode: { kind: "core" }, grants: new GrantModel(api as never) } as unknown as RegistrationWizard;
const inputs = { amount: () => "", discreet: false, date: () => "" } as unknown as StatusInputs;
const done = namedModel("alice-77", { kind: "registered" }, () => "");

const welcome = (share: { walletId: string; identity: string } | null = { walletId: "w", identity: "ID" }, onAction: (k: ScreenActionKind) => void = () => undefined) =>
  renderToStaticMarkup(createElement(RegistrationScreen, { wiz, model: done, onAction, inputs, share }));

/** The element with `testId` in a tree of plain elements (no component is rendered). */
function find(node: ReactNode, testId: string): ReactElement<{ onClick: () => void; disabled: boolean }> | null {
  if (Array.isArray(node)) {
    for (const n of node) {
      const hit = find(n, testId);
      if (hit) return hit;
    }
    return null;
  }
  if (!isValidElement(node)) return null;
  const props = node.props as { "data-testid"?: string; children?: ReactNode };
  if (props["data-testid"] === testId) return node as ReactElement<{ onClick: () => void; disabled: boolean }>;
  return find(props.children, testId);
}

beforeEach(() => ui.set({ route: "joinDashPay", addContact: null, contact: null, contactPrefill: null }));

describe("the welcome moment (R9)", () => {
  it("congratulates the new @username and offers three next steps and a quiet exit", () => {
    const html = welcome();
    expect(html).toContain('data-kind="done"');
    expect(html).toContain("Welcome to DashPay, @alice-77");
    expect(html).toContain("Your username is ready.");
    for (const [id, title] of [
      ["welcome-editProfile", "Add a photo and name"],
      ["welcome-findFriends", "Find friends"],
      ["welcome-share", "Share my username"],
    ])
      expect(html).toMatch(new RegExp(`data-testid="${id}"[^>]*>.*?${title}`));
    expect(html).toMatch(/data-testid="registration-wallet"[^>]*>Go to wallet</);
    // The exit is the plain button: the steps lead.
    expect(html).not.toContain("dash-button-filledBlue");
    // No QR until Share is chosen.
    expect(html).not.toContain("add-contact-my-qr");
  });

  it("each step reports its own kind", () => {
    const steps: string[] = [];
    const tree = WelcomeSteps({ onStep: (k) => steps.push(k), disabled: false, canEdit: true, canShare: true });
    for (const id of ["welcome-editProfile", "welcome-findFriends", "welcome-share"]) find(tree, id)!.props.onClick();
    expect(steps).toEqual(["editProfile", "findFriends", "share"]);
  });

  it("Share waits for the identity; a running step holds them all", () => {
    expect(find(WelcomeSteps({ onStep: () => undefined, disabled: false, canEdit: true, canShare: false }), "welcome-share")!.props.disabled).toBe(true);
    expect(find(WelcomeSteps({ onStep: () => undefined, disabled: false, canEdit: true, canShare: false }), "welcome-findFriends")!.props.disabled).toBe(false);
    const busy = WelcomeSteps({ onStep: () => undefined, disabled: true, canEdit: true, canShare: true });
    for (const id of ["welcome-editProfile", "welcome-findFriends", "welcome-share"]) expect(find(busy, id)!.props.disabled).toBe(true);
  });

  it("Add a photo and name waits for the main identity", () => {
    const unread = WelcomeSteps({ onStep: () => undefined, disabled: false, canEdit: false, canShare: true });
    expect(find(unread, "welcome-editProfile")!.props.disabled).toBe(true);
    expect(find(unread, "welcome-share")!.props.disabled).toBe(false);
    expect(find(unread, "welcome-findFriends")!.props.disabled).toBe(false);
  });

  it("Share my username shows Add contact's My QR with the link", () => {
    const html = renderToStaticMarkup(createElement(ShareSheet, { walletId: "w", identity: "ID", onClose: () => undefined }));
    expect(html).toContain('data-testid="welcome-share"');
    expect(html).toContain("Share my username");
    expect(html).toContain('data-testid="add-contact-my-qr"');
    expect(html).toContain(LINK);
    expect(html).toContain("Copy Link");
  });
});

describe("where the steps lead", () => {
  it("Find friends opens Contacts with Add contact on Search", () => {
    ui.set({ contact: "someone" });
    findFriends("w");
    expect(ui.get()).toMatchObject({ route: "contacts", contact: null, contactPrefill: null, addContact: { walletId: "w", query: "", hit: null } });
  });

  it("Add a photo and name opens My Profile with Edit Profile for the main identity", () => {
    const main = { identity: "ID-1", profile: { displayName: "Alice" } } as unknown as IdentitySummary;
    openEditProfile(api, "w-edit", main);
    expect(ui.get().route).toBe("profile");
    const editor = editProfileFor(api, "w-edit");
    expect(editor).toMatchObject({ open: true, identity: "ID-1", displayName: "Alice" });
  });

  it("with the identity unread, Add a photo and name opens My Profile alone", () => {
    openEditProfile(api, "w-unread", undefined);
    expect(ui.get().route).toBe("profile");
    expect(editProfileFor(api, "w-unread").open).toBe(false);
  });
});
