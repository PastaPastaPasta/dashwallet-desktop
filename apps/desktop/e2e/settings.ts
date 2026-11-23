import type { Page } from "@playwright/test";
import type { ShellCommand } from "../src/api/generated/ShellCommand";
import { FIXTURE_WALLET_ID } from "../src/fixture/scenarios";
import { fixtureUrl, fontsReady, type Theme } from "./helpers";

// The U-04 settings screens in a fixed state, for axe (settings.spec.ts) and the visual baselines.

export const SETTINGS_VIEWS = [
  "general",
  "security",
  "wallets",
  "backups",
  "about",
  "changePassphrase",
  "wipe",
  "xpub",
  "backupSheet",
  "exportSheet",
  "importFile",
] as const;
export type SettingsView = (typeof SETTINGS_VIEWS)[number];

const TAB: Record<SettingsView, string> = {
  general: "general",
  security: "security",
  wallets: "wallets",
  backups: "backups",
  about: "about",
  changePassphrase: "security",
  wipe: "security",
  xpub: "wallets",
  backupSheet: "backups",
  exportSheet: "backups",
  importFile: "backups",
};

const OPEN: Partial<Record<SettingsView, [button: string, ready: string]>> = {
  changePassphrase: ["settings-change-passphrase", "settings-change-current"],
  wipe: ["settings-wipe", "settings-wipe-sentence"],
  xpub: [`settings-wallet-${FIXTURE_WALLET_ID.slice(0, 8)}-xpub`, "settings-xpub-key"],
  backupSheet: ["settings-backup", "settings-backup-write"],
  exportSheet: ["settings-export", "settings-export-passphrase-field"],
  importFile: ["settings-import-file", "settings-import-path"],
};

/** Opens the `funded` wallet's Settings and shows `view`. */
export async function showSettingsView(page: Page, view: SettingsView, theme: Theme) {
  await page.goto(fixtureUrl("funded", theme));
  await page.getByTestId("toolbar").waitFor();
  const navigate: ShellCommand = { kind: "navigate", route: "settings" };
  await page.evaluate((c) => window.__dwFixture!.shellCommand(c), navigate);
  await page.getByTestId(`settings-tabs-${TAB[view]}`).click();
  const open = OPEN[view];
  if (open) {
    await page.getByTestId(open[0]).click();
    await page.getByTestId(open[1]).waitFor();
  }
  // The rows that load from the engine (data directory, backups) are in.
  if (view === "about") await page.getByTestId("settings-about-agent").getByText("…").waitFor({ state: "detached" });
  await fontsReady(page);
}
