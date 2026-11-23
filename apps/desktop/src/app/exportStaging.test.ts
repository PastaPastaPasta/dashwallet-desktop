import { afterEach, describe, expect, it, vi } from "vitest";
import type { RecentNotice } from "../api/generated/RecentNotice";

const toasts: string[] = [];
vi.mock("../components/toast", () => ({ showShellToast: (text: string) => toasts.push(text) }));

const { dismissExportStaging, onExportStagingNotice, readExportStagingNotices, resetExportStaging, subscribeStuck } = await import("./exportStaging");
const { t } = await import("../i18n");

describe("unfinished export staging files", () => {
  afterEach(() => {
    resetExportStaging();
    toasts.length = 0;
  });

  it("toasts a deleted file once, and keeps one it could not delete until dismissed", async () => {
    const stuck: string[][] = [];
    const stop = subscribeStuck((paths) => stuck.push([...paths]));
    onExportStagingNotice("exportStagingDeleted", "/home/u/.dash-wallet-export.tmp-1-0000000000000001");
    onExportStagingNotice("exportStagingDeleted", "/home/u/.dash-wallet-export.tmp-1-0000000000000001");
    expect(toasts).toEqual([t("home.exportStaging.deleted")]);

    const path = "/home/u/.dash-wallet-export.tmp-1-0000000000000002";
    onExportStagingNotice("exportStagingNotDeleted", path);
    onExportStagingNotice("exportStagingNotDeleted", path);
    expect(stuck.at(-1)).toEqual([path]);
    dismissExportStaging(path);
    expect(stuck.at(-1)).toEqual([]);
    expect(toasts).toHaveLength(1);
    stop();
  });

  it("reads the notices of its network sent before the renderer listened", async () => {
    const notice = (seq: number, network: string | null, code: RecentNotice["code"], detail: string): RecentNotice => ({ seq, network, code, detail });
    const stuck: string[][] = [];
    const stop = subscribeStuck((paths) => stuck.push([...paths]));
    await readExportStagingNotices(
      async () => [
        notice(1, "testnet", "backupFailed", "wallet w: x"),
        notice(2, "mainnet", "exportStagingNotDeleted", "/other"),
        notice(3, "testnet", "exportStagingNotDeleted", "/mine"),
        notice(4, "testnet", "exportStagingDeleted", "/gone"),
      ],
      "testnet",
    );
    expect(stuck.at(-1)).toEqual(["/mine"]);
    expect(toasts).toEqual([t("home.exportStaging.deleted")]);
    stop();
  });

  it("brings a dismissed card back at each start while the engine still names its file, and not once it is gone", async () => {
    const path = "/home/u/.dash-wallet-export.tmp-1-0000000000000003";
    const named: RecentNotice = { seq: 1, network: "testnet", code: "exportStagingNotDeleted", detail: path };
    const start = async (recent: RecentNotice[]) => {
      vi.resetModules();
      const run = await import("./exportStaging");
      const shown: (readonly string[])[] = [];
      run.subscribeStuck((paths) => shown.push(paths));
      await run.readExportStagingNotices(async () => recent, "testnet");
      return { run, shown };
    };

    const first = await start([named]);
    expect(first.shown.at(-1)).toEqual([path]);
    first.run.dismissExportStaging(path);
    expect(first.shown.at(-1)).toEqual([]);

    const second = await start([named]);
    expect(second.shown.at(-1)).toEqual([path]);

    const third = await start([]);
    expect(third.shown).toEqual([]);
  });
});
