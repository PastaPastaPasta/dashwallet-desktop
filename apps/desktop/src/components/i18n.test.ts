// The shared controls and text modules read the language in use (MP-14): the sheet's Close and
// Cancel, the passphrase field and its busy label, copy names, empty states, the detail date's
// joiner, the refused network switch's whole sentences, and Coin Selection's Tree mode rows.

import { createElement } from "react";
import { renderToStaticMarkup } from "react-dom/server";
import { afterEach, describe, expect, it } from "vitest";
import { DEFAULT_PREFS, ui } from "../app/ui";
import { flatten } from "../i18n/core";
import { en } from "../i18n/en";
import { LANGUAGES, type Language } from "../i18n";
import { NetworkText } from "../network/choice";
import { detailDate } from "../format/tx";
import type { Utxo } from "../api/generated/Utxo";
import { display } from "../format/display";
import { groupCoins } from "../tools/coins";
import { CoinTable } from "./coins";
import { CopyValue } from "./detail";
import { EMPTY } from "./emptyStates";
import { PassphrasePrompt } from "./passphrase";
import { QuestionSheet } from "./sheet";

const choose = (language: string | null) => ui.set({ prefs: { ...DEFAULT_PREFS, language } });
afterEach(() => choose(null));

const PSEUDO = /^⟦.+⟧$/;
const attr = (html: string, name: string) => [...html.matchAll(new RegExp(`${name}="([^"]*)"`, "g"))].map((m) => m[1]);
const unescape = (s: string) => s.replace(/&amp;/g, "&").replace(/&quot;/g, '"').replace(/&#x27;/g, "'");

const question = () =>
  renderToStaticMarkup(createElement(QuestionSheet, { title: "T", text: "X", confirm: "OK", testId: "q", confirmTestId: "q-ok", onConfirm: () => {}, onClose: () => {} }));
const prompt = (busy: boolean) =>
  renderToStaticMarkup(createElement(PassphrasePrompt, { testId: "p", error: null, busy, submitLabel: "Unlock", onSubmit: () => {} }));

describe("shared controls", () => {
  it("name the sheet's Close and the question's Cancel, the passphrase field and its busy label in English", () => {
    expect(attr(question(), "aria-label")).toEqual(["Close"]);
    expect(question()).toContain(">Cancel</button>");
    expect(prompt(false)).toContain(">Passphrase</label>");
    expect(prompt(true)).toContain(">Checking…</button>");
  });

  it("take them from the catalog under the long pseudo-locale", () => {
    choose("en-XA");
    expect(attr(question(), "aria-label").map(unescape)).toEqual([expect.stringMatching(PSEUDO)]);
    expect(question()).not.toContain(">Cancel<");
    expect(prompt(false)).toMatch(/<label [^>]*>⟦[^<]+⟧<\/label>/);
    expect(prompt(true)).toMatch(/<button [^>]*>⟦[^<]+⟧<\/button>/);
    expect(prompt(true)).not.toContain("Checking…");
  });

  it("name a copy button with a whole sentence, or the noun a screen passes in the catalog's sentence", () => {
    const name = (props: Parameters<typeof CopyValue>[0]) => attr(renderToStaticMarkup(createElement(CopyValue, props)), "aria-label");
    expect(name({ value: "v", label: "address" })).toEqual(["Copy address"]);
    expect(name({ value: "v", action: "Copy data directory" })).toEqual(["Copy data directory"]);
    choose("en-XA");
    expect(name({ value: "v", label: "address" }).map(unescape)).toEqual([expect.stringMatching(/^⟦.*⟧$/)]);
  });
});

describe("text modules", () => {
  it("read empty states in the language in use when the screen reads them", () => {
    expect(EMPTY.coins.title).toBe("No coins yet");
    expect(EMPTY.coins.action.label).toBe("Receive Dash");
    choose("en-XA");
    expect(EMPTY.coins.title).toMatch(PSEUDO);
    expect(EMPTY.coins.action.label).toMatch(PSEUDO);
  });

  it("join a detail date and its time with the catalog's pattern, English unchanged", () => {
    const at = new Date(2026, 9, 6, 1, 39);
    expect(detailDate(at, "en-US")).toBe("Oct 6, 2026 at 1:39 AM");
    choose("en-XA");
    expect(detailDate(at, "en-US")).toMatch(PSEUDO);
    expect(detailDate(at, "en-US")).toContain("Oct 6, 2026");
    expect(detailDate(at, "en-US")).not.toContain(" at ");
    choose("ar-XB");
    expect(detailDate(at, "en-US")).toBe("\u2068Oct 6, 2026\u2069\u202e at \u202c\u20681:39 AM\u2069");
  });
});

describe("a refused network switch", () => {
  it("says each variant as one sentence with English unchanged", () => {
    expect(NetworkText.busy(["payment"], false)).toBe("Wait for the payment to finish before switching networks.");
    expect(NetworkText.busy(["payment", "topUp", "withdraw"], true)).toBe(
      "Wait for the payment, credit top-up and credit withdrawal to finish, or stop mixing, before switching networks.",
    );
    expect(NetworkText.busy(["contactRequest"], true, ["contactRequest"])).toBe(
      "Wait for contact request to finish, or stop mixing, before switching networks. Locking the wallet stops contact request.",
    );
    expect(NetworkText.busy([], true)).toBe("Stop mixing before switching networks.");
  });

  it("names every flow the lock report names, as it reads inside the sentence", () => {
    const flows = Object.entries(en.lock.flow);
    expect(Object.keys(en.network.busyItem).sort()).toEqual(["payment", ...flows.map(([k]) => k)].sort());
    for (const [k, name] of flows) expect(en.network.busyItem[k as keyof typeof en.lock.flow]).toBe(name[0].toLowerCase() + name.slice(1));
  });

  it("says a refusal it has no words for with the engine's message, and its code when there is one", () => {
    expect(NetworkText.notSwitchedBecause("busy", "app.network_busy")).toBe("Dash Wallet could not switch networks. busy (app.network_busy)");
    expect(NetworkText.notSwitchedBecause("boom")).toBe("Dash Wallet could not switch networks. boom");
  });

  it("takes every part from the catalog, the payment and each flow as the sentence needs them", () => {
    choose("en-XA");
    const text = NetworkText.busy(["payment", "registration"], false, ["registration"]);
    expect(text).toMatch(PSEUDO);
    expect(text).not.toMatch(/the payment|username registration|Username registration/);
  });

  it("joins a translation's list with the language's own pattern", () => {
    const german = flatten({ network: { busyItem: { payment: "die Zahlung", topUp: "die Aufladung", withdraw: "die Auszahlung" } } });
    const de: Language = { tag: "de", name: "Deutsch", dir: "ltr", messages: german };
    LANGUAGES.push(de);
    try {
      choose("de");
      expect(NetworkText.busy(["payment", "topUp", "withdraw"], false)).toBe("Wait for die Zahlung, die Aufladung und die Auszahlung to finish before switching networks.");
    } finally {
      LANGUAGES.splice(LANGUAGES.indexOf(de), 1);
    }
  });
});

describe("Coin Selection's Tree mode rows", () => {
  const coin = (n: number, address: string): Utxo => ({
    outpoint: { txid: n.toString(16).padStart(64, "0"), vout: 0 },
    address,
    amount: 100_000_000,
    confirmations: 10,
    blockHeight: 100,
    timestamp: 1_700_000_000,
    instantLocked: false,
    chainLocked: true,
    userLocked: false,
    reserved: false,
    label: null,
    isChange: false,
    isCoinbase: false,
    coinjoinDenominated: false,
    coinjoinRounds: null,
    spendable: true,
    pendingUnproven: false,
  });
  const ONE = "yAddressOne0000000000000000000001";
  const TWO = "yAddressTwo0000000000000000000002";
  const coins = [coin(1, ONE), coin(2, TWO), coin(3, TWO)];
  const groups = () => {
    const html = renderToStaticMarkup(
      createElement(CoinTable, {
        coins,
        shown: display({ unit: "dash", decimalDigits: 2 }, { kind: "testnet", dirName: "testnet", unitName: "tDASH" }),
        selected: new Set<string>(),
        onToggle: () => {},
        detail: { sort: { column: "amount", ascending: false }, onSort: () => {}, groups: groupCoins(coins), onToggleGroup: () => {}, actions: () => null },
      }),
    );
    return [...html.matchAll(/<tr class="coin-group"[^>]*>.*?aria-label="([^"]*)".*?<td colSpan="5">([^<]*)<\/td><\/tr>/g)].map((m) => [unescape(m[1]), m[2]]);
  };

  it("count a group's coins and name its checkbox in English as before", () => {
    expect(groups()).toEqual([
      ["Use every coin at yAddressOne0…000000000001", "1 coin"],
      ["Use every coin at yAddressTwo0…000000000002", "2 coins"],
    ]);
  });

  it("take both from the catalog under the long pseudo-locale", () => {
    choose("en-XA");
    for (const [name, count] of groups()) {
      expect(name).toMatch(/^⟦.+⟧$/);
      expect(count).toMatch(/^⟦.+⟧$/);
    }
  });
});
