// Address Book, by parity ID (the Swift `AddressBookViewModelTests`, ported).

import { describe, expect, it, vi } from "vitest";
import type { AddressBookEntry } from "../api/generated/AddressBookEntry";
import type { AddressClass } from "../api/generated/AddressClass";
import type { AddressPurposeName } from "../api/generated/AddressPurposeName";
import { appError, WALLET_A } from "../onboarding/testWorld";
import { AddressBookModel, AddressBookText, wildcard } from "./addressBook";

const ALICE = "yAliceAddress0000000000000000000001";
const BOB = "yBobAddress0000000000000000000000002";
const SCRIPT = "8ScriptAddress00000000000000000003";
const FRESH = "yFreshAddress000000000000000000009";

const classify = async (_: string, text: string): Promise<AddressClass> =>
  text.startsWith("8") ? { kind: "core", scriptHash: true } : text.startsWith("y") ? { kind: "core", scriptHash: false } : { kind: "invalid", problem: { kind: "notBech32mOrBase58" } };

function book(saveError: unknown = null) {
  let entries: AddressBookEntry[] = [
    { address: BOB, label: "bob", purpose: "send", createdAt: null },
    { address: ALICE, label: "Alice", purpose: "send", createdAt: null },
    { address: SCRIPT, label: "", purpose: "receive", createdAt: null },
  ];
  const saves: [string, string, AddressPurposeName, boolean][] = [];
  const deletes: string[] = [];
  const api = {
    classifyAddress: vi.fn(classify),
    addressBook: vi.fn(async (_: string, purpose: AddressPurposeName | null) => entries.filter((e) => purpose === null || e.purpose === purpose)),
    saveAddressBookEntry: vi.fn(async (_: string, address: string, label: string, purpose: AddressPurposeName, replace: boolean) => {
      if (saveError) throw saveError;
      const known = entries.find((e) => e.address === address);
      if (replace && !known) throw appError("labels.entry_not_found");
      saves.push([address, label, purpose, replace]);
      const entry = { address, label, purpose, createdAt: null };
      entries = [...entries.filter((e) => e.address !== address), entry];
      return entry;
    }),
    deleteAddressBookEntry: vi.fn(async (_: string, address: string) => {
      deletes.push(address);
      entries = entries.filter((e) => e.address !== address);
      return null;
    }),
  };
  return { api, saves, deletes };
}

async function model(purpose: AddressPurposeName = "send", selectionMode = false, saveError: unknown = null) {
  const b = book(saveError);
  const m = new AddressBookModel(b.api, WALLET_A, "testnet", purpose, selectionMode);
  await m.load();
  return { m, ...b };
}

describe("Address Book (QT-095…098)", () => {
  it("QT095_sendingListSortedByLabel", async () => {
    const { m } = await model();
    expect(m.entries.map((e) => e.label)).toEqual(["Alice", "bob"]);
    expect(m.header).toBe(AddressBookText.sendingHeader);
    expect(m.canCreate && m.canDelete).toBe(true);
  });

  it("QT095_wildcardSearchOnAddressOrLabel", async () => {
    const { m } = await model();
    m.setSearch("al?ce");
    expect(m.entries.map((e) => e.label)).toEqual(["Alice"]);
    m.setSearch("B?B");
    expect(m.entries.map((e) => e.label)).toEqual(["bob"]);
    m.setSearch("00002");
    expect(m.entries.map((e) => e.label)).toEqual(["bob"]);
    m.setSearch("");
    expect(m.entries).toHaveLength(2);
    expect(wildcard("a*c")("xxABCxx")).toBe(true);
    expect(wildcard("a?c")("abbc")).toBe(false);
    expect(wildcard("a.c")("abc")).toBe(false);
  });

  it("QT095_newEntryIsValidatedAndSaved", async () => {
    const { m, saves } = await model();
    expect(await m.save(` ${FRESH} `, "Carol", false)).toBe(true);
    expect(saves.at(-1)).toEqual([FRESH, "Carol", "send", false]);
    expect(m.entries.map((e) => e.label)).toEqual(["Alice", "bob", "Carol"]);
    expect(await m.save("garbage", "x", false)).toBe(false);
    expect(m.error).toBe(AddressBookText.invalidAddress("garbage"));
  });

  it("QT098_duplicateMessagesMatchDashQt", async () => {
    const { m, saves } = await model();
    expect(await m.save(ALICE, "again", false)).toBe(false);
    expect(m.error).toBe(`The entered address "${ALICE}" is already in the address book with label "Alice".`);
    expect(await m.save(SCRIPT, "mine", false)).toBe(false);
    expect(m.error).toBe(AddressBookText.existsAsReceiving(SCRIPT, ""));
    expect(await m.save(ALICE, "Alice B.", true)).toBe(true);
    expect(saves.at(-1)?.[3]).toBe(true);
  });

  it("QT096_receivingEntriesEditLabelOnlyNoDelete", async () => {
    const { m, deletes, saves } = await model("receive");
    expect(m.entries.map((e) => e.address)).toEqual([SCRIPT]);
    expect(m.canCreate || m.canDelete).toBe(false);
    expect(m.labelText(m.entries[0])).toBe("(no label)");
    expect(await m.save(SCRIPT, "Savings", true)).toBe(true);
    expect(saves.at(-1)).toEqual([SCRIPT, "Savings", "receive", true]);
    expect(await m.save("yNotInBook0000000000000000000000001", "x", true)).toBe(false);
    expect(m.error).toBe(AddressBookText.entryNotFound);
    await m.delete(SCRIPT);
    expect(m.error).toBe(AddressBookText.receivingNotDeletable);
    expect(deletes).toEqual([]);
  });

  it("QT095_deleteSendingEntry", async () => {
    const { m, deletes } = await model();
    await m.delete(BOB);
    expect(deletes).toEqual([BOB]);
    expect(m.entries.map((e) => e.label)).toEqual(["Alice"]);
  });

  it("QT095_exportCSVHasLabelAndAddress", async () => {
    const { m } = await model();
    expect(m.exportCsv()).toBe(`"Label","Address"\n"Alice","${ALICE}"\n"bob","${BOB}"\n`);
    await m.save(FRESH, 'Quote "q"', false);
    expect(m.exportCsv()).toContain(`"Quote ""q""","${FRESH}"`);
  });

  it("QT095_QT096_showQRCodeOfAnEntry: cleared when the list changes purpose", async () => {
    const { m } = await model();
    m.showQr(m.entries[0]);
    expect(m.qrEntry?.label).toBe("Alice");
    m.setPurpose("receive");
    expect(m.qrEntry).toBeNull();
  });

  it("QT097_selectionMode", async () => {
    const picker = (await model("send", true)).m;
    picker.choose(picker.entries[0]);
    expect(picker.chosen?.label).toBe("Alice");
    const plain = (await model()).m;
    plain.choose(plain.entries[0]);
    expect(plain.chosen).toBeNull();
  });

  it("engineDuplicateCodeMapsToDashQtText", async () => {
    const { m } = await model("send", false, appError("labels.duplicate_address"));
    expect(await m.save(FRESH, "x", false)).toBe(false);
    expect(m.error).toBe(AddressBookText.alreadyInBook(FRESH, ""));
  });

  it("a refusal for an entry this list has not read yet names its label", async () => {
    const { m, api } = await model("send", false, appError("labels.own_address"));
    const read = api.addressBook.getMockImplementation()!;
    api.addressBook.mockImplementation(async (w, purpose) =>
      purpose === "receive" ? [{ address: FRESH, label: "Savings", purpose: "receive", createdAt: null }] : read(w, purpose),
    );
    expect(await m.save(FRESH, "x", false)).toBe(false);
    expect(m.error).toBe(AddressBookText.existsAsReceiving(FRESH, "Savings"));
  });
});
