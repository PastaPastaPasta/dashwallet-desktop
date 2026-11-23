// The renderer's side of the file chooser (U-05 D2) over the fixture's dialogs.

import { clearMocks } from "@tauri-apps/api/mocks";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { IpcBackend } from "../api/tauri";
import { installFixture, type FixtureHandle } from "../fixture";
import { base64ToBytes, bytesToBase64, chooseFilePath, fileErrorText, openFile, saveText, textToBase64 } from "./files";

let fixture: FixtureHandle;
const b = new IpcBackend("fixture");
beforeEach(() => {
  vi.stubGlobal("window", globalThis);
  fixture = installFixture({ scenario: "funded" });
});
afterEach(() => {
  clearMocks();
  vi.unstubAllGlobals();
});

describe("files", () => {
  it("encodes text as UTF-8 and bytes of any size", () => {
    expect(textToBase64('"Label","Address"\n"Zoë",')).toBe(btoa('"Label","Address"\n"ZoÃ«",'));
    const big = Uint8Array.from({ length: 100_000 }, (_, i) => i % 256);
    expect(base64ToBytes(bytesToBase64(big))).toEqual(big);
  });

  it("saves where the dialog answers, and a cancel writes nothing", async () => {
    expect(await saveText(b, "csv", "Addresses.csv", "a,b")).toBe("/home/user/Documents/Addresses.csv");
    expect(fixture.world.tools.files.saved).toEqual([{ purpose: "csv", path: "/home/user/Documents/Addresses.csv", data: "YSxi" }]);
    fixture.world.tools.files.saveDir = null;
    expect(await saveText(b, "csv", "Addresses.csv", "a,b")).toBeNull();
    expect(fixture.world.tools.files.saved).toHaveLength(1);
  });

  it("opens the file the dialog picks, or nothing", async () => {
    expect(await openFile(b, "psbt")).toBeNull();
    fixture.world.tools.files.open = { name: "tx.psbt", data: "cHNidP8=" };
    expect(await openFile(b, "psbt")).toEqual({ name: "tx.psbt", data: "cHNidP8=" });
    expect(await chooseFilePath(b, "walletBackup", true, "Main.dwbackup")).toBe("/home/user/Documents/Main.dwbackup");
    expect(fixture.world.tools.files.dialogs.map((d) => d.purpose)).toEqual(["psbt", "psbt", "walletBackup"]);
  });

  it("names each failure", async () => {
    fixture.fail("open_file", "none");
    const e = await openFile(b, "psbt").catch((x: unknown) => x);
    expect(fileErrorText(e)).not.toBe("");
    const err = (code: "app.file_too_large" | "app.file_write") => ({ code, message: code, data: null });
    expect(fileErrorText(err("app.file_too_large"))).toBe("The file is too large.");
    expect(fileErrorText(err("app.file_write"))).toBe("The file could not be saved.");
  });
});
