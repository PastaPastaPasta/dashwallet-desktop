// "Sign in to an app" (DashPay App Connect): what the app claims to be and asks for, the identity
// to sign in as, the keys and funding limit, then Approve or Deny; progress, the answer, and the
// way back to the app. The flow is dashpay/connect.ts. The app's name is its own claim, so it is
// never styled as trusted.

import { useEffect, useId, useState, type ReactNode } from "react";
import type { ConnectBinding } from "../../api/generated/ConnectBinding";
import type { ConnectGroup } from "../../api/generated/ConnectGroup";
import { hasCode } from "../../api/backend";
import { useBackend } from "../../api/queries";
import { DashButton } from "../../components/basics";
import { CopyIconButton } from "../../components/copy";
import { Sheet } from "../../components/sheet";
import { bindingText, ConnectFlow, connectErrorText, criticalText, expiryDays, limitText, quotedLimit } from "../../dashpay/connect";
import { creditCount } from "../../dashpay/profile";
import { useT } from "../../i18n";
import { useModel } from "../../onboarding/model";
import { GrantSheet } from "../tools/common";

function AppHeader({ flow }: { flow: ConnectFlow }) {
  const t = useT();
  const { sheet, returnsTo } = flow;
  return (
    <div className="connect-app" data-testid="connect-app">
      {sheet && (
        <>
          <p className="connect-claim" data-testid="connect-claim">
            {t("dashpay.connect.app.claim", { label: sheet.label })}
          </p>
          {flow.phase === "sheet" && (
            <p className="sheet-text" data-testid="connect-unverified">
              {t("dashpay.connect.app.unverified")}
            </p>
          )}
          <div className="connect-app-id">
            <span className="detail-label">{t("dashpay.connect.app.id")}</span>
            <span className="connect-app-id-value">
              <bdi dir="ltr" className="mono" data-testid="connect-app-id">
                {sheet.appId}
              </bdi>
              <CopyIconButton text={sheet.appId} label={t("dashpay.connect.app.copyId")} size={16} />
            </span>
          </div>
        </>
      )}
      {returnsTo !== null && (
        <p className="field-hint" data-testid="connect-returns-to">
          {t("dashpay.connect.returnsTo", { host: returnsTo })}
        </p>
      )}
    </div>
  );
}

export function GroupDetails({ group }: { group: ConnectGroup }) {
  const t = useT();
  const rows: [string, string, string[]][] = [
    ["owner", t("dashpay.connect.group.owner"), [group.owner]],
    ["admins", t("dashpay.connect.group.admins"), group.admins],
    ["contracts", t("dashpay.connect.group.contracts"), group.contracts],
    ["documentTypes", t("dashpay.connect.group.documentTypes"), group.documentTypes.map((d) => `${d.documentType} · ${d.contract}`)],
    ["tokens", t("dashpay.connect.group.tokens"), group.tokens.map((k) => `${k.contract} · ${k.position}`)],
  ];
  return (
    <dl className="connect-group" data-testid="connect-group">
      {rows.map(([key, label, ids]) => (
        <div key={key} className="connect-group-row" data-testid={`connect-group-${key}`}>
          <dt className="field-hint">{label}</dt>
          {ids.length === 0 ? (
            <dd data-testid="connect-group-none">{t("dashpay.connect.group.none")}</dd>
          ) : (
            ids.map((id) => (
              <dd key={id} className="mono">
                <bdi dir="ltr">{id}</bdi>
              </dd>
            ))
          )}
        </div>
      ))}
    </dl>
  );
}

function BindingRow({ flow, binding }: { flow: ConnectFlow; binding: ConnectBinding }) {
  const { scope } = binding;
  const critical = criticalText(binding);
  const contract = scope.kind === "contract" || scope.kind === "documentType" ? scope.contract : null;
  return (
    <li className="connect-binding" data-testid="connect-binding" data-index={binding.index} data-primary={binding.primary || undefined}>
      <label className="check-row">
        <input
          type="checkbox"
          data-testid="connect-binding-check"
          checked={flow.selected(binding)}
          disabled={binding.primary || !flow.editable}
          onChange={() => flow.toggle(binding)}
        />
        <span>{bindingText(binding)}</span>
      </label>
      {contract !== null && (
        <span className="connect-binding-detail mono">
          <bdi dir="ltr">{contract}</bdi>
        </span>
      )}
      {scope.kind === "group" && <GroupDetails group={scope.group} />}
      {critical !== null && (
        <p className="sheet-warning connect-critical" role="note" data-testid="connect-critical">
          {critical}
        </p>
      )}
    </li>
  );
}

function SheetStep({ flow, network }: { flow: ConnectFlow; network: string }) {
  const t = useT();
  const id = useId();
  const sheet = flow.sheet!;
  const limitProblem = flow.limitValid() ? null : t("dashpay.connect.limitInvalid", { max: limitText(sheet.maxFundingLimit) });
  return (
    <form
      className="connect-form"
      onSubmit={(e) => {
        e.preventDefault();
        void flow.approve();
      }}
    >
      {flow.planChanged && (
        <p className="form-problem" role="status" data-testid="connect-plan-changed">
          {t("dashpay.connect.planChanged")}
        </p>
      )}
      <label className="form-field">
        {t("dashpay.connect.signInAs")}
        <select data-testid="connect-identity" value={flow.identity ?? ""} disabled={!flow.editable} onChange={(e) => flow.setIdentity(e.target.value)}>
          {sheet.identities.map((i) => (
            <option key={i.identity} value={i.identity}>
              {i.username}
            </option>
          ))}
        </select>
      </label>
      <div className="connect-asks">
        <p className="connect-asks-title" id={`${id}-asks`}>
          {t("dashpay.connect.asks")}
        </p>
        <ul className="connect-bindings" aria-labelledby={`${id}-asks`} data-testid="connect-bindings">
          {sheet.bindings.map((b) => (
            <BindingRow key={b.index} flow={flow} binding={b} />
          ))}
          {sheet.encryption && (
            <li className="connect-binding" data-testid="connect-encryption">
              <span className="check-row">
                <input type="checkbox" checked disabled aria-label={t("dashpay.connect.grant.encryption")} />
                <span aria-hidden="true">{t("dashpay.connect.grant.encryption")}</span>
              </span>
            </li>
          )}
        </ul>
      </div>
      <p className="sheet-text" data-testid="connect-cannot">
        {t("dashpay.connect.cannot")}
      </p>
      <label className="form-field">
        {t("dashpay.connect.limit")}
        <input
          data-testid="connect-limit"
          inputMode="decimal"
          spellCheck={false}
          autoComplete="off"
          value={flow.limit}
          disabled={!flow.editable}
          aria-invalid={limitProblem !== null}
          aria-describedby={`${id}-limit-hint ${id}-limit-problem`}
          onChange={(e) => flow.setLimit(e.target.value)}
        />
      </label>
      <p className="field-hint" id={`${id}-limit-hint`}>
        {t("dashpay.connect.limitHint")}
      </p>
      <p className="form-problem" id={`${id}-limit-problem`} role="alert" data-testid="connect-limit-problem">
        {limitProblem}
      </p>
      <dl className="detail-list connect-terms">
        {flow.quote && (
          <div className="detail-row">
            <dt className="detail-label">{t("dashpay.connect.limit")}</dt>
            <dd className="detail-value mono" data-testid="connect-limit-confirm">
              {quotedLimit(flow.quote)}
            </dd>
          </div>
        )}
        <div className="detail-row">
          <dt className="detail-label">{t("dashpay.connect.expiry")}</dt>
          <dd className="detail-value" data-testid="connect-expiry">
            {t("dashpay.connect.expiryValue", { days: expiryDays({ blockTimeMs: sheet.blockTimeMs, terms: (flow.attempted ?? sheet).terms }) })}
          </dd>
        </div>
      </dl>
      {flow.quoteError !== null ? (
        <p className="form-problem" role="alert" data-testid="connect-quote-error">
          {connectErrorText(flow.quoteError, network)}
        </p>
      ) : (
        <p className="sheet-text" role="status" data-testid="connect-fee">
          {flow.quote ? t("dashpay.connect.fee", { amount: creditCount(flow.quote.fee) }) : flow.quoting && <span className="spinner" aria-hidden="true" />}
        </p>
      )}
      <div className="sheet-actions">
        <DashButton variant="tintedGray" size="large" type="button" data-testid="connect-deny" disabled={flow.busy} onClick={() => void flow.deny()}>
          {t("dashpay.connect.deny")}
        </DashButton>
        <DashButton variant="filledBlue" size="large" type="submit" data-testid="connect-approve" disabled={flow.busy || flow.quoting || flow.quote === null || limitProblem !== null}>
          {t("dashpay.connect.approve")}
        </DashButton>
      </div>
    </form>
  );
}

const STEPS = ["keys", "confirm", "send"] as const;

/** The approval's steps, display only: it is one call, so the first shows while it runs. */
function Steps({ at }: { at: (typeof STEPS)[number] }) {
  const t = useT();
  const step = { keys: t("dashpay.connect.step.keys"), confirm: t("dashpay.connect.step.confirm"), send: t("dashpay.connect.step.send") };
  return (
    <ol className="connect-steps" role="status" data-testid="connect-steps" data-step={at}>
      {STEPS.map((s) => (
        <li key={s} className="connect-step" data-current={s === at || undefined}>
          {s === at && <span className="spinner" aria-hidden="true" />}
          <span>{step[s]}</span>
        </li>
      ))}
    </ol>
  );
}

/** Return to the app (when the request came with a return link, and it has an answer to carry) and Close. */
function EndActions({ flow, primary, noReturn }: { flow: ConnectFlow; primary?: ReactNode; noReturn?: boolean }) {
  const t = useT();
  return (
    <div className="sheet-actions">
      <DashButton variant="tintedGray" size="large" data-testid="connect-close" disabled={flow.busy} onClick={() => flow.close()}>
        {t("dashpay.connect.close")}
      </DashButton>
      {primary}
      {flow.returnsTo !== null && !noReturn && (
        <DashButton variant="filledBlue" size="large" data-testid="connect-return" disabled={flow.busy} onClick={() => void flow.returnToApp()}>
          {t("dashpay.connect.return", { host: flow.returnsTo })}
        </DashButton>
      )}
    </div>
  );
}

function Body({ flow, network }: { flow: ConnectFlow; network: string }) {
  const t = useT();
  const returnFailed = flow.returnFailed && flow.returnsTo !== null && (
    <p className="form-problem" role="alert" data-testid="connect-return-failed">
      {t("dashpay.connect.err.returnFailed", { host: flow.returnsTo })}
    </p>
  );
  switch (flow.phase) {
    case "loading":
      return (
        <div className="connect-loading" role="status" data-testid="connect-loading">
          <span className="spinner" aria-hidden="true" />
          <span>{t("dashpay.connect.loading")}</span>
        </div>
      );
    case "mainnetOff":
      return (
        <>
          <p className="sheet-text" role="alert" data-testid="connect-mainnet-off">
            {t("dashpay.connect.err.mainnetOff")}
          </p>
          {returnFailed}
          <EndActions flow={flow} />
        </>
      );
    case "refused":
      return (
        <>
          <p className="form-problem" role="alert" data-testid="connect-refusal">
            {connectErrorText(flow.error, network)}
          </p>
          {returnFailed}
          <EndActions flow={flow} noReturn={hasCode(flow.error, "connect.answered")} />
        </>
      );
    case "sheet":
      return <SheetStep flow={flow} network={network} />;
    case "approving":
      return <Steps at="keys" />;
    case "sending":
      return <Steps at="send" />;
    case "sendFailed":
      return (
        <>
          <p className="form-problem" role="alert" data-testid="connect-send-failed" data-again={flow.retryFailed || undefined}>
            {t("dashpay.connect.sendFailed")}
          </p>
          <EndActions
            flow={flow}
            noReturn
            primary={
              <DashButton variant="filledBlue" size="large" data-testid="connect-retry" onClick={() => void flow.retrySend()}>
                {t("dashpay.connect.retrySend")}
              </DashButton>
            }
          />
        </>
      );
    case "done":
      return (
        <>
          <p className="topup-title" role="status" data-testid="connect-done">
            {t("dashpay.connect.done", { label: flow.sheet?.label ?? "" })}
          </p>
          {returnFailed}
          <EndActions flow={flow} />
        </>
      );
    case "denied":
      return (
        <>
          <p className="sheet-text" role="status" data-testid="connect-denied">
            {t(flow.attempted ? "dashpay.connect.deniedAfterApprove" : "dashpay.connect.denied")}
          </p>
          {returnFailed}
          <EndActions flow={flow} />
        </>
      );
  }
}

/** The sheet of the request `linkId`, for the wallet `walletId`; `onClose` once it closed. */
export function AppConnectSheet({ walletId, linkId, returnsTo, network, onClose }: { walletId: string; linkId: string; returnsTo: string | null; network: string; onClose: () => void }) {
  const t = useT();
  const backend = useBackend();
  const [flow] = useState(() => new ConnectFlow(backend, walletId, linkId, returnsTo, onClose));
  useModel(flow);
  useEffect(() => {
    void flow.load();
  }, [flow]);
  return (
    <>
      <Sheet title={t("dashpay.connect.title")} width="medium" testId="connect-sheet" closable={flow.closable} onClose={() => flow.close()}>
        <div className="connect-body" data-testid="connect-body" data-phase={flow.phase}>
          {flow.phase !== "mainnetOff" && <AppHeader flow={flow} />}
          <Body flow={flow} network={network} />
        </div>
      </Sheet>
      <GrantSheet grants={flow.grants} />
    </>
  );
}
