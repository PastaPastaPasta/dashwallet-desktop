import { describe, expect, it, vi } from "vitest";
import type { ContactList } from "../../api/generated/ContactList";
import type { ContactSummaryInfo } from "../../api/generated/ContactSummaryInfo";
import type { UserHit } from "../../api/generated/UserHit";
import { editRecipient, parseRecipientInput, recipientSuggestions, resolveRecipient, type RecipientApi } from "./recipient";

const ADDRESS = "yNPbcFfabtNmmxKdGwhHomdYfVs6gikbPf";
const hit: UserHit = { identity: "bob", username: "bob", displayName: "Bob Builder", relation: "contact", unverified: true, avatarUrl: null };
const summary: ContactSummaryInfo = { ...hit, contact: hit.identity, alias: "Bobby", hidden: false, since: null, lastActivityAt: null, channel: "ready" };
const uri = { address: ADDRESS, amount: 25_000_001, label: "Rent", message: "June" };
const api = (): RecipientApi => ({
  routeLink: vi.fn(async () => ({ kind: "pay" as const, uri, username: "bob" })),
  classifyAddress: vi.fn(async () => ({ kind: "core" as const, scriptHash: false })),
  dashpayVerifyScanned: vi.fn(async () => ({ ...hit, code: { kind: "userLink" as const } })),
});

describe("Pay to", () => {
  it("drops an address quote's recipient on edit, including a change back to the same address", () => {
    const address = { kind: "address", route: "core", username: null, ...uri } as const;
    const before = { address, text: ADDRESS, revision: 0 };
    const cleared = editRecipient(before, "");
    const retyped = editRecipient(cleared, ADDRESS);
    expect(cleared.address).toBeNull();
    expect(retyped).toEqual({ address: null, text: ADDRESS, revision: 2 });
  });

  it("classifies a raw address with the same network and retains its spelling", async () => {
    const backend = api();
    const input = parseRecipientInput(` ${ADDRESS} `);
    expect(await resolveRecipient(backend, "wallet", "testnet", input)).toMatchObject({ kind: "address", address: ADDRESS, route: "core", amount: null });
    expect(backend.classifyAddress).toHaveBeenCalledWith("testnet", ADDRESS);
    expect(backend.routeLink).not.toHaveBeenCalled();
  });

  it.each([`dash:${ADDRESS}?amount=0.25000001&label=Rent&message=June`, `pay:${ADDRESS}`, `dashwallet://pay=${ADDRESS}`])("retains the engine's URI fields for %s", async (text) => {
    const backend = api();
    expect(await resolveRecipient(backend, "wallet", "testnet", parseRecipientInput(text))).toMatchObject({ kind: "address", ...uri, username: "bob" });
    expect(backend.routeLink).toHaveBeenCalledWith("testnet", text);
  });

  it("uses the verified identity from a DashPay user link", async () => {
    const backend = api();
    const link = "dashpay://user?id=fixture&username=bob";
    expect(await resolveRecipient(backend, "wallet", "testnet", parseRecipientInput(link))).toMatchObject({ kind: "user", hit });
    expect(backend.dashpayVerifyScanned).toHaveBeenCalledWith("wallet", link);
    expect(backend.classifyAddress).not.toHaveBeenCalled();
  });

  it("keeps the existing wrong-network and invalid-address messages", async () => {
    const backend: RecipientApi = {
      ...api(),
      classifyAddress: vi.fn().mockResolvedValueOnce({ kind: "invalid", problem: { kind: "invalidBase58Prefix" } }).mockResolvedValueOnce({ kind: "invalid", problem: { kind: "notBech32mOrBase58" } }),
    };
    const input = parseRecipientInput(ADDRESS);
    expect(await resolveRecipient(backend, "wallet", "mainnet", input)).toMatchObject({ kind: "invalid", error: "This address is not valid on this network." });
    expect(await resolveRecipient(backend, "wallet", "mainnet", input)).toMatchObject({ kind: "invalid", error: "The recipient address is not valid. Please recheck." });
  });

  it("lists matching contacts before other network usernames and keeps the contact's alias", () => {
    const list: ContactList = { updating: false, sections: [{ section: "contacts", contacts: [summary] }] };
    const network = { ...hit, identity: "other", username: "bobby", relation: "stranger" as const };
    expect(recipientSuggestions(list, [network, hit])).toMatchObject([
      { kind: "contact", payee: { contact: hit.identity, name: "Bobby" } },
      { kind: "user", hit: network },
    ]);
  });
});
