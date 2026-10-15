// Tools ▸ Console (QT-145): the Swift `ConsoleViewModel`, ported. dash-qt's local RPC console over
// dw-console's grammar on the engine; nothing reaches a shell (U-05 D5). The echo and the history
// hold only the redacted line (D6); a command that needs a grant asks for it and runs again (D7).
// The font size lasts the session (D8).

import { errorMessage, hasCode, isAppError, type Backend } from "../api/backend";
import type { AuthGrant } from "../api/generated/AuthGrant";
import type { ConsoleCommand } from "../api/generated/ConsoleCommand";
import type { WalletSummary } from "../api/generated/WalletSummary";
import { Model } from "../onboarding/model";
import type { GrantModel } from "./grant";

export const HISTORY_LIMIT = 50;
export const FONT_SIZES = { min: 4, max: 40, default: 12 };

export const ConsoleText = {
  welcome: (mac: boolean) => {
    const [clear, bigger, smaller] = mac ? ["⌘L", "⌘+", "⌘-"] : ["Ctrl+L", "Ctrl++", "Ctrl+-"];
    return [
      "Welcome to the Dash Wallet RPC console.",
      `Use up and down arrows to navigate history, and ${clear} to clear screen.`,
      `Use ${bigger} and ${smaller} to increase or decrease the font size.`,
      "Type help for an overview of available commands.",
      "For more information on using this console, type help-console.",
    ].join("\n");
  },
  scamWarning:
    "WARNING: Scammers have been active, telling users to type commands here, stealing their wallet contents. Do not use this console without fully understanding the ramifications of a command.",
  executingWith: (name: string) => `Executing command using "${name}" wallet`,
  executingWithout: "Executing command without any wallet",
  invalidCommandLine: "Error: Invalid command line",
  notAvailable: "Not available in SPV mode.",
  walletRequired: "Wallet file not specified (must request wallet RPC through /wallet/<filename> uri-path).",
  authorizationCancelled: "Authorization cancelled.",
  grantInvalid: "The authorization expired. Please try again.",
};

export type EntryKind = "welcome" | "warning" | "command" | "reply" | "error" | "info";

export interface ConsoleEntry {
  id: number;
  kind: EntryKind;
  text: string;
  isJson: boolean;
}

/** A failed line as the console prints it: Core's "message (code N)" for RPC errors. */
export function consoleErrorText(e: unknown): string {
  if (hasCode(e, "console.rpc_error") && e.data?.kind === "rpc") return `${e.data.rpcMessage} (code ${e.data.rpcCode})`;
  if (hasCode(e, "console.parse_error")) return ConsoleText.invalidCommandLine;
  if (hasCode(e, "console.not_available")) return ConsoleText.notAvailable;
  if (hasCode(e, "console.wallet_required")) return ConsoleText.walletRequired;
  if (hasCode(e, "vault.grant_invalid")) return ConsoleText.grantInvalid;
  return errorMessage(e);
}

type Api = Pick<Backend, "consoleCommands" | "consoleRedact" | "consoleExecute">;

export class ConsoleModel extends Model {
  entries: ConsoleEntry[] = [];
  /** Redacted lines, oldest first (memory only, as dash-qt). */
  history: string[] = [];
  commands: ConsoleCommand[] = [];
  wallets: WalletSummary[] = [];
  /** `null`: run without any wallet. */
  walletId: string | null = null;
  fontSize = FONT_SIZES.default;
  /** A line that did not parse (dash-qt's error box); the line stays in the field. */
  error: string | null = null;
  running = false;
  #nextId = 0;
  #cursor: number | null = null;

  constructor(
    private readonly api: Api,
    readonly grants: GrantModel,
    private readonly mac = false,
  ) {
    super();
    this.clear();
  }

  /** dash-qt shows the selector with two wallets or more. */
  get showsWalletSelector(): boolean {
    return this.wallets.length >= 2;
  }

  /** Command names for completion; the wallets for the selector (the first is selected). */
  async load(wallets: WalletSummary[]) {
    this.setWallets(wallets);
    try {
      this.commands = await this.api.consoleCommands();
    } catch (e) {
      this.error = errorMessage(e);
    }
    this.changed();
  }

  setWallets(wallets: WalletSummary[]) {
    this.wallets = wallets;
    if (this.walletId === null || !wallets.some((w) => w.walletId === this.walletId)) this.walletId = wallets[0]?.walletId ?? null;
    this.changed();
  }

  /** Ctrl+L: the welcome text and the anti-scam warning only. */
  clear() {
    this.entries = [];
    this.#append("welcome", ConsoleText.welcome(this.mac));
    this.#append("warning", ConsoleText.scamWarning);
    this.changed();
  }

  /** Echoes and keeps the redacted line, then runs it; `false` when nothing ran (empty, busy, or
   * a line that does not parse, which is not kept), so the field keeps it. */
  async run(text: string): Promise<boolean> {
    const line = text.trim();
    if (this.running || line === "") return false;
    let redacted: string;
    try {
      redacted = await this.api.consoleRedact(line);
    } catch (e) {
      this.error = hasCode(e, "console.parse_error") ? ConsoleText.invalidCommandLine : errorMessage(e);
      this.changed();
      return false;
    }
    this.error = null;
    this.history = [...this.history, redacted].slice(-HISTORY_LIMIT);
    this.#cursor = null;
    this.#append("command", redacted);
    this.running = true;
    this.changed();
    try {
      await this.#execute(line);
    } finally {
      this.running = false;
      this.changed();
    }
    return true;
  }

  async #execute(line: string) {
    let grant: AuthGrant | null = null;
    for (;;) {
      try {
        const out = await this.api.consoleExecute(this.walletId, line, grant?.id ?? null);
        this.#append("reply", out.text, out.isJson);
        return;
      } catch (e) {
        // A grant the engine refused once is not asked for in a loop.
        if (!hasCode(e, "console.authorization_required") || grant !== null || e.data?.kind !== "authorize") {
          if (grant) this.grants.revoke(grant);
          this.#append("error", isAppError(e) ? consoleErrorText(e) : errorMessage(e));
          return;
        }
        try {
          grant = await this.grants.obtain(e.data.purpose, e.data.walletId);
        } catch (refused) {
          this.#append("error", errorMessage(refused));
          return;
        }
        if (grant === null) {
          this.#append("error", ConsoleText.authorizationCancelled);
          return;
        }
      }
    }
  }

  /** Up arrow: the previous history line; `null` with no history. */
  historyUp(): string | null {
    if (this.history.length === 0) return null;
    this.#cursor = Math.max((this.#cursor ?? this.history.length) - 1, 0);
    return this.history[this.#cursor];
  }

  /** Down arrow: the next line, or empty past the newest; `null` when not browsing. */
  historyDown(): string | null {
    if (this.#cursor === null) return null;
    const next = this.#cursor + 1;
    if (next >= this.history.length) {
      this.#cursor = null;
      return "";
    }
    this.#cursor = next;
    return this.history[next];
  }

  /** Tab completion over every command name plus `help` and `help-console`. */
  completions(prefix: string): string[] {
    const p = prefix.toLowerCase();
    return [...new Set([...this.commands.map((c) => c.name), "help", "help-console"])].filter((n) => n.startsWith(p)).sort();
  }

  selectWallet(walletId: string | null) {
    if (walletId === this.walletId) return;
    this.walletId = walletId;
    const w = this.wallets.find((x) => x.walletId === walletId);
    this.#append("info", w ? ConsoleText.executingWith(w.name) : ConsoleText.executingWithout);
    this.changed();
  }

  setFontSize(size: number) {
    const clamped = Math.min(Math.max(size, FONT_SIZES.min), FONT_SIZES.max);
    if (clamped === this.fontSize) return;
    this.fontSize = clamped;
    this.changed();
  }

  #append(kind: EntryKind, text: string, isJson = false) {
    this.entries = [...this.entries, { id: this.#nextId++, kind, text, isJson }];
  }
}
