// The CoinJoin Home card (U-06 D1), Android's `MixingStatusCard`: only while mixing does something
// or stopped without being asked to. The model is coinjoin/model.ts `coinjoinView`; the stop
// confirmation and the button's dispatch are shared with the page.

import { useId, useState } from "react";
import type { NetworkInfo } from "../../api/generated/NetworkInfo";
import { navigate } from "../../app/ui";
import { DashButton, Icon } from "../../components/basics";
import { Sheet } from "../../components/sheet";
import { CoinJoinText, type CoinJoinView, type ViewActionKind } from "../../coinjoin/model";
import { useCoinJoinView, useDismissedStop, useMixingAction } from "../../coinjoin/queries";

/** The stop confirmation (D8). */
export function StopSheet({ onStop, onClose }: { onStop: () => void; onClose: () => void }) {
  return (
    <Sheet title={CoinJoinText.stopTitle} width="small" testId="coinjoin-stop-sheet" onClose={onClose}>
      <p className="sheet-text">{CoinJoinText.stopMessage}</p>
      <div className="sheet-actions">
        <DashButton variant="tintedGray" data-autofocus onClick={onClose}>
          Cancel
        </DashButton>
        <DashButton
          variant="filledBlue"
          data-testid="coinjoin-stop-confirm"
          onClick={() => {
            onClose();
            onStop();
          }}
        >
          {CoinJoinText.stop}
        </DashButton>
      </div>
    </Sheet>
  );
}

/** What the view's button does, the stop sheet it may open, and the call's busy state and
 * failure (D9). */
export function useViewAction(walletId: string, network: NetworkInfo, retry: () => void) {
  const mixing = useMixingAction(walletId, network);
  const [confirming, setConfirming] = useState(false);
  const press = (kind: ViewActionKind) => {
    if (kind === "start") return void mixing.run("start");
    if (kind === "stop") return setConfirming(true);
    if (kind === "retry") return retry();
    navigate("settings", { settingsTab: "coinjoin" });
  };
  const sheet = confirming && <StopSheet onStop={() => void mixing.run("stop")} onClose={() => setConfirming(false)} />;
  return { press, sheet, busy: mixing.busy, problem: mixing.problem };
}

/** The view's percent bar, "X of Y" and status line; the card and the page show the same. */
export function ViewFacts({ view, testId }: { view: CoinJoinView; testId: string }) {
  return (
    <>
      {view.percent !== null && (
        <div className="coinjoin-progress-row">
          <progress className="coinjoin-progress" max={100} value={view.percent} aria-label="Mixing progress" />
          <span data-testid={`${testId}-percent`}>{view.percent}%</span>
        </div>
      )}
      {view.mixed && (
        <p className="coinjoin-mixed" data-testid={`${testId}-mixed`}>
          {view.mixed}
        </p>
      )}
      {view.line && (
        <p className="coinjoin-line" data-testid={`${testId}-line`}>
          {view.line}
        </p>
      )}
    </>
  );
}

export function CoinJoinCard({ walletId, network }: { walletId: string; network: NetworkInfo }) {
  const id = useId();
  const { view, status, retry } = useCoinJoinView(walletId, network);
  const { isDismissed, dismiss } = useDismissedStop(walletId, status?.state);
  const act = useViewAction(walletId, network, retry);
  if (!view.onHome || (view.dismissable && isDismissed)) return null;
  const { action } = view;
  return (
    <section className="coinjoin-card card-l1" data-testid="coinjoin-card" data-state={view.state} aria-labelledby={`${id}-title`}>
      <Icon id="coinjoinMixing" size={40} className="coinjoin-card-icon" />
      <div className="coinjoin-card-body">
        <div role="status" className="coinjoin-card-live">
          <h2 className="coinjoin-card-title" id={`${id}-title`}>
            {/* The card opens the CoinJoin page (D1). */}
            <button type="button" className="coinjoin-card-open" data-testid="coinjoin-card-open" onClick={() => navigate("coinJoin")}>
              {view.title}
            </button>
            {view.busy && <span className="spinner coinjoin-spinner" aria-hidden="true" />}
          </h2>
          <ViewFacts view={view} testId="coinjoin-card" />
        </div>
        {act.problem && (
          <p className="form-problem" role="alert" data-testid="coinjoin-card-problem">
            {act.problem}
          </p>
        )}
      </div>
      <div className="coinjoin-card-actions">
        {action && (
          <DashButton
            variant={action.kind === "stop" ? "tintedBlue" : "filledBlue"}
            size="small"
            data-testid="coinjoin-card-action"
            disabled={action.disabled || act.busy}
            onClick={() => act.press(action.kind)}
          >
            {action.label}
          </DashButton>
        )}
        {view.dismissable && (
          <DashButton variant="plainBlue" size="small" data-testid="coinjoin-card-dismiss" onClick={dismiss}>
            {CoinJoinText.dismiss}
          </DashButton>
        )}
      </div>
      {act.sheet}
    </section>
  );
}
