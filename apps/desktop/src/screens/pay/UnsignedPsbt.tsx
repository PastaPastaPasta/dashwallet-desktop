// Advanced: an unsigned transaction (P15). The payment as entered becomes a PSBT to sign
// elsewhere, reviewed on the confirm sheet and then opened in Tools ▸ PSBT to copy or save.
// Closed by default; open from the start for a watch-only wallet, whose only way to pay it is.

import type { Ref } from "react";
import { DashButton } from "../../components/basics";

export function UnsignedPsbt({
  watchOnly,
  why,
  onCreate,
  buttonRef,
  summaryRef,
}: {
  watchOnly: boolean;
  why: string | null;
  onCreate: () => void;
  buttonRef: Ref<HTMLButtonElement>;
  summaryRef: Ref<HTMLElement>;
}) {
  return (
    <details className="disclosure card-l1" data-testid="pay-advanced-psbt" open={watchOnly || undefined}>
      <summary ref={summaryRef}>Advanced: unsigned transaction (PSBT)</summary>
      <div className="coin-control-body">
        <p className="placeholder-note" data-testid="pay-psbt-note">
          {watchOnly
            ? "This wallet is watch-only, so it cannot sign. Create an unsigned transaction and sign it in the wallet that holds the keys, for example an offline Dash Core wallet."
            : "Nothing is signed or sent. You can copy or save the PSBT and sign it in another wallet, for example an offline Dash Core wallet."}
        </p>
        <div className="tool-actions">
          <DashButton ref={buttonRef} variant={watchOnly ? "filledBlue" : "tintedBlue"} size="small" data-testid="pay-create-psbt" disabled={why !== null} why={why} onClick={onCreate}>
            Create unsigned transaction (PSBT)
          </DashButton>
        </div>
      </div>
    </details>
  );
}
