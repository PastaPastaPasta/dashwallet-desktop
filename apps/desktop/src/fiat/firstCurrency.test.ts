import { afterEach, describe, expect, it, vi } from "vitest";
import { IpcBackend } from "../api/tauri";
import { DEFAULT_PREFS, ui } from "../app/ui";
import { firstCurrency } from "../format/fiat";
import { LANGUAGES, type Language } from "../i18n";

const choose = (language: string | null) => ui.set({ prefs: { ...DEFAULT_PREFS, language } });
const system = (...languages: string[]) => vi.stubGlobal("navigator", { languages, language: languages[0] });

/** A complete German catalog (English's messages under the tag `de`), shipped for the test. */
function withGerman(): () => void {
  const english = LANGUAGES[0];
  const german: Language = { tag: "de", name: "Deutsch", dir: "ltr", messages: english.messages };
  LANGUAGES.push(german);
  return () => void LANGUAGES.splice(LANGUAGES.indexOf(german), 1);
}

afterEach(() => {
  choose(null);
  vi.unstubAllGlobals();
});

describe("the first local currency", () => {
  it("comes from the system region, not from a non-English UI language", () => {
    const remove = withGerman();
    try {
      system("en-US");
      choose("de");
      const b = new IpcBackend("tauri");
      expect(b.locale).toBe("de");
      expect(b.regionLocale).toBe("en-US");
      expect(firstCurrency(null, b.regionLocale)).toBe("USD");
    } finally {
      remove();
    }
  });

  it("follows a non-US system region under System default", () => {
    system("de-DE", "en-US");
    choose(null);
    const b = new IpcBackend("tauri");
    expect(b.locale).toBeUndefined();
    expect(b.regionLocale).toBe("de-DE");
    expect(firstCurrency(null, b.regionLocale)).toBe("EUR");
  });

  it("takes the first system language that names a region", () => {
    system("fr", "fr-CH");
    expect(new IpcBackend("tauri").regionLocale).toBe("fr-CH");
    expect(firstCurrency(undefined, "fr-CH")).toBe("CHF");
  });

  it("keeps a saved currency in both cases", () => {
    system("de-DE");
    expect(firstCurrency("JPY", new IpcBackend("tauri").regionLocale)).toBe("JPY");
    system("en-US");
    expect(firstCurrency("JPY", new IpcBackend("tauri").regionLocale)).toBe("JPY");
  });

  it("uses the fixture's fixed locale in a fixture build", () => {
    system("de-DE");
    expect(new IpcBackend("fixture", () => new Date(0), "en-US").regionLocale).toBe("en-US");
  });
});
