// Change keys or payout (MG-10 §1.7, `upd-registrar`) and the operator explanation sheet
// (`mn-operator`, MG-00 D24). The update is a ProUpRegTx signed with the owner key under a
// `masternodeOp` grant for the preview's debit; the wallet never signs with an operator key, so
// service updates and revocations are explained, not offered.

import { useEffect, useRef, useState } from "react";
import { keepPreviousData, useQuery } from "@tanstack/react-query";
import type { MasternodeDetail } from "../../api/generated/MasternodeDetail";
import type { MasternodeRow } from "../../api/generated/MasternodeRow";
import type { NetworkInfo } from "../../api/generated/NetworkInfo";
import type { ProTxOperation } from "../../api/generated/ProTxOperation";
import type { UpdateRegistrarRequest } from "../../api/generated/UpdateRegistrarRequest";
import type { WalletSummary } from "../../api/generated/WalletSummary";
import { useBackend } from "../../api/queries";
import { AmountText, DashButton } from "../../components/basics";
import { Sheet } from "../../components/sheet";
import { useDisplay } from "../../format/display";
import { useT } from "../../i18n";
import {
  buildRequest,
  changeKeysErrorText,
  changesNothing,
  feeWentUp,
  fieldErrors,
  initialForm,
  keepsRecordedPayout,
  pinToPreview,
  recordedPayout,
  registrarUpdateSends,
  reviewLines,
  signerLine,
  unsentUpdate,
  type ChangeKeysForm,
} from "../../masternodes/changeKeys";
import { registrarLine } from "../../masternodes/model";
import { mnKeys, useMasternodeOperations, useMasternodesRefresh } from "../../masternodes/queries";
import { CredentialField, useCredential } from "./credential";

const P = "masternodes.myMasternodes.changeKeys";
const DEBOUNCE_MS = 300;

function useDebounced<T>(value: T): T {
  const [v, setV] = useState(value);
  useEffect(() => {
    const id = window.setTimeout(() => setV(value), DEBOUNCE_MS);
    return () => window.clearTimeout(id);
  }, [value]);
  return v;
}

type Done = { kind: "sent"; op: ProTxOperation; evonodePayout: boolean } | { kind: "discarded" };

export function ChangeKeysSheet({
  wallet,
  network,
  row,
  detail,
  onClose,
}: {
  wallet: WalletSummary;
  network: NetworkInfo;
  row: MasternodeRow;
  detail: MasternodeDetail;
  onClose: () => void;
}) {
  const t = useT();
  const b = useBackend();
  const sends = registrarUpdateSends(network);
  const shown = useDisplay(network);
  const refresh = useMasternodesRefresh();
  const ops = useMasternodeOperations(wallet.walletId);
  const recorded = recordedPayout(detail);
  const [form, setForm] = useState<ChangeKeysForm>(() => initialForm(detail));
  // Q4: the recorded payout is a probe until a preview reads it as no change; a choice the user makes ends it.
  const [probing, setProbing] = useState(form.payout === "recorded");
  const [busy, setBusy] = useState(false);
  const [done, setDone] = useState<Done | null>(null);
  const [unsent, setUnsent] = useState<ProTxOperation | null>(null);
  const credential = useCredential(wallet.walletId, t(`${P}.ask`), null);
  const errors = fieldErrors(form);
  const request = buildRequest(row.proTxHash, form, recorded);
  const live = request === null ? null : JSON.stringify(request);
  const debounced = useDebounced(live);
  const stuck = unsent ?? unsentUpdate(ops.data, row.proTxHash);
  const preview = useQuery({
    queryKey: [...mnKeys.detail(wallet.walletId, row.proTxHash), "updateRegistrar", debounced],
    queryFn: () => b.mnPreviewUpdateRegistrar(wallet.walletId, JSON.parse(debounced!) as UpdateRegistrarRequest),
    enabled: debounced !== null && done === null && stuck === null && ops.isSuccess,
    placeholderData: keepPreviousData,
    retry: false,
  });
  // Only a settled, successful preview of exactly the live request can be signed (a failed refetch keeps old data).
  const current = live !== null && debounced === live && preview.isSuccess && !preview.isPlaceholderData ? preview.data : undefined;
  const nothing = current !== undefined && changesNothing(current);
  const set = (patch: Partial<ChangeKeysForm>) => setForm((f) => ({ ...f, ...patch }));
  const pickPayout = (patch: Partial<ChangeKeysForm>) => {
    setProbing(false);
    set(patch);
  };
  const listOperator = detail.list?.operatorPublicKey ?? "";
  useEffect(() => {
    if (!probing || !current || form.payout !== "recorded") return;
    setProbing(false);
    if (!keepsRecordedPayout(current)) set({ payout: null });
  }, [probing, current, form.payout]);
  const signable = current !== undefined && !nothing && !probing;

  const finish = (op: ProTxOperation, payoutChanges: boolean | null) => {
    void refresh();
    if (op.state.kind === "not_sent" || op.state.kind === "signed") setUnsent(op);
    else setDone({ kind: "sent", op, evonodePayout: row.kind === "evonode" && payoutChanges !== false });
  };

  const sending = useRef(false);
  const send = async () => {
    if (!sends || sending.current || busy || !request || !current || !signable || !credential.ready) return;
    sending.current = true;
    setBusy(true);
    try {
      const grant = await credential.authorize({ kind: "masternodeOp", maxDuffs: current.totalDebit });
      if (!grant) return;
      finish(await b.mnUpdateRegistrar(wallet.walletId, pinToPreview(request, current), grant.id), current.payoutChanges);
    } catch (e) {
      credential.setError(changeKeysErrorText(e));
      if (feeWentUp(e)) void preview.refetch();
    } finally {
      sending.current = false;
      setBusy(false);
    }
  };

  if (done)
    return (
      <Sheet title={t(`${P}.title`)} width="medium" testId="mn-change-keys-sheet" onClose={onClose}>
        <div className="mg-sheet-body" data-testid={done.kind === "sent" ? "mn-change-keys-sent" : "mn-change-keys-discarded"}>
          {done.kind === "sent" ? (
            <>
              <p className="mg-flush">{t(`${P}.sent`)}</p>
              {done.op.lastError && <p className="mg-muted mg-small mg-flush" data-testid="mn-change-keys-unsure">{t(`${P}.sentUnsure`)}</p>}
              {done.evonodePayout && <p className="mg-muted mg-small mg-flush" data-testid="mn-change-keys-evonode">{t(`${P}.sentEvonode`)}</p>}
            </>
          ) : (
            <p className="mg-flush">{t(`${P}.notSent.discarded`)}</p>
          )}
          <footer className="mg-sheet-footer">
            <span />
            <span className="mg-actions">
              <DashButton variant="filledBlue" size="medium" data-testid="mn-change-keys-done" data-autofocus onClick={onClose}>
                {t(`${P}.done`)}
              </DashButton>
            </span>
          </footer>
        </div>
      </Sheet>
    );

  if (stuck)
    return (
      <Sheet title={t(`${P}.title`)} width="medium" testId="mn-change-keys-sheet" closable={!busy} onClose={onClose}>
        <UnsentUpdate
          wallet={wallet}
          op={stuck}
          busy={busy}
          setBusy={setBusy}
          sends={sends}
          onSent={(op) => finish(op, null)}
          onDiscarded={() => {
            setUnsent(null);
            void refresh();
            setDone({ kind: "discarded" });
          }}
          onClose={onClose}
        />
      </Sheet>
    );

  return (
    <Sheet title={t(`${P}.title`)} width="medium" testId="mn-change-keys-sheet" closable={!busy} onClose={onClose}>
      <form
        className="mg-sheet-body mg-change-keys"
        onSubmit={(e) => {
          e.preventDefault();
          if (sends) void send();
        }}
      >
        <p className="mg-muted mg-small mg-flush">{t(`${P}.intro`)}</p>
        <div className="mg-choices" role="radiogroup" aria-labelledby="mn-change-operator-title" data-testid="mn-change-operator">
          <span id="mn-change-operator-title" className="mg-section-title">{t(`${P}.operator.legend`)}</span>
          <Choice disabled={busy} name="operator" value="keep" checked={form.operator === "keep"} onPick={() => set({ operator: "keep" })} title={t(`${P}.operator.keep`)} note={listOperator ? t(`${P}.operator.keepValue`, { key: listOperator.slice(0, 16) }) : undefined} mono />
          <Choice disabled={busy} name="operator" value="keychain" checked={form.operator === "keychain"} onPick={() => set({ operator: "keychain" })} title={t(`${P}.operator.keychain`)} note={t(`${P}.operator.keychainNote`)} />
          <Choice disabled={busy} name="operator" value="given" checked={form.operator === "given"} onPick={() => set({ operator: "given" })} title={t(`${P}.operator.given`)} />
          {form.operator === "given" && (
            <TextField disabled={busy} testId="mn-change-operator-key" label={t(`${P}.operator.given`)} help={t(`${P}.operator.givenHelp`)} value={form.operatorKey} error={errors.operatorKey} onChange={(v) => set({ operatorKey: v })} />
          )}
        </div>
        <div className="mg-choices" role="radiogroup" aria-labelledby="mn-change-voting-title" data-testid="mn-change-voting">
          <span id="mn-change-voting-title" className="mg-section-title">{t(`${P}.voting.legend`)}</span>
          <Choice disabled={busy} name="voting" value="keep" checked={form.voting === "keep"} onPick={() => set({ voting: "keep" })} title={t(`${P}.voting.keep`)} note={detail.list?.votingAddress} mono />
          <Choice disabled={busy} name="voting" value="keychain" checked={form.voting === "keychain"} onPick={() => set({ voting: "keychain" })} title={t(`${P}.voting.keychain`)} />
          <Choice disabled={busy} name="voting" value="address" checked={form.voting === "address"} onPick={() => set({ voting: "address" })} title={t(`${P}.voting.address`)} />
          {form.voting === "address" && (
            <TextField disabled={busy} testId="mn-change-voting-input" label={t(`${P}.voting.address`)} help={t(`${P}.voting.addressHelp`)} value={form.votingAddress} error={errors.votingAddress} onChange={(v) => set({ votingAddress: v })} />
          )}
        </div>
        <div className="mg-choices" role="radiogroup" aria-labelledby="mn-change-payout-title" data-testid="mn-change-payout">
          <span id="mn-change-payout-title" className="mg-section-title">{t(`${P}.payout.legend`)}</span>
          {recorded ? (
            <Choice
              disabled={busy}
              name="payout"
              value="recorded"
              checked={form.payout === "recorded"}
              onPick={() => pickPayout({ payout: "recorded" })}
              title={t(`${P}.payout.recorded`, { address: recorded })}
              note={form.payout === "recorded" && !probing ? t(`${P}.payout.recordedNote`) : undefined}
            />
          ) : (
            <p className="mg-muted mg-small mg-flush" data-testid="mn-change-payout-none">
              {t(`${P}.payout.none`)}
            </p>
          )}
          <Choice disabled={busy} name="payout" value="new" checked={form.payout === "new"} onPick={() => pickPayout({ payout: "new" })} title={t(`${P}.payout.newAddress`)} />
          <Choice disabled={busy} name="payout" value="address" checked={form.payout === "address"} onPick={() => pickPayout({ payout: "address" })} title={t(`${P}.payout.address`)} />
          {form.payout === "address" && (
            <TextField disabled={busy} testId="mn-change-payout-input" label={t(`${P}.payout.address`)} help={t(`${P}.payout.addressHelp`)} value={form.payoutAddress} error={errors.payoutAddress} onChange={(v) => set({ payoutAddress: v })} />
          )}
        </div>
        <section className="card-l1 mg-change-review" data-testid="mn-change-review" aria-live="polite">
          <h3 className="mg-section-title">{t(`${P}.review.title`)}</h3>
          {preview.isError && request !== null && debounced === live ? (
            <p className="mg-field-error" role="alert" data-testid="mn-change-preview-error">
              {changeKeysErrorText(preview.error)}
            </p>
          ) : current ? (
            <>
              <dl className="mg-facts">
                {reviewLines(current).map((l) => [
                  <dt key={`${l.id}-t`}>{l.title}</dt>,
                  <dd key={`${l.id}-d`} data-testid={`mn-change-review-${l.id}`}>
                    <span className="mono">{l.value}</span> · {l.changes ? <strong>{l.status}</strong> : <span className="mg-muted">{l.status}</span>}
                  </dd>,
                ])}
                <dt>{t(`${P}.review.fee`)}</dt>
                <dd data-testid="mn-change-review-fee">
                  <AmountText text={shown.compact(current.fee)} unitName={shown.name} />
                </dd>
              </dl>
              <p className="mg-muted mg-small mg-flush" data-testid="mn-change-signer">
                {signerLine(current)}
              </p>
              {current.poseBans && (
                <p className="mg-notice mg-notice-danger" data-testid="mn-change-pose">
                  <strong>{t(`${P}.poseWarning`)}</strong> {t(`${P}.poseExplain`)}
                </p>
              )}
              {nothing && (
                <p className="mg-notice mg-notice-warning" data-testid="mn-change-nothing">
                  {t(`${P}.nothing`)}
                </p>
              )}
            </>
          ) : request !== null ? (
            <p className="mg-muted mg-small mg-flush">{t(`${P}.review.checking`)}</p>
          ) : null}
        </section>
        {sends ? <CredentialField c={credential} busy={busy} testId="mn-change" /> : <HeldLine />}
        <footer className="mg-sheet-footer">
          <span />
          <span className="mg-actions">
            <DashButton variant="tintedGray" size="medium" data-testid="mn-change-cancel" disabled={busy} onClick={onClose}>
              {t("common.cancel")}
            </DashButton>
            {sends && (
              <DashButton type="submit" variant="filledBlue" size="medium" data-testid="mn-change-send" disabled={busy || !signable || !credential.ready}>
                {busy ? t(`${P}.sending`) : t(`${P}.send`)}
              </DashButton>
            )}
          </span>
        </footer>
      </form>
    </Sheet>
  );
}

/** A signed update that didn't go out: send it again under a grant for its debit, or discard it (MG-04 D7). */
function UnsentUpdate({
  wallet,
  op,
  sends,
  busy,
  setBusy,
  onSent,
  onDiscarded,
  onClose,
}: {
  wallet: WalletSummary;
  op: ProTxOperation;
  sends: boolean;
  busy: boolean;
  setBusy: (b: boolean) => void;
  onSent: (op: ProTxOperation) => void;
  onDiscarded: () => void;
  onClose: () => void;
}) {
  const t = useT();
  const b = useBackend();
  const credential = useCredential(wallet.walletId, t(`${P}.notSent.ask`), null);
  const acting = useRef(false);
  const act = async (what: "resend" | "discard") => {
    if (acting.current || busy || (what === "resend" && (!sends || !credential.ready))) return;
    acting.current = true;
    setBusy(true);
    let granted: string | null = null;
    try {
      if (what === "discard") {
        await b.mnDiscardOperation(wallet.walletId, op.id);
        onDiscarded();
        return;
      }
      const grant = await credential.authorize({ kind: "masternodeOp", maxDuffs: op.debit });
      if (!grant) return;
      granted = grant.id;
      onSent(await b.mnResendOperation(wallet.walletId, op.id, grant.id));
    } catch (e) {
      // A refusal before the lease leaves the grant unused.
      if (granted) void b.revokeGrant(granted).catch(() => null);
      credential.setError(changeKeysErrorText(e));
    } finally {
      acting.current = false;
      setBusy(false);
    }
  };
  return (
    <form
      className="mg-sheet-body"
      data-testid="mn-change-unsent"
      onSubmit={(e) => {
        e.preventDefault();
        if (sends) void act("resend");
      }}
    >
      <p className="mg-notice mg-notice-warning">
        <strong>{t(`${P}.notSent.title`)}</strong> {t(`${P}.notSent.text`)}
      </p>
      {op.lastError === "send.no_peers" && <p className="mg-muted mg-small mg-flush">{t(`${P}.notSent.noPeers`)}</p>}
      {sends ? (
        <CredentialField c={credential} busy={busy} testId="mn-change-unsent" />
      ) : (
        <>
          <HeldLine />
          <p className="form-problem" role="alert" data-testid="mn-change-unsent-error">
            {credential.error}
          </p>
        </>
      )}
      <footer className="mg-sheet-footer">
        <span>
          <DashButton variant="tintedGray" size="medium" data-testid="mn-change-unsent-discard" disabled={busy} onClick={() => void act("discard")}>
            {t(`${P}.notSent.discard`)}
          </DashButton>
        </span>
        <span className="mg-actions">
          <DashButton variant="tintedGray" size="medium" disabled={busy} onClick={onClose}>
            {t("common.cancel")}
          </DashButton>
          {sends && (
            <DashButton type="submit" variant="filledBlue" size="medium" data-testid="mn-change-unsent-resend" disabled={busy || !credential.ready}>
              {busy ? t(`${P}.notSent.resending`) : t(`${P}.notSent.resend`)}
            </DashButton>
          )}
        </span>
      </footer>
    </form>
  );
}

function HeldLine() {
  const t = useT();
  return (
    <p className="mg-notice mg-notice-warning" data-testid="mn-change-held">
      {t(`${P}.held`)}
    </p>
  );
}

function Choice({ name, value, checked, onPick, title, note, mono, disabled }: { name: string; value: string; checked: boolean; onPick: () => void; title: string; note?: string; mono?: boolean; disabled?: boolean }) {
  return (
    <label className="mg-choice" data-testid={`mn-change-${name}-${value}`}>
      <input type="radio" name={`mn-change-${name}`} value={value} checked={checked} disabled={disabled} onChange={onPick} />
      <span className="mg-choice-text">
        <span>{title}</span>
        {note && <span className={`mg-muted mg-small${mono ? " mono" : ""}`}>{note}</span>}
      </span>
    </label>
  );
}

function TextField({ testId, label, help, value, error, onChange, disabled }: { testId: string; label: string; help: string; value: string; error?: string; onChange: (v: string) => void; disabled?: boolean }) {
  return (
    <label className={`mg-field ${error ? "has-error" : ""}`}>
      <span className="sr-only">{label}</span>
      <input className="mono" data-testid={testId} value={value} disabled={disabled} autoComplete="off" spellCheck={false} aria-invalid={!!error} data-autofocus onChange={(e) => onChange(e.target.value)} />
      <span className="mg-field-help">{help}</span>
      {error && (
        <span className="mg-field-error" role="alert" data-testid={`${testId}-error`}>
          {error}
        </span>
      )}
    </label>
  );
}

/** More ▸ Service update or revoke…: who acts, worded by custody (MG-00 D24 f1, MG-10 G19). */
export function OperatorSheet({ row, detail, onChangeKeys, onClose }: { row: MasternodeRow; detail: MasternodeDetail | undefined; onChangeKeys: (() => void) | null; onClose: () => void }) {
  const t = useT();
  const O = "masternodes.myMasternodes.operatorSheet";
  const canReplace = detail?.registrar.kind === "available";
  const holdsCollateral = row.roles.collateral;
  // An owner key held (the keychain's or one added) without the registration: say why it can't replace the operator.
  const holdsOwnerKey = row.roles.owner || (row.roles.attached ?? []).some((k) => k.role === "owner");
  const reason = detail ? registrarLine(detail)?.text : undefined;
  return (
    <Sheet title={t(`${O}.title`)} width="small" testId="mn-operator-sheet" onClose={onClose}>
      <div className="mg-sheet-body mg-operator-body">
        <p className="mg-flush" data-testid="mn-operator-intro">
          {t(row.roles.operator ? `${O}.selfRun` : `${O}.hosted`)}
        </p>
        <p className="mg-muted mg-small mg-flush">{t(`${O}.covers`)}</p>
        {canReplace || holdsCollateral ? (
          <>
            <p className="mg-flush">{t(`${O}.instead`)}</p>
            <ul className="mg-operator-list" data-testid="mn-operator-instead">
              {canReplace && <li data-testid="mn-operator-replace">{t(`${O}.replace`)}</li>}
              {holdsCollateral && <li data-testid="mn-operator-end">{t(`${O}.end`)}</li>}
            </ul>
          </>
        ) : holdsOwnerKey && reason ? (
          <p className="mg-flush" data-testid="mn-operator-reason">
            {reason}
          </p>
        ) : (
          <p className="mg-flush" data-testid="mn-operator-elsewhere">
            {t(`${O}.elsewhere`)}
          </p>
        )}
        <footer className="mg-sheet-footer">
          <span />
          <span className="mg-actions">
            {canReplace && onChangeKeys && (
              <DashButton variant="tintedBlue" size="medium" data-testid="mn-operator-change-keys" onClick={onChangeKeys}>
                {t("masternodes.myMasternodes.changeKeys.action")}
              </DashButton>
            )}
            <DashButton variant="filledBlue" size="medium" data-testid="mn-operator-close" data-autofocus onClick={onClose}>
              {t(`${O}.close`)}
            </DashButton>
          </span>
        </footer>
      </div>
    </Sheet>
  );
}
