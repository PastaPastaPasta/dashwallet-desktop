// The username registration wizard (DASHPAY F2, F4; DP1-09), on the Join page: step 1 the name
// with its live checks, step 2 the cost and Confirm & pay, then the registration's progress or
// result for its row (the design's R1–R11). The view model is dashpay/registration.ts, the flow
// dashpay/wizard.ts; the passphrase prompt is U-05's GrantSheet.

import { useId, useState } from "react";
import type { RegistrationQuote } from "../../api/generated/RegistrationQuote";
import { DashButton } from "../../components/basics";
import { Sheet } from "../../components/sheet";
import { useLabelQuote, useNameChecks, useNameCredits, useUsernameCheck } from "../../dashpay/queries";
import {
  availabilityLine,
  canChoose,
  confirmModel,
  MAX_LABEL,
  REG_COPY,
  RULE_TEXT,
  startErrorText,
  type ScreenActionKind,
  type ScreenModel,
} from "../../dashpay/registration";
import type { StatusInputs } from "../../dashpay/status";
import type { RegistrationWizard } from "../../dashpay/wizard";
import { useModel } from "../../onboarding/model";
import { GrantSheet } from "../tools/common";
import { VotingInfoSheet } from "./AboutDashPay";

function StepHeader({ n, step, title, id }: { n: 1 | 2; step: string; title: string; id: string }) {
  return (
    <header className="registration-header">
      <p className="registration-step" data-testid="registration-step">
        {REG_COPY.header(n, step)}
      </p>
      <h2 className="registration-title" id={id}>
        {title}
      </h2>
    </header>
  );
}

function ErrorLine({ wiz, inputs }: { wiz: RegistrationWizard; inputs: StatusInputs }) {
  if (wiz.error === null) return null;
  return (
    <p className="registration-error" role="alert" data-testid="registration-error">
      {startErrorText(wiz.error, inputs.amount, inputs.discreet)}
    </p>
  );
}

/** Step 1 (IOS-066, F4): the checklist as the user types, then the network's answer. */
function NameStep({ wiz, walletId }: { wiz: RegistrationWizard; walletId: string }) {
  const id = useId();
  const [votingInfo, setVotingInfo] = useState(false);
  // The engine checks the label as given; the host trims it.
  const label = wiz.label.trim();
  const { check, availability, pending } = useNameChecks(walletId, label);
  const line = availabilityLine(check, availability, pending);
  // An instant username (DP1-10 D13) is never one the network votes on.
  const instant = wiz.mode.kind === "credits" && wiz.mode.instant === true;
  const refused = instant && check?.contested === true;
  const ok = canChoose(check, availability, pending) && !refused;
  const passed = (rule: keyof typeof RULE_TEXT) => check?.rules.find((r) => r.rule === rule)?.passed;
  return (
    <section className="registration card-l1" data-testid="registration-name" aria-labelledby={`${id}-title`}>
      <StepHeader n={1} step={REG_COPY.name.step} title={instant ? REG_COPY.name.instantTitle : REG_COPY.name.title} id={`${id}-title`} />
      <form
        className="registration-form"
        onSubmit={(e) => {
          e.preventDefault();
          if (ok) wiz.next();
        }}
      >
        <label className="form-field">
          {REG_COPY.name.field}
          <input
            data-testid="registration-name-field"
            value={wiz.label}
            maxLength={MAX_LABEL}
            autoFocus
            autoComplete="off"
            autoCapitalize="off"
            spellCheck={false}
            aria-invalid={label !== "" && check?.valid === false}
            aria-describedby={`${id}-rules`}
            onChange={(e) => wiz.setLabel(e.target.value)}
          />
        </label>
        <ul className="registration-rules" id={`${id}-rules`} data-testid="registration-rules">
          {(Object.keys(RULE_TEXT) as (keyof typeof RULE_TEXT)[]).map((rule) => (
            <li key={rule} data-passed={label === "" ? undefined : String(passed(rule) ?? false)}>
              {RULE_TEXT[rule]}
            </li>
          ))}
        </ul>
        <p className="registration-availability" role="status" data-testid="registration-availability" data-tone={line?.tone}>
          {line?.text}
        </p>
        {refused && (
          <p className="registration-line registration-problem" role="alert" data-testid="registration-instant-only">
            {REG_COPY.name.instantOnly}
          </p>
        )}
        {check?.contested && !refused && (
          <div className="registration-contested" data-testid="registration-contested">
            <p className="registration-line">{REG_COPY.name.contested}</p>
            <DashButton variant="plainBlue" size="small" data-testid="registration-voting-info" onClick={() => setVotingInfo(true)}>
              {REG_COPY.name.votingLink}
            </DashButton>
          </div>
        )}
        <div className="registration-actions">
          <DashButton variant="tintedGray" data-testid="registration-cancel" onClick={() => wiz.close()}>
            {REG_COPY.name.cancel}
          </DashButton>
          <DashButton variant="filledBlue" type="submit" data-testid="registration-next" disabled={!ok}>
            {REG_COPY.name.next}
          </DashButton>
        </div>
      </form>
      {votingInfo && <VotingInfoSheet onClose={() => setVotingInfo(false)} />}
    </section>
  );
}

/** The wedge (m4 §2.2, DEC-197): a start refused behind a registration whose funding is unknown.
 * A refusal of the start past it shows here, over the step. */
function WedgeSheet({ wiz, quote, inputs }: { wiz: RegistrationWizard; quote: RegistrationQuote; inputs: StatusInputs }) {
  return (
    <Sheet title={REG_COPY.wedge.title} width="small" testId="registration-wedge" onClose={() => wiz.dismissWedge()}>
      <p className="sheet-text">{REG_COPY.wedge.text}</p>
      <ErrorLine wiz={wiz} inputs={inputs} />
      <div className="sheet-actions">
        <DashButton variant="tintedGray" size="large" data-testid="registration-wedge-cancel" onClick={() => wiz.dismissWedge()}>
          {REG_COPY.wedge.cancel}
        </DashButton>
        <DashButton
          variant="filledBlue"
          size="large"
          data-testid="registration-wedge-ok"
          disabled={wiz.running !== null}
          onClick={() => void wiz.confirmWedge(quote.grant)}
        >
          {REG_COPY.wedge.confirm}
        </DashButton>
      </div>
    </Sheet>
  );
}

/** Step 2's cost: the quote in Dash (D2), or what the identity's credits may pay (D6). */
function CostLines({ wiz, walletId, identity, inputs }: { wiz: RegistrationWizard; walletId: string; identity: string | null; inputs: StatusInputs }) {
  const quote = useLabelQuote(walletId, wiz.label, identity === null);
  const credits = useNameCredits(walletId, identity, wiz.label);
  // Either read failing says so (review r2 L1).
  if ((identity !== null ? credits : quote).isError) return <p className="registration-line registration-problem">{REG_COPY.confirm.quoteFailed}</p>;
  if (identity !== null)
    return <p className="registration-line">{credits.data ? REG_COPY.confirm.fromCredits(credits.data.maxCredits) : REG_COPY.confirm.quoting}</p>;
  if (!quote.data) return <p className="registration-line">{REG_COPY.confirm.quoting}</p>;
  const model = confirmModel(quote.data, inputs.amount);
  return (
    <>
      <p className="registration-fee">
        <span>{REG_COPY.confirm.fee}</span>
        <span className="registration-fee-amount" data-testid="registration-fee">
          {model.cost}
        </span>
      </p>
      {model.lines.map((line) => (
        <p key={line} className="registration-line">
          {line}
        </p>
      ))}
      {model.shortfall && (
        <p className="registration-line registration-problem" data-testid="registration-shortfall">
          {model.shortfall}
        </p>
      )}
    </>
  );
}

/** Step 2 (IOS-067): the cost in Dash (D2) from the wallet's coins, or the identity's credits (D6). */
function ConfirmStep({ wiz, walletId, inputs }: { wiz: RegistrationWizard; walletId: string; inputs: StatusInputs }) {
  const id = useId();
  const [accepted, setAccepted] = useState(false);
  const identity = wiz.mode.kind === "credits" ? wiz.mode.identity : null;
  // The same reads as CostLines (shared through the query cache).
  const quote = useLabelQuote(walletId, wiz.label, identity === null);
  const credits = useNameCredits(walletId, identity, wiz.label);
  const check = useUsernameCheck(wiz.label);
  const contested = identity !== null ? (check.data?.contested ?? false) : (quote.data?.contested ?? false);
  const ready = identity !== null ? credits.data !== undefined : quote.data !== undefined && confirmModel(quote.data, inputs.amount).shortfall === null;
  const disabled = !ready || wiz.running !== null || (contested && !accepted);
  const pay = () => {
    if (identity !== null) void wiz.registerName(identity);
    else if (quote.data) void wiz.confirm(quote.data.grant);
  };
  return (
    <section className="registration card-l1" data-testid="registration-confirm" aria-labelledby={`${id}-title`}>
      <StepHeader n={2} step={REG_COPY.confirm.step} title={REG_COPY.confirm.title} id={`${id}-title`} />
      <p className="registration-line">{REG_COPY.confirm.message(wiz.label)}</p>
      <div className="registration-cost" data-testid="registration-cost">
        <CostLines wiz={wiz} walletId={walletId} identity={identity} inputs={inputs} />
      </div>
      {contested && (
        <label className="registration-accept">
          <input type="checkbox" data-testid="registration-accept" checked={accepted} onChange={(e) => setAccepted(e.target.checked)} />
          {REG_COPY.confirm.accept}
        </label>
      )}
      <ErrorLine wiz={wiz} inputs={inputs} />
      <div className="registration-actions">
        <DashButton variant="tintedGray" data-testid="registration-back" disabled={wiz.running !== null} onClick={() => wiz.back()}>
          {REG_COPY.confirm.back}
        </DashButton>
        <DashButton
          variant="filledBlue"
          data-testid="registration-pay"
          disabled={disabled}
          onClick={pay}
        >
          {wiz.running !== null && <span className="spinner" aria-hidden="true" />}
          {identity !== null ? REG_COPY.confirm.register : REG_COPY.confirm.pay}
        </DashButton>
      </div>
      {wiz.wedge !== null && quote.data && <WedgeSheet wiz={wiz} quote={quote.data} inputs={inputs} />}
    </section>
  );
}

/** Steps 1 and 2, and the passphrase prompt their grants ask for. */
export function WizardSteps({ wiz, walletId, inputs }: { wiz: RegistrationWizard; walletId: string; inputs: StatusInputs }) {
  useModel(wiz);
  return (
    <>
      {wiz.step === "name" ? <NameStep wiz={wiz} walletId={walletId} /> : <ConfirmStep wiz={wiz} walletId={walletId} inputs={inputs} />}
      <GrantSheet grants={wiz.grants} />
    </>
  );
}

/** A registration's progress or result (R1–R11), or a name's from credits (D6). `data-kind` is
 * the screen, `data-phase` and `data-waiting` the row's. */
export function RegistrationScreen({
  wiz,
  model,
  phase,
  waiting,
  onAction,
  inputs,
}: {
  wiz: RegistrationWizard;
  model: ScreenModel;
  phase?: string;
  waiting?: string | null;
  onAction: (kind: ScreenActionKind) => void;
  inputs: StatusInputs;
}) {
  useModel(wiz);
  const id = useId();
  const [votingInfo, setVotingInfo] = useState(false);
  const { step } = model;
  return (
    <section
      className="registration card-l1"
      data-testid="registration-screen"
      data-kind={model.kind}
      data-phase={phase}
      data-waiting={waiting ?? undefined}
      aria-labelledby={`${id}-title`}
    >
      {/* Changes as the run moves on, announced politely, as the Home card (DP1-08 D25). */}
      <div role="status" className="registration-live">
        <h2 className="registration-title" id={`${id}-title`}>
          {model.title}
          {model.busy && <span className="spinner registration-spinner" aria-hidden="true" />}
        </h2>
        {step && (
          <div className="dashpay-card-step" data-testid="registration-progress-step">
            <span>
              ({step.n}/3) {step.text}
            </span>
            <progress className="dashpay-card-progress" max={3} value={step.n} aria-label="Registration progress" />
          </div>
        )}
        <div data-testid="registration-lines">
          {model.lines.map((line) => (
            <p key={line} className="registration-line">
              {line}
            </p>
          ))}
        </div>
      </div>
      {wiz.finished && (
        <p className="registration-line" role="status" data-testid="registration-finished">
          {REG_COPY.finished(wiz.finished)}
        </p>
      )}
      <ErrorLine wiz={wiz} inputs={inputs} />
      <div className="registration-actions">
        {model.actions.map((a, i) => (
          <DashButton
            key={a.kind}
            variant={i === 0 ? "filledBlue" : "plainBlue"}
            data-testid={`registration-${a.kind}`}
            disabled={wiz.running !== null}
            onClick={() => (a.kind === "votingInfo" ? setVotingInfo(true) : onAction(a.kind))}
          >
            {wiz.running !== null && i === 0 && <span className="spinner" aria-hidden="true" />}
            {a.label}
          </DashButton>
        ))}
      </div>
      <GrantSheet grants={wiz.grants} />
      {votingInfo && <VotingInfoSheet onClose={() => setVotingInfo(false)} />}
    </section>
  );
}
