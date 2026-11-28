// Tools ▸ PSBT (QT-076…080): load a PSBT from a file (D2) or a pasted base64 text (D16), or the
// one Pay's Create Unsigned made (D18); dash-qt's PSBT Operations lines and status; Sign under a
// spend grant (D17), Broadcast, Copy and Save…. External signers wait on M6.

import { useEffect, useId, useState } from "react";
import type { NetworkInfo } from "../../api/generated/NetworkInfo";
import { useBackend } from "../../api/queries";
import { openFile } from "../../app/files";
import { ui } from "../../app/ui";
import { DashButton } from "../../components/basics";
import { LoadingState } from "../../components/history";
import { CopiedLabel, useCopy } from "../../components/copy";
import { useModel } from "../../onboarding/model";
import type { GrantModel } from "../../tools/grant";
import { PsbtModel, SESSION_BROADCASTS } from "../../tools/psbt";
import { useAcrossLock } from "../../app/acrossLock";
import { useDisplay } from "../../format/display";
import { useT } from "../../i18n";
import { NotYet, Outcome, ToolCard } from "./common";

export function PsbtTab({ grants, walletId, network }: { grants: GrantModel; walletId: string | null; network: NetworkInfo }) {
  const t = useT();
  const b = useBackend();
  // Amounts in the display unit (U-04 D4); General changes it on another route, which remounts this.
  const shown = useDisplay(network);
  // A broadcast under way or done when the vault locks keeps its PSBT and its line for the unlock
  // (app/acrossLock.ts); the session's log already stops a second broadcast of it.
  const model = useAcrossLock(`tools-psbt:${walletId}`, () => new PsbtModel(b, grants, walletId, shown, SESSION_BROADCASTS), (m) => m.step === "signing" || m.step === "broadcasting" || m.step === "broadcast");
  // A model kept across a lock was made with the page before it: a Sign from now on asks through
  // this page's passphrase sheet, the one on screen (the old one was cancelled with its page).
  model.grants = grants;
  useModel(model);
  // A broadcast by another tab changes what this one offers (D40).
  useModel(model.broadcasts);
  const [pasted, setPasted] = useState("");
  const id = useId();
  const { copied, copy } = useCopy();
  // Pay's Create Unsigned leaves its PSBT here once (D18).
  useEffect(() => {
    const prefill = ui.get().psbtPrefill;
    if (!prefill) return;
    ui.set({ psbtPrefill: null });
    void model.loadUnsigned(prefill.psbt, prefill.copied);
  }, [model]);

  const load = async () => {
    try {
      const file = await openFile(b, "psbt");
      if (file) await model.loadFile(file);
    } catch (e) {
      model.fileFailed(e);
    }
  };

  const { step } = model;
  const busy = step === "loading" || step === "signing" || step === "broadcasting";
  return (
    <div className="tool-stack" data-testid="tools-psbt">
      <ToolCard
        title={t("tools.psbt.loadTitle")}
        testId="psbt-load"
        actions={
          <DashButton variant="tintedBlue" size="small" data-testid="psbt-open-file" disabled={busy} onClick={() => void load()}>
            {t("tools.psbt.loadFromFile")}
          </DashButton>
        }
      >
        <form
          className="tool-stack"
          onSubmit={(e) => {
            e.preventDefault();
            void model.loadText(pasted);
          }}
        >
          <label htmlFor={`${id}-paste`}>{t("tools.psbt.pasteLabel")}</label>
          <textarea
            id={`${id}-paste`}
            className="tool-field tool-message mono"
            data-testid="psbt-paste"
            rows={3}
            spellCheck={false}
            value={pasted}
            onChange={(e) => setPasted(e.target.value)}
          />
          <div className="tool-actions">
            <DashButton type="submit" variant="tintedBlue" size="small" data-testid="psbt-load-text" disabled={busy || pasted.trim() === ""}>
              {t("tools.psbt.loadButton")}
            </DashButton>
          </div>
        </form>
      </ToolCard>
      <Outcome ok={false} text={model.error} testId="psbt-error" reserved />
      {step === "loading" && <LoadingState testId="psbt-loading" text={t("tools.psbt.loading")} />}
      {model.psbt && (
        <ToolCard
          title={t("tools.psbt.operationsTitle")}
          testId="psbt-operations"
          actions={
            <DashButton variant="plainBlue" size="small" data-testid="psbt-close" disabled={busy} onClick={() => model.close()}>
              {t("tools.psbt.close")}
            </DashButton>
          }
        >
          <ul className="psbt-lines" data-testid="psbt-lines">
            {model.lines.map((line, i) => (
              <li key={i}>{line}</li>
            ))}
          </ul>
          {model.status && (
            <p className="placeholder-note" data-testid="psbt-status">
              {model.status}
            </p>
          )}
          {model.notice && (
            <p className="placeholder-note" role="status" data-testid="psbt-notice">
              {model.notice}
            </p>
          )}
          <div className="tool-actions">
            <DashButton variant="filledBlue" size="small" data-testid="psbt-sign" disabled={!model.canSign} onClick={() => void model.sign()}>
              {step === "signing" ? t("tools.psbt.signing") : t("tools.psbt.signButton")}
            </DashButton>
            <DashButton variant="filledBlue" size="small" data-testid="psbt-broadcast" disabled={!model.canBroadcast} onClick={() => void model.broadcast()}>
              {step === "broadcasting" ? t("tools.psbt.broadcasting") : t("tools.psbt.broadcastButton")}
            </DashButton>
            <DashButton
              variant="tintedGray"
              size="small"
              data-testid="psbt-copy"
              disabled={busy || !model.psbt}
              data-copied={copied || undefined}
              onClick={() => void copy(model.psbt!).then((ok) => ok && model.copied())}
            >
              {copied ? <CopiedLabel /> : t("tools.psbt.copyToClipboard")}
            </DashButton>
            <DashButton variant="tintedGray" size="small" data-testid="psbt-save" disabled={busy || !model.psbt} onClick={() => void model.save()}>
              {t("tools.psbt.save")}
            </DashButton>
          </div>
          <Outcome ok text={model.message} testId="psbt-message" reserved />
        </ToolCard>
      )}
      <NotYet title={t("tools.psbt.externalSignerTitle")} testId="psbt-external-signer-not-yet">
        {t("tools.psbt.externalSignerSoon")}
      </NotYet>
    </div>
  );
}
