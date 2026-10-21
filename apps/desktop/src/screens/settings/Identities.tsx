// Settings ▸ Advanced ▸ Identities (DP6-01, IOS-071; design note "UI"): the wallet's identities
// with the main one marked (L1–L5), each identity's keys (the detail sheet), Refresh, Refresh all,
// Find my identities and the read-only look-up. The view models are dashpay/identities.ts.

import { useId, useState } from "react";
import type { NetworkInfo } from "../../api/generated/NetworkInfo";
import { mainIdentityOf, useBackend, useDashPayShell, useWallets } from "../../api/queries";
import { navigate, useUi } from "../../app/ui";
import { DashButton } from "../../components/basics";
import { CopyValue, DetailRow } from "../../components/detail";
import { Sheet } from "../../components/sheet";
import {
  IDENTITIES_COPY as C,
  identitiesFor,
  identityErrorText,
  keyRows,
  listModel,
  shortId,
  type IdentitiesModel,
  type IdentityRow,
  type KeyRow,
} from "../../dashpay/identities";
import { useIdentityDetail, useIdentityList } from "../../dashpay/identityQueries";
import { creditBalance, PROFILE_COPY } from "../../dashpay/profile";
import { useCostTable } from "../../dashpay/profileQueries";
import { useDisplay } from "../../format/display";
import { useModel } from "../../onboarding/model";
import { Avatar } from "../contacts/Avatar";
import { chooseName } from "../dashpay/MyProfile";
import { GrantSheet } from "../tools/common";
import { SettingsCard } from "./parts";

export function AdvancedTab({ network }: { network: NetworkInfo }) {
  const walletId = useUi((s) => s.walletId);
  const wallets = useWallets();
  const wallet = wallets.data?.find((w) => w.walletId === walletId) ?? wallets.data?.[0];
  if (!wallet) return null;
  return <IdentitiesCard key={wallet.walletId} walletId={wallet.walletId} network={network} />;
}

function IdentitiesCard({ walletId, network }: { walletId: string; network: NetworkInfo }) {
  const b = useBackend();
  const model = useModel(identitiesFor(b, walletId));
  const list = useIdentityList(walletId);
  const shell = useDashPayShell(walletId);
  const costs = useCostTable(walletId).data;
  const shown = useDisplay(network);
  const discreet = useUi((s) => s.discreet);
  const [open, setOpen] = useState<string | null>(null);
  const [finding, setFinding] = useState(false);
  const page = listModel(list);
  const balance = (credits: string | null) =>
    credits === null ? C.unknown : creditBalance(credits, costs, discreet, (duffs) => `${shown.compact(duffs)} ${shown.name}`, shown.masked());
  const busy = model.running !== null;
  const header = model.result?.identity === null ? model.result : null;
  return (
    <SettingsCard title={C.title} testId="identities-page">
      <p className="field-hint settings-hint">{C.intro}</p>
      <div className="identities-actions">
        <DashButton variant="tintedBlue" size="small" data-testid="identities-refresh-all" disabled={busy || page.kind !== "rows"} onClick={() => void model.refreshAll()}>
          {C.refreshAll}
        </DashButton>
        <DashButton variant="tintedBlue" size="small" data-testid="identities-find-mine" disabled={busy} onClick={() => void model.discover()}>
          {C.findMine}
        </DashButton>
        <DashButton variant="tintedGray" size="small" data-testid="identities-look-up" aria-expanded={finding} onClick={() => setFinding(!finding)}>
          {C.lookUp}
        </DashButton>
      </div>
      {header && <ResultLine ok={header.ok} text={header.text} testId="identities-result" />}
      {finding && <FindPanel model={model} balance={balance} />}
      {page.kind === "loading" && (
        <p className="placeholder-note" role="status" data-testid="identities-loading">
          {C.loading}
        </p>
      )}
      {page.kind === "failed" && (
        <div className="identities-empty" data-testid="identities-failed">
          <p className="form-problem" role="alert">
            {C.readFailed}
          </p>
          <DashButton variant="tintedGray" size="small" data-testid="identities-retry" onClick={() => void list.refetch()}>
            {C.tryAgain}
          </DashButton>
        </div>
      )}
      {page.kind === "none" && (
        <div className="identities-empty" data-testid="identities-none">
          <p>{C.none}</p>
          {mainIdentityOf(shell.status) === null && (
            <DashButton variant="tintedBlue" size="small" data-testid="identities-join" onClick={() => navigate("joinDashPay")}>
              {PROFILE_COPY.settings.join}
            </DashButton>
          )}
        </div>
      )}
      {page.kind === "rows" && (
        <ul className="identities-list" data-testid="identities-list">
          {page.rows.map((row) => (
            <IdentityRowView key={row.identity} row={row} model={model} balance={balance} onKeys={() => setOpen(row.identity)} />
          ))}
        </ul>
      )}
      {open && <DetailSheet walletId={walletId} identity={open} model={model} onClose={() => setOpen(null)} />}
      <GrantSheet grants={model.grants} />
    </SettingsCard>
  );
}

function ResultLine({ ok, text, testId }: { ok: boolean; text: string; testId: string }) {
  return (
    <p className={ok ? "form-ok" : "form-problem"} role={ok ? "status" : "alert"} data-testid={testId}>
      {text}
    </p>
  );
}

function IdentityRowView({ row, model, balance, onKeys }: { row: IdentityRow; model: IdentitiesModel; balance: (c: string | null) => string; onKeys: () => void }) {
  const b = useBackend();
  const walletId = model.walletId;
  const busy = model.running !== null;
  const result = model.result?.identity === row.identity ? model.result : null;
  return (
    <li className="identity-row" data-testid="identity-row" data-identity={row.identity} data-main={row.isMain || undefined}>
      <Avatar letters={row.initials} identity={row.identity} own />
      <div className="identity-row-text">
        <p className="identity-row-title">
          <span data-testid="identity-username">{row.username}</span>
          {row.isMain && (
            <span className="identity-badge" data-testid="identity-main-badge">
              {C.main}
            </span>
          )}
          {row.unverified && (
            <span className="identity-badge identity-badge-warn" data-testid="identity-unverified">
              {C.unverified}
            </span>
          )}
        </p>
        <p className="field-hint">
          {row.index} · <CopyValue value={row.identity} display={shortId(row.identity)} label="identity id" mono />
        </p>
        <p className="identity-row-balance" data-testid="identity-balance">
          {balance(row.credits)}
          {row.stale && row.credits !== null && <span className="profile-credits-updating">{C.updating}</span>}
        </p>
        {result && <ResultLine ok={result.ok} text={result.text} testId="identity-result" />}
      </div>
      <div className="identity-row-actions">
        {!row.isMain && (
          <DashButton variant="tintedBlue" size="small" data-testid="identity-set-main" disabled={busy} onClick={() => void model.setMain(row.identity)}>
            {C.setMain}
          </DashButton>
        )}
        {row.noName && (
          <DashButton variant="tintedBlue" size="small" data-testid="identity-choose-name" onClick={() => chooseName(b, walletId, row.identity, false)}>
            {PROFILE_COPY.chooseName}
          </DashButton>
        )}
        <DashButton variant="tintedGray" size="small" data-testid="identity-keys" onClick={onKeys}>
          {C.keys}
        </DashButton>
        <DashButton variant="tintedGray" size="small" data-testid="identity-refresh" disabled={busy} onClick={() => void model.refresh(row.identity)}>
          {model.running === `refresh:${row.identity}` ? C.updating : C.refresh}
        </DashButton>
      </div>
    </li>
  );
}

export function KeysTable({ keys, testId }: { keys: KeyRow[]; testId: string }) {
  const T = C.table;
  return (
    <div className="identity-keys-wrap">
      <table className="data-table identity-keys" data-testid={testId}>
        <thead>
          <tr>
            <th scope="col">{T.id}</th>
            <th scope="col">{T.purpose}</th>
            <th scope="col">{T.level}</th>
            <th scope="col">{T.type}</th>
            <th scope="col">{T.status}</th>
            <th scope="col">{T.bound}</th>
          </tr>
        </thead>
        <tbody>
          {keys.map((k) => (
            <tr key={k.id} data-testid="identity-key" data-status={k.status}>
              <td>{k.id}</td>
              <td>{k.purpose}</td>
              <td>{k.level}</td>
              <td>{k.type}</td>
              <td data-testid="identity-key-status">{k.statusText}</td>
              <td>{k.bound ?? "—"}</td>
            </tr>
          ))}
        </tbody>
      </table>
    </div>
  );
}

function DetailSheet({ walletId, identity, model, onClose }: { walletId: string; identity: string; model: IdentitiesModel; onClose: () => void }) {
  const q = useIdentityDetail(walletId, identity);
  const D = C.detail;
  const d = q.data;
  const result = model.result?.identity === identity ? model.result : null;
  return (
    <Sheet title={D.title} width="large" testId="identity-detail" onClose={onClose}>
      {d === undefined ? (
        q.isError ? (
          <div data-testid="identity-detail-failed">
            <p className="form-problem" role="alert">
              {identityErrorText(q.error)}
            </p>
            <DashButton variant="tintedGray" size="small" data-testid="identity-detail-retry" onClick={() => void q.refetch()}>
              {C.tryAgain}
            </DashButton>
          </div>
        ) : (
          <p className="placeholder-note" role="status">
            {D.loading}
          </p>
        )
      ) : (
        <>
          <dl className="detail-list">
            <DetailRow label={D.identity} testId="identity-detail-id">
              <CopyValue value={d.summary.identity} label="identity id" mono />
            </DetailRow>
            <DetailRow label={D.index} testId="identity-detail-index">
              {d.summary.index}
            </DetailRow>
            <DetailRow label={D.revision} testId="identity-detail-revision">
              {d.revision === null ? C.unknown : d.revision}
            </DetailRow>
            <DetailRow label={D.usernames} testId="identity-detail-names">
              {d.summary.names.length === 0 ? (
                PROFILE_COPY.noUsername
              ) : (
                <ul className="identity-names">
                  {d.summary.names.map((n) => (
                    <li key={n} data-testid="identity-name" data-main={n === d.summary.mainName || undefined}>
                      @{n}
                      {n === d.summary.mainName ? (
                        <span className="identity-badge">{D.mainName}</span>
                      ) : (
                        <DashButton
                          variant="tintedGray"
                          size="small"
                          data-testid="identity-set-main-name"
                          disabled={model.running !== null}
                          onClick={() => void model.setMainName(identity, n)}
                        >
                          {D.setMainName}
                        </DashButton>
                      )}
                    </li>
                  ))}
                </ul>
              )}
            </DetailRow>
          </dl>
          {result && !result.ok && <ResultLine ok={false} text={result.text} testId="identity-detail-result" />}
          <h3 className="card-title">{D.keys}</h3>
          {d.publicKeys.length === 0 ? (
            <p data-testid="identity-detail-no-keys">{D.noKeys}</p>
          ) : (
            <KeysTable keys={keyRows(d.publicKeys)} testId="identity-detail-keys" />
          )}
        </>
      )}
      <div className="sheet-actions">
        <DashButton variant="tintedGray" size="large" data-testid="identity-detail-close" onClick={onClose}>
          {D.close}
        </DashButton>
      </div>
    </Sheet>
  );
}

function FindPanel({ model, balance }: { model: IdentitiesModel; balance: (c: string | null) => string }) {
  const F = C.find;
  const id = useId();
  const [text, setText] = useState("");
  const s = model.find;
  return (
    <section className="identity-find" data-testid="identity-find" data-state={s.kind} aria-label={F.title}>
      <form
        className="identity-find-form"
        onSubmit={(e) => {
          e.preventDefault();
          void model.lookUp(text);
        }}
      >
        <div className="form-field">
          <label htmlFor={`${id}-query`}>{F.field}</label>
          <input
            id={`${id}-query`}
            data-testid="identity-find-field"
            value={text}
            spellCheck={false}
            autoComplete="off"
            onChange={(e) => setText(e.target.value)}
            aria-invalid={s.kind === "invalid" || undefined}
          />
        </div>
        <DashButton variant="filledBlue" size="small" type="submit" data-testid="identity-find-submit" disabled={s.kind === "searching"}>
          {F.submit}
        </DashButton>
      </form>
      {s.kind === "invalid" && <ResultLine ok={false} text={F.invalid} testId="identity-find-invalid" />}
      {s.kind === "searching" && (
        <p className="placeholder-note" role="status">
          {F.searching}
        </p>
      )}
      {s.kind === "notFound" && (
        <p role="status" data-testid="identity-find-none">
          {F.notFound}
        </p>
      )}
      {s.kind === "failed" && (
        <div data-testid="identity-find-failed">
          <ResultLine ok={false} text={s.text} testId="identity-find-error" />
          <DashButton variant="tintedGray" size="small" data-testid="identity-find-retry" onClick={() => void model.lookUp(s.query)}>
            {C.tryAgain}
          </DashButton>
        </div>
      )}
      {s.kind === "found" && (
        <div className="identity-found" data-testid="identity-found">
          <dl className="detail-list">
            <DetailRow label={C.detail.identity} testId="identity-found-id">
              <CopyValue value={s.found.identity} label="identity id" mono />
            </DetailRow>
            <DetailRow label={C.detail.usernames} testId="identity-found-names">
              {s.found.usernames}
            </DetailRow>
            <DetailRow label={F.balance} testId="identity-found-balance">
              {balance(s.found.credits)}
            </DetailRow>
            <DetailRow label={C.detail.revision} testId="identity-found-revision">
              {s.found.revision}
            </DetailRow>
          </dl>
          <p className="field-hint" data-testid="identity-found-how">
            {s.found.how}
          </p>
          {s.found.own && (
            <p className="identity-badge" data-testid="identity-found-own">
              {F.own}
            </p>
          )}
          {s.found.unverified && (
            <p className="identity-badge identity-badge-warn" data-testid="identity-found-unverified">
              {C.unverified}
            </p>
          )}
          {s.found.keys.length > 0 && <KeysTable keys={s.found.keys} testId="identity-found-keys" />}
        </div>
      )}
    </section>
  );
}
