// The shared controls and text modules read the language in use (MP-14): the sheet's Close and
// Cancel, the passphrase field and its busy label, copy names, empty states, failure texts by code,
// and the refused network switch's whole sentences.

import { createElement } from "react";
import { renderToStaticMarkup } from "react-dom/server";
import { afterEach, describe, expect, it } from "vitest";
import { DEFAULT_PREFS, ui } from "../app/ui";
import { flatten } from "../i18n/core";
import { en } from "../i18n/en";
import { LANGUAGES, type Language } from "../i18n";
import { NetworkText } from "../network/choice";
import { failureText } from "../onboarding/model";
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
    expect(name({ value: "v", copyLabel: "Copy data directory" })).toEqual(["Copy data directory"]);
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

  it("say a failure by its code in the language in use, and the code itself only in the generic sentence", () => {
    const e = (code: string, message = "x") => ({ code, message });
    expect(failureText(e("backup.corrupt"))).toBe("This file is not a Dash Wallet backup, or it is damaged.");
    expect(failureText(e("vault.wrong_passphrase"))).toBe("The passphrase entered for the wallet decryption was incorrect.");
    expect(failureText(e("wallet", "still opening"))).toBe("A wallet is still opening; close the network once it is open.");
    expect(failureText(e("name.taken"))).toBe("Something went wrong (name.taken).");
    expect(failureText(new Error("boom"))).toBe("Something went wrong.");
    choose("en-XA");
    for (const code of ["backup.corrupt", "vault.wrong_passphrase", "invalid_mnemonic"]) expect(failureText(e(code))).toMatch(PSEUDO);
    expect(failureText(e("name.taken"))).toMatch(/^⟦.*name\.taken.*⟧$/);
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
