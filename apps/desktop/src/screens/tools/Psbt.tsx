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
import { showToast } from "../../components/toast";
import { useModel } from "../../onboarding/model";
import type { GrantModel } from "../../tools/grant";
import { PsbtModel, SESSION_BROADCASTS } from "../../tools/psbt";
import { chainOf } from "../pay/PayToAddress";
import { NotYet, Outcome, ToolCard } from "./common";

export function PsbtTab({ grants, walletId, network }: { grants: GrantModel; walletId: string | null; network: NetworkInfo }) {
  const b = useBackend();
  const [model] = useState(() => new PsbtModel(b, grants, walletId, chainOf(network), SESSION_BROADCASTS));
  useModel(model);
  const [pasted, setPasted] = useState("");
  const id = useId();
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
        title="Load a PSBT"
        testId="psbt-load"
        actions={
          <DashButton variant="tintedBlue" size="small" data-testid="psbt-open-file" disabled={busy} onClick={() => void load()}>
            Load from file…
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
          <label htmlFor={`${id}-paste`}>Or paste a PSBT (base64)</label>
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
              Load
            </DashButton>
          </div>
        </form>
      </ToolCard>
      <Outcome ok={false} text={model.error} testId="psbt-error" />
      {step === "loading" && <LoadingState testId="psbt-loading" text="Reading the PSBT" />}
      {model.psbt && (
        <ToolCard
          title="PSBT Operations"
          testId="psbt-operations"
          actions={
            <DashButton variant="plainBlue" size="small" data-testid="psbt-close" disabled={busy} onClick={() => model.close()}>
              Close
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
          <Outcome ok text={model.message} testId="psbt-message" />
          {model.notice && (
            <p className="placeholder-note" role="status" data-testid="psbt-notice">
              {model.notice}
            </p>
          )}
          <div className="tool-actions">
            <DashButton variant="filledBlue" size="small" data-testid="psbt-sign" disabled={!model.canSign} onClick={() => void model.sign()}>
              {step === "signing" ? "Signing…" : "Sign Tx"}
            </DashButton>
            <DashButton variant="filledBlue" size="small" data-testid="psbt-broadcast" disabled={!model.canBroadcast} onClick={() => void model.broadcast()}>
              {step === "broadcasting" ? "Broadcasting…" : "Broadcast Tx"}
            </DashButton>
            <DashButton
              variant="tintedGray"
              size="small"
              data-testid="psbt-copy"
              disabled={busy || !model.psbt}
              onClick={() =>
                void navigator.clipboard.writeText(model.psbt!).then(
                  () => model.copied(),
                  () => showToast("Could not copy"),
                )
              }
            >
              Copy to Clipboard
            </DashButton>
            <DashButton variant="tintedGray" size="small" data-testid="psbt-save" disabled={busy || !model.psbt} onClick={() => void model.save()}>
              Save…
            </DashButton>
          </div>
        </ToolCard>
      )}
      <NotYet title="External signer" owner="M6" testId="psbt-external-signer-not-yet">
        Signing with a hardware wallet through HWI.
      </NotYet>
    </div>
  );
}
