// Tools ▸ Console (QT-145): a line goes to dw-console on the engine, never to a shell (D5). Echo
// and history hold the redacted line (D6); ↑/↓ browse it, Tab completes a command name, Ctrl+L
// clears. A command that needs a grant asks for the passphrase and runs again (D7).

import { useEffect, useId, useRef, useState, type KeyboardEvent } from "react";
import type { WalletSummary } from "../../api/generated/WalletSummary";
import { DashButton } from "../../components/basics";
import { useModel } from "../../onboarding/model";
import { FONT_SIZES, type ConsoleModel } from "../../tools/console";

/** `model` is the page's, one per window wallet: lines, history and font size outlive a tab
 * change (D1, D8). */
export function ConsoleTab({ model, wallets }: { model: ConsoleModel; wallets: WalletSummary[] }) {
  useModel(model);
  const [line, setLine] = useState("");
  const log = useRef<HTMLDivElement>(null);
  const id = useId();
  // The command names once; the wallets whenever the list changes.
  useEffect(() => void model.load(wallets), [model]);
  useEffect(() => model.setWallets(wallets), [model, wallets]);
  useEffect(() => {
    log.current?.scrollTo({ top: log.current.scrollHeight });
  }, [model.entries.length]);

  const onKeyDown = (e: KeyboardEvent<HTMLInputElement>) => {
    if (e.key === "Enter") {
      // WebKit (the app's WebKitGTK too) takes Enter in an input with a datalist for the
      // suggestions and never submits the form.
      e.preventDefault();
      e.currentTarget.form?.requestSubmit();
    } else if (e.key === "ArrowUp" || e.key === "ArrowDown") {
      const next = e.key === "ArrowUp" ? model.historyUp() : model.historyDown();
      if (next !== null) {
        e.preventDefault();
        setLine(next);
      }
    } else if (e.key === "Tab" && line.trim() !== "" && !/\s/.test(line.trim())) {
      const found = model.completions(line.trim());
      if (found.length === 1) {
        e.preventDefault();
        setLine(`${found[0]} `);
      }
    } else if (e.ctrlKey || e.metaKey) {
      // dash-qt's shortcuts, as the welcome text says: Ctrl+L clears, Ctrl++ and Ctrl+- resize.
      const key = e.key.toLowerCase();
      if (key === "l") model.clear();
      else if (key === "+" || key === "=") model.setFontSize(model.fontSize + 1);
      else if (key === "-") model.setFontSize(model.fontSize - 1);
      else return;
      e.preventDefault();
    }
  };

  return (
    <div className="tool-stack console" data-testid="tools-console">
      <div className="console-bar">
        {model.showsWalletSelector && (
          <label className="inline-field">
            Wallet
            <select data-testid="console-wallet" value={model.walletId ?? ""} onChange={(e) => model.selectWallet(e.target.value || null)}>
              <option value="">(none)</option>
              {model.wallets.map((w) => (
                <option key={w.walletId} value={w.walletId}>
                  {w.name}
                </option>
              ))}
            </select>
          </label>
        )}
        <span className="console-font" role="group" aria-label="Font size">
          <DashButton variant="tintedGray" size="small" data-testid="console-font-smaller" aria-label="Decrease font size" disabled={model.fontSize <= FONT_SIZES.min} onClick={() => model.setFontSize(model.fontSize - 1)}>
            A−
          </DashButton>
          <DashButton variant="tintedGray" size="small" data-testid="console-font-larger" aria-label="Increase font size" disabled={model.fontSize >= FONT_SIZES.max} onClick={() => model.setFontSize(model.fontSize + 1)}>
            A+
          </DashButton>
        </span>
        <DashButton variant="tintedGray" size="small" data-testid="console-clear" onClick={() => model.clear()}>
          Clear console
        </DashButton>
      </div>
      <div
        ref={log}
        className="console-log card-l1"
        role="log"
        aria-live="polite"
        aria-label="Console output"
        tabIndex={0}
        data-testid="console-log"
        style={{ fontSize: `${model.fontSize}px` }}
      >
        {model.entries.map((entry) => (
          <div key={entry.id} className={`console-entry console-${entry.kind}`} data-testid={`console-entry-${entry.kind}`}>
            {entry.kind === "command" && <span aria-hidden="true">&gt; </span>}
            {entry.isJson ? <pre className="console-json">{entry.text}</pre> : <span className="console-text">{entry.text}</span>}
          </div>
        ))}
      </div>
      <form
        className="console-input"
        onSubmit={(e) => {
          e.preventDefault();
          void model.run(line).then((ran) => ran && setLine(""));
        }}
      >
        <label className="sr-only" htmlFor={id}>
          Console command
        </label>
        <input
          id={id}
          data-testid="console-line"
          className="mono"
          value={line}
          autoComplete="off"
          autoCapitalize="off"
          spellCheck={false}
          list={`${id}-commands`}
          aria-invalid={!!model.error}
          aria-describedby={model.error ? `${id}-error` : undefined}
          onChange={(e) => setLine(e.target.value)}
          onKeyDown={onKeyDown}
        />
        <datalist id={`${id}-commands`}>
          {model.completions("").map((n) => (
            <option key={n} value={n} />
          ))}
        </datalist>
        <DashButton type="submit" variant="filledBlue" size="small" data-testid="console-run" disabled={model.running || line.trim() === ""}>
          {model.running ? "Running…" : "Run"}
        </DashButton>
      </form>
      {model.error && (
        <p id={`${id}-error`} className="form-problem" role="alert" data-testid="console-error">
          {model.error}
        </p>
      )}
    </div>
  );
}
