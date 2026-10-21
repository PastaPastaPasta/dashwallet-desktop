// Tools ▸ Information's DashPay card (DP6-03, DASHPAY F23): the toolbar wallet's DashPay sync state
// (tools/dashpayInfo.ts) and Sync now. The sync status sits under `["dashpay", …]`, so every
// Platform signal re-reads it; sync events re-read it too (D6).

import { useEffect, useEffectEvent, useId, useState } from "react";
import { useQuery } from "@tanstack/react-query";
import { useBackend, useDashPayShell, useWallets } from "../../api/queries";
import { useUi } from "../../app/ui";
import { DashButton } from "../../components/basics";
import { DetailRow } from "../../components/detail";
import { dashpayKeys } from "../../dashpay/queries";
import { failureText } from "../../onboarding/model";
import { dashpayInfo, INFO_COPY } from "../../tools/dashpayInfo";
import { Outcome, ToolCard, useReloadOnSync } from "./common";

/** The session's trust reads: no signal changes them (the toggle is set in a developer build only). */
const trustKeys = {
  source: ["trust", "source"] as const,
  trustedOnly: ["trust", "trustedOnly"] as const,
};

/** D3: Sync now's pass reports through no call, so the card reads again this long after asking. */
const SYNC_NOW_REREAD_MS = 5_000;

export function DashPayInfoCard() {
  const b = useBackend();
  const noteId = useId();
  const walletId = useUi((s) => s.walletId);
  const wallets = useWallets();
  const id = (wallets.data?.find((w) => w.walletId === walletId) ?? wallets.data?.[0])?.walletId;
  const noWallet = wallets.data?.length === 0;
  const { status } = useDashPayShell(id);
  const sync = useQuery({ queryKey: dashpayKeys.syncInfo(id ?? ""), queryFn: () => b.dashpaySyncStatus(id!), enabled: !!id });
  const trustedOnly = useQuery({ queryKey: trustKeys.trustedOnly, queryFn: () => b.trustedQuorumsOnly() });
  // Without a wallet the session's own; with one, the sync status carries it.
  const source = useQuery({ queryKey: trustKeys.source, queryFn: () => b.quorumSource(), enabled: noWallet });
  const [asked, setAsked] = useState(0);
  const [asking, setAsking] = useState(false);
  const [outcome, setOutcome] = useState<{ ok: boolean; text: string } | null>(null);

  // `refetch` ignores `enabled`, so each read is guarded as its query is.
  const reread = () => {
    if (id) void sync.refetch();
    if (noWallet) void source.refetch();
  };
  useReloadOnSync(reread);
  const rereadLater = useEffectEvent(reread);
  useEffect(() => {
    if (!asked) return;
    const timer = setTimeout(rereadLater, SYNC_NOW_REREAD_MS);
    return () => clearTimeout(timer);
  }, [asked]);

  const info = dashpayInfo({
    noWallet,
    status,
    sync: sync.data,
    syncError: sync.isError && !sync.data ? failureText(sync.error) : null,
    quorumSource: source.data,
    trustedOnly: trustedOnly.data,
    now: Math.floor(Date.now() / 1000),
  });

  const syncNow = async () => {
    setAsking(true);
    setOutcome(null);
    try {
      await b.dashpaySyncSoon();
      setOutcome({ ok: true, text: INFO_COPY.requested });
      setAsked((n) => n + 1);
    } catch (e) {
      setOutcome({ ok: false, text: `Could not ask for a DashPay pass: ${failureText(e)}` });
    } finally {
      setAsking(false);
    }
  };

  const button = info.syncNow && (
    <DashButton
      variant="tintedBlue"
      size="small"
      data-testid="information-dashpay-sync-now"
      disabled={info.syncNow.disabled || asking}
      aria-describedby={info.syncNow.note ? noteId : undefined}
      onClick={() => void syncNow()}
    >
      {INFO_COPY.syncNow}
    </DashButton>
  );

  return (
    <ToolCard title="DashPay" testId="information-dashpay" actions={button}>
      <div className="dashpay-info-state" role="status" data-testid="information-dashpay-state" data-state={info.state}>
        <span className={`chip chip-${info.tone}`}>{info.chip}</span>
        <span className="dashpay-info-headline">{info.headline}</span>
        {info.busy && <span className="spinner" aria-hidden="true" />}
      </div>
      {info.line && (
        <p className="field-hint dashpay-info-line" data-testid="information-dashpay-line">
          {info.line}
        </p>
      )}
      {info.syncNow?.note && (
        <p className="field-hint dashpay-info-line" id={noteId} data-testid="information-dashpay-sync-note">
          {info.syncNow.note}
        </p>
      )}
      {outcome && <Outcome ok={outcome.ok} text={outcome.text} testId="information-dashpay-outcome" />}
      {info.state === "error" && (
        <div className="tool-actions">
          <DashButton variant="tintedGray" size="small" data-testid="information-dashpay-retry" onClick={() => void sync.refetch()}>
            Try again
          </DashButton>
        </div>
      )}
      {info.rows.length > 0 && (
        <dl className="detail-list" data-testid="information-dashpay-rows">
          {info.rows.map((r) => (
            <DetailRow key={r.title} label={r.title}>
              <span className="tool-value">{r.value}</span>
              {r.note && <span className="field-hint tool-note">{r.note}</span>}
            </DetailRow>
          ))}
        </dl>
      )}
    </ToolCard>
  );
}
