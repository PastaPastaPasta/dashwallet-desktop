// DP5-01: the fixture's link stash answers as dw-app's intake and stash commands do.

import { clearMocks } from "@tauri-apps/api/mocks";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import * as cmd from "../api/generated/commands";
import { installFixture } from "./index";

beforeEach(() => vi.stubGlobal("window", globalThis));
afterEach(() => {
  clearMocks();
  vi.unstubAllGlobals();
});

const ADDRESS = "yNPbcFfabtNmmxKdGwhHomdYfVs6gikbPf";
const PAY = `dash:${ADDRESS}?amount=0.25&label=Coffee`;
const INVITE = "dashpay://invite?du=alice&pk=secret";

describe("the link stash (fixture, as shell/links.rs)", () => {
  it("DP5-01 keeps links opened before a wallet, offers them once, invitations by id", async () => {
    const f = installFixture({ scenario: "no-wallet" });
    const saved = await cmd.routeLink("testnet", PAY);
    expect(saved).toMatchObject({ kind: "saved", link: { kind: "payment" } });
    // The same link again is the same entry.
    expect(await cmd.routeLink("testnet", PAY)).toEqual(saved);
    const invitation = await cmd.routeLink("testnet", INVITE);
    expect(invitation).toMatchObject({ kind: "invitation" });
    const linkId = invitation.kind === "invitation" ? invitation.linkId : "";
    expect(linkId).toMatch(/^[0-9a-f]{32}$/);
    expect((await cmd.stashedLinks()).map((l) => l.kind)).toEqual(["payment", "invitation"]);
    // Nothing in the stash carries the text.
    expect(JSON.stringify(await cmd.stashedLinks())).not.toContain(ADDRESS);

    const id = saved.kind === "saved" ? saved.link.id : "";
    f.world.lock = { ...f.world.lock, state: "locked" };
    await expect(cmd.openStashedLink(id)).rejects.toMatchObject({ code: "platform.signer_unavailable" });
    f.world.lock = { ...f.world.lock, state: "unlocked" };
    expect(await cmd.openStashedLink(id)).toMatchObject({ kind: "pay", uri: { address: ADDRESS, amount: 25_000_000 } });
    expect(await cmd.openStashedLink(id)).toBeNull();
    expect(await cmd.openStashedLink(linkId)).toEqual({ kind: "invitation", linkId });
    expect(await cmd.openStashedLink(linkId)).toEqual({ kind: "invitation", linkId });
    await cmd.dismissStashedLink(linkId);
    expect(await cmd.stashedLinks()).toEqual([]);
    await expect(cmd.openStashedLink("nope")).rejects.toMatchObject({ code: "invalid_argument" });
  });

  it("DP5-01 with a wallet, a payment link routes at once", async () => {
    installFixture({ scenario: "funded" });
    expect(await cmd.routeLink("testnet", `dash:${ADDRESS}`)).toMatchObject({ kind: "pay" });
    expect(await cmd.stashedLinks()).toEqual([]);
  });
});
