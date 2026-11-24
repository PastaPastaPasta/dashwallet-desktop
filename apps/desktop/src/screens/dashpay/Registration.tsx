// The username registration wizard (DASHPAY F2, F4; DP1-09), on the Join page: step 1 the name
// with its live checks, step 2 the cost and Confirm & pay, then the registration's progress or
// result for its row (the design's R1–R11). The view model is dashpay/registration.ts, the flow
// dashpay/wizard.ts; the passphrase prompt is U-05's GrantSheet.

import { useEffect, useId, useRef, useState } from "react";
import { flushSync } from "react-dom";
import { hasCode } from "../../api/backend";
import type { RegistrationQuote } from "../../api/generated/RegistrationQuote";
import { myQr } from "../../contacts/add";
import { useMyUserLink } from "../../contacts/queries";
import { CLAIM_COPY, claimErrorText } from "../../dashpay/claim";
import { DashButton } from "../../components/basics";
import { Sheet } from "../../components/sheet";
import { useBackend, usePoolBalances } from "../../api/queries";
import { PAY_WITH_COPY, payWithRows } from "../../dashpay/payWith";
import { timeText } from "../../fiat/rates";
import { known } from "../../format/homeBalance";
import { useInstantChecks, useLabelQuote, useNameChecks, useNameCredits, usePlatformSource, useShieldedReadiness, useUsernameCheck, useUsernameQuote } from "../../dashpay/queries";
import { instantCheck, suggest } from "../../dashpay/instantName";
import { INSTANT_COPY, voteDuration } from "../../dashpay/instantOffer";
import type { NetworkKind } from "../../api/generated/NetworkKind";
import {
  availabilityLine,
  canChoose,
  confirmModel,
  MAX_LABEL,
  platformRefusalText,
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
import { useT } from "../../i18n";
import { VotingInfoSheet } from "./AboutDashPay";
import { ShareSheet } from "./ShareSheet";
import { WelcomeSteps } from "./Welcome";

export function StepHeader({ n, step, title, id }: { n: 1 | 2; step: string; title: string; id: string }) {
  useT();
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

/** The last call's refusal; a claim's (`claim`, DP5-03 C12) in its own words, and one paid from
 * the Platform balance (`fromPlatform`: the wizard's source, or the row's) in that balance's. */
export function ErrorLine({ wiz, inputs, claim = false, fromPlatform = wiz.fromPlatform }: { wiz: RegistrationWizard; inputs: StatusInputs; claim?: boolean; fromPlatform?: boolean }) {
  useT();
  if (wiz.error === null) return null;
  return (
    <p className="registration-error" role="alert" data-testid="registration-error">
      {claim || wiz.mode.kind === "invitation" ? claimErrorText(wiz.error, inputs.amount) : startErrorText(wiz.error, inputs.amount, inputs.discreet, fromPlatform)}
    </p>
  );
}

/** The instant username's offer (D4 reversed; iOS `CreateInstantUsernameSheet`), over step 1 once
 * a contested name is chosen: Create instant username is the default, focused, so Enter takes it. */
export function InstantOfferSheet({ wiz, network }: { wiz: RegistrationWizard; network: NetworkKind }) {
  useT();
  const create = () => wiz.createInstant(suggest(wiz.label));
  return (
    <Sheet title={INSTANT_COPY.title} width="small" testId="instant-offer" onClose={() => wiz.dismissOffer()}>
      <form
        onSubmit={(e) => {
          e.preventDefault();
          create();
        }}
      >
        {INSTANT_COPY.offer(voteDuration(network)).map((text) => (
          <p key={text} className="sheet-text">
            {text}
          </p>
        ))}
        <div className="sheet-actions">
          <DashButton variant="tintedGray" size="large" data-testid="instant-offer-decline" onClick={() => wiz.declineInstant()}>
            {INSTANT_COPY.decline}
          </DashButton>
          <DashButton variant="filledBlue" size="large" type="submit" data-autofocus data-testid="instant-offer-create">
            {INSTANT_COPY.create}
          </DashButton>
        </div>
      </form>
    </Sheet>
  );
}

/** The instant username's form (D4 reversed): iOS's explainer, the suggestion, and one line for
 * the checks in iOS's order (`instantCheck`). */
function InstantStep({ wiz, walletId }: { wiz: RegistrationWizard; walletId: string }) {
  useT();
  const id = useId();
  const candidate = (wiz.temporaryLabel ?? "").trim();
  const { check, availability, pending } = useInstantChecks(walletId, wiz.label, candidate);
  const c = instantCheck(wiz.label, candidate, check, availability, pending);
  return (
    <section className="registration card-l1" data-testid="registration-instant" aria-labelledby={`${id}-title`}>
      <StepHeader n={1} step={REG_COPY.name.step} title={REG_COPY.name.instantTitle} id={`${id}-title`} />
      <p className="registration-line" data-testid="registration-instant-explainer">
        {INSTANT_COPY.explainer(wiz.label)}
      </p>
      <form
        className="registration-form"
        onSubmit={(e) => {
          e.preventDefault();
          if (c.ok) wiz.confirmInstant();
        }}
      >
        <label className="form-field">
          {REG_COPY.name.field}
          <input
            data-testid="registration-instant-field"
            value={wiz.temporaryLabel ?? ""}
            maxLength={MAX_LABEL}
            autoFocus
            autoComplete="off"
            autoCapitalize="off"
            spellCheck={false}
            aria-invalid={c.line?.tone === "problem"}
            aria-describedby={`${id}-check`}
            onChange={(e) => wiz.setTemporaryLabel(e.target.value)}
          />
        </label>
        <p className="registration-availability" id={`${id}-check`} role="status" data-testid="registration-instant-check" data-kind={c.kind} data-tone={c.line?.tone}>
          {c.line?.text}
        </p>
        <div className="registration-actions">
          <DashButton variant="tintedGray" data-testid="registration-back" onClick={() => wiz.back()}>
            {REG_COPY.confirm.back}
          </DashButton>
          <DashButton variant="filledBlue" type="submit" data-testid="registration-next" disabled={!c.ok}>
            {REG_COPY.name.next}
          </DashButton>
        </div>
      </form>
    </section>
  );
}

/** Step 1 (IOS-066, F4): the checklist as the user types, then the network's answer. */
function NameStep({ wiz, walletId, network }: { wiz: RegistrationWizard; walletId: string; network: NetworkKind }) {
  useT();
  const id = useId();
  const [votingInfo, setVotingInfo] = useState(false);
  // The engine checks the label as given; the host trims it.
  const label = wiz.label.trim();
  const { check, availability, pending } = useNameChecks(walletId, label);
  const line = availabilityLine(check, availability, pending);
  // An instant username (DP1-10 D13) is never one the network votes on, nor is a name an
  // invitation cannot pay a vote for (DP5-03 D15).
  const instant = wiz.mode.kind === "credits" && wiz.mode.instant === true;
  const limited = wiz.mode.kind === "invitation" && !wiz.mode.contestedAllowed;
  const refused = (instant || limited) && check?.contested === true;
  const ok = canChoose(check, availability, pending) && !refused;
  const passed = (rule: keyof typeof RULE_TEXT) => check?.rules.find((r) => r.rule === rule)?.passed;
  return (
    <section className="registration card-l1" data-testid="registration-name" aria-labelledby={`${id}-title`}>
      <StepHeader n={1} step={REG_COPY.name.step} title={instant ? REG_COPY.name.instantTitle : REG_COPY.name.title} id={`${id}-title`} />
      <form
        className="registration-form"
        onSubmit={(e) => {
          e.preventDefault();
          if (ok) wiz.next(check?.contested === true);
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
            {limited ? CLAIM_COPY.name.contestedRefused : REG_COPY.name.instantOnly}
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
          {wiz.picked ? (
            <DashButton variant="tintedGray" data-testid="registration-back" onClick={() => wiz.back()}>
              {REG_COPY.confirm.back}
            </DashButton>
          ) : (
            <DashButton variant="tintedGray" data-testid="registration-cancel" onClick={() => wiz.close()}>
              {REG_COPY.name.cancel}
            </DashButton>
          )}
          <DashButton variant="filledBlue" type="submit" data-testid="registration-next" disabled={!ok}>
            {REG_COPY.name.next}
          </DashButton>
        </div>
      </form>
      {votingInfo && <VotingInfoSheet onClose={() => setVotingInfo(false)} />}
      {wiz.offer && <InstantOfferSheet wiz={wiz} network={network} />}
    </section>
  );
}

const lost = () => document.activeElement === null || document.activeElement === document.body;

/** While the wizard runs a call its button is disabled, and once the row shows its progress the
 * button is gone, so focus falls to <body>; a passphrase sheet closing onto a disabled button
 * cannot give it back either. A frame after a render (after the sheet gave focus back itself if it
 * could), focus lost goes to `target` if it was lost on mount, when a call or a passphrase sheet
 * ended, while a call runs, or because this screen's control that had it went away or was
 * disabled. When the passphrase was cancelled meanwhile and nothing runs, focus parked there goes
 * back to that control if it is enabled again. A sheet over it does not count; focus the user moved
 * elsewhere, a click on text included, stays. */
function useFocusFallback(wiz: RegistrationWizard, target: () => HTMLElement | null | undefined, off = false) {
  useModel(wiz);
  const { pending } = useModel(wiz.grants);
  const busy = wiz.running !== null;
  // This screen's control that had focus last, while it may still lose it to the screen.
  const last = useRef<HTMLButtonElement | null>(null);
  // Not parked, or the control focus was lost from and the grants' cancel count then.
  const parked = useRef<{ from: HTMLButtonElement | null; epoch: number } | undefined>(undefined);
  const seen = useRef<{ busy: boolean; pending: unknown } | null>(null);
  const due = useRef(false);
  useEffect(() => {
    const foreign = (at: HTMLElement, t: HTMLElement | null | undefined) => {
      const sheet = at.closest('[role="dialog"]');
      return sheet !== null && !sheet.contains(t ?? null);
    };
    const into = (e: FocusEvent) => {
      const at = e.target;
      const t = target();
      if (!(at instanceof HTMLElement) || at === t || foreign(at, t)) return;
      parked.current = undefined;
      const scope = t?.closest('[role="dialog"]') ?? t?.closest("section");
      last.current = at instanceof HTMLButtonElement && scope?.contains(at) ? at : null;
    };
    // Judged after the event: Chromium blurs a control as it is removed, before it leaves the page.
    const away = (e: FocusEvent) => {
      const at = last.current;
      if (at === null || e.target !== at) return;
      queueMicrotask(() => {
        if (last.current === at && at.isConnected && !at.matches(":disabled")) last.current = null;
      });
    };
    document.addEventListener("focusin", into);
    document.addEventListener("focusout", away);
    return () => {
      document.removeEventListener("focusin", into);
      document.removeEventListener("focusout", away);
    };
  }, []);
  useEffect(() => {
    if (seen.current === null || seen.current.busy !== busy || seen.current.pending !== pending) due.current = true;
    seen.current = { busy, pending };
    if (off || pending) return;
    const frame = requestAnimationFrame(() => {
      const edge = due.current;
      due.current = false;
      const t = target();
      const p = parked.current;
      const from = p?.from;
      if (!busy && p && p.epoch !== wiz.grants.epoch && from?.isConnected && !from.matches(":disabled") && document.activeElement === t) return from.focus();
      const held = document.activeElement?.matches(":disabled") === true;
      if (!lost() && !held) return;
      const dropped = last.current !== null && (!last.current.isConnected || last.current.matches(":disabled"));
      if (!(edge || busy || held || dropped)) return;
      parked.current = p ?? { from: last.current, epoch: wiz.grants.epoch };
      last.current = null;
      t?.focus();
    });
    return () => cancelAnimationFrame(frame);
  });
}

/** The wedge (m4 §2.2, DEC-197): a start refused behind a registration whose funding is unknown.
 * A refusal of the start past it shows here, over the step. */
function WedgeSheet({ wiz, quote, inputs }: { wiz: RegistrationWizard; quote: RegistrationQuote; inputs: StatusInputs }) {
  useT();
  const text = useRef<HTMLParagraphElement>(null);
  useFocusFallback(wiz, () => text.current?.closest<HTMLElement>('[role="dialog"]'));
  return (
    <Sheet title={REG_COPY.wedge.title} width="small" testId="registration-wedge" onClose={() => wiz.dismissWedge()}>
      <p className="sheet-text" ref={text}>{REG_COPY.wedge.text}</p>
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
          onClick={() => void wiz.confirmWedge(quote)}
        >
          {REG_COPY.wedge.confirm}
        </DashButton>
      </div>
    </Sheet>
  );
}

/** Step 2's cost: the quote in Dash (D2) from the Dash wallet or the Platform balance (C-funding
 * F9), or what the identity's credits may pay (D6). */
function CostLines({ wiz, walletId, identity, inputs }: { wiz: RegistrationWizard; walletId: string; identity: string | null; inputs: StatusInputs }) {
  useT();
  const quote = useLabelQuote(walletId, wiz.label, identity === null, wiz.fromPlatform, wiz.temporaryLabel);
  const credits = useNameCredits(walletId, identity, wiz.label);
  // Either read failing says so (review r2 L1); a Platform quote's refusal says why.
  const failed = identity !== null ? credits : quote;
  if (failed.isError) {
    const why = (wiz.fromPlatform && platformRefusalText(failed.error, inputs.amount)) || REG_COPY.confirm.quoteFailed;
    return (
      <p className="registration-line registration-problem" data-testid="registration-quote-failed">
        {why}
      </p>
    );
  }
  if (identity !== null)
    return <p className="registration-line">{credits.data ? REG_COPY.confirm.fromCredits(credits.data.maxCredits) : REG_COPY.confirm.quoting}</p>;
  if (!quote.data || !wiz.quoteStands(quote.dataUpdatedAt)) return <p className="registration-line">{REG_COPY.confirm.quoting}</p>;
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
  useT();
  const id = useId();
  const [accepted, setAccepted] = useState(false);
  const identity = wiz.mode.kind === "credits" ? wiz.mode.identity : null;
  // The same reads as CostLines (shared through the query cache).
  const quote = useLabelQuote(walletId, wiz.label, identity === null, wiz.fromPlatform, wiz.temporaryLabel);
  // F4: a start refused for a changed cost shows the new one; F22: so does a run's.
  const changedCost = hasCode(wiz.error, "registration.quote_changed");
  const stands = wiz.quoteStands(quote.dataUpdatedAt);
  const { refetch } = quote;
  useEffect(() => {
    if (changedCost || !stands) void refetch();
  }, [changedCost, stands, refetch]);
  const credits = useNameCredits(walletId, identity, wiz.label);
  const check = useUsernameCheck(wiz.label);
  const contested = identity !== null ? (check.data?.contested ?? false) : (quote.data?.contested ?? false);
  // A refetch that fails keeps the last quote: Pay waits for one that stands.
  const ready =
    identity !== null ? credits.data !== undefined : quote.data !== undefined && !quote.isError && stands && confirmModel(quote.data, inputs.amount).shortfall === null;
  const disabled = !ready || wiz.running !== null || (contested && !accepted);
  const pay = () => {
    if (identity !== null) void wiz.registerName(identity);
    else if (quote.data) void wiz.confirm(quote.data);
  };
  return (
    <section className="registration card-l1" data-testid="registration-confirm" aria-labelledby={`${id}-title`}>
      <StepHeader n={2} step={REG_COPY.confirm.step} title={REG_COPY.confirm.title} id={`${id}-title`} />
      <p className="registration-line">{REG_COPY.confirm.message(wiz.label)}</p>
      {wiz.temporaryLabel !== null && (
        <p className="registration-line" data-testid="registration-confirm-instant">
          {INSTANT_COPY.confirm(wiz.temporaryLabel)}
        </p>
      )}
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

/** "Pay with" (C-funding F19): each source with its balance and, when it cannot pay, its one
 * reason; Dash wallet selected each time. A source that cannot pay cannot be chosen. */
function PayWithStep({ wiz, walletId, inputs }: { wiz: RegistrationWizard; walletId: string; inputs: StatusInputs }) {
  useT();
  const id = useId();
  const b = useBackend();
  const core = useUsernameQuote(walletId, true) ?? null;
  const pools = usePoolBalances(walletId);
  const readiness = useShieldedReadiness(walletId);
  const platform = usePlatformSource(walletId);
  const rows = payWithRows({
    core,
    platformDuffs: known(pools.platform) ? pools.platform : null,
    platform,
    shieldedDuffs: known(pools.shielded) ? pools.shielded : null,
    readiness,
    amount: inputs.amount,
    clock: (secs) => timeText(secs, b.locale),
    discreet: inputs.discreet,
  });
  const chosen = rows.find((r) => r.source === wiz.source);
  return (
    <section className="registration card-l1" data-testid="registration-pay-with" aria-labelledby={`${id}-title`}>
      <header className="registration-header">
        <h2 className="registration-title" id={`${id}-title`}>
          {PAY_WITH_COPY.title}
        </h2>
      </header>
      <form
        className="registration-form"
        onSubmit={(e) => {
          e.preventDefault();
          if (chosen?.reason === null) wiz.pickSource();
        }}
      >
        <fieldset className="topup-choices pay-with-sources" aria-labelledby={`${id}-title`}>
          {rows.map((r) => (
            <label key={r.source} className="topup-choice pay-with-source" data-testid={`pay-with-${r.source}`} data-disabled={r.reason !== null || undefined}>
              <input
                type="radio"
                name={`${id}-source`}
                checked={wiz.source === r.source}
                disabled={r.reason !== null}
                aria-describedby={r.reason !== null ? `${id}-${r.source}-reason` : undefined}
                onChange={() => wiz.setSource(r.source)}
              />
              <span className="topup-choice-amount">{r.label}</span>
              {r.balance !== null && (
                <span className="topup-choice-buys" data-testid={`pay-with-${r.source}-balance`}>
                  <bdi dir="ltr">{r.balance}</bdi>
                </span>
              )}
              {r.reason !== null && (
                <span className="topup-choice-buys pay-with-reason" id={`${id}-${r.source}-reason`} data-testid={`pay-with-${r.source}-reason`}>
                  {r.reason}
                </span>
              )}
            </label>
          ))}
        </fieldset>
        <div className="registration-actions">
          <DashButton variant="tintedGray" data-testid="registration-cancel" onClick={() => wiz.close()}>
            {REG_COPY.name.cancel}
          </DashButton>
          <DashButton variant="filledBlue" type="submit" data-testid="registration-next" disabled={chosen?.reason !== null}>
            {REG_COPY.name.next}
          </DashButton>
        </div>
      </form>
    </section>
  );
}

/** Steps 1 and 2 (after "Pay with" when it was opened), and the passphrase prompt their grants
 * ask for. */
export function WizardSteps({ wiz, walletId, inputs, network }: { wiz: RegistrationWizard; walletId: string; inputs: StatusInputs; network: NetworkKind }) {
  useT();
  useModel(wiz);
  return (
    <>
      {wiz.step === "source" && <PayWithStep wiz={wiz} walletId={walletId} inputs={inputs} />}
      {wiz.step === "name" && <NameStep wiz={wiz} walletId={walletId} network={network} />}
      {wiz.step === "instant" && <InstantStep wiz={wiz} walletId={walletId} />}
      {wiz.step === "confirm" && <ConfirmStep wiz={wiz} walletId={walletId} inputs={inputs} />}
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
  claim = false,
  fromPlatform = false,
  onAction,
  inputs,
  share = null,
}: {
  wiz: RegistrationWizard;
  model: ScreenModel;
  phase?: string;
  waiting?: string | null;
  /** The row is an invitation's claim (DP5-03): its refusals in the claim's words. */
  claim?: boolean;
  /** The row is paid from the Platform balance: its refusals in that balance's words. */
  fromPlatform?: boolean;
  onAction: (kind: ScreenActionKind) => void;
  inputs: StatusInputs;
  /** Whose username the welcome's Share my username shows, once known. */
  share?: { walletId: string; identity: string; username: string | null } | null;
}) {
  const tr = useT();
  useModel(wiz);
  const id = useId();
  const [votingInfo, setVotingInfo] = useState(false);
  const [sharing, setSharing] = useState(false);
  // As on My Profile, sharing follows the engine's link for the name shown. An open sheet closes
  // once that link is refused or the name is gone, and waits for another click.
  const link = useMyUserLink(share?.walletId ?? "", share?.identity ?? "", share !== null);
  const linkKind = share ? myQr(link, share.username).kind : null;
  const sheetOpen = linkKind === "link" || linkKind === "loading";
  useEffect(() => {
    if (!sheetOpen) setSharing(false);
  }, [sheetOpen]);
  const { step } = model;
  const welcome = model.kind === "done";
  const section = useRef<HTMLElement>(null);
  const title = useRef<HTMLHeadingElement>(null);
  const [announced, setAnnounced] = useState("");
  const shown = useRef<boolean | null>(null);
  const latest = useRef(model);
  latest.current = model;
  // The progress screen's controls go when it turns into the welcome, and focus with them; start
  // keyboard users at the title then, and on any screen that appears with focus lost. Focus still
  // somewhere else stays, and the welcome is announced instead, as it is when the title already
  // has focus and so does not take it anew.
  useFocusFallback(wiz, () => title.current, welcome);
  useEffect(() => {
    const from = shown.current;
    shown.current = welcome;
    if (!welcome) return setAnnounced("");
    // `kept`: the title may hold focus from before, with no new focus event to read it.
    // `late`: from a frame, where the text is committed at once, before a sheet can cover it again.
    const announce = (kept: boolean, late = false) => {
      const active = document.activeElement;
      if (active !== title.current && (lost() || section.current?.contains(active))) title.current?.focus();
      else if (active !== title.current || kept) {
        const say = () => setAnnounced(REG_COPY.announced(latest.current.title, latest.current.lines));
        if (late) flushSync(say);
        else say();
      }
    };
    // Behind a sheet the screen is inert, so neither reaches a reader: once it is not, a frame
    // later (after the sheet gave focus back).
    const covered = () => section.current?.closest("[inert]") != null;
    if (!covered()) return announce(from === false);
    let frame = 0;
    const watch = new MutationObserver(() => {
      cancelAnimationFrame(frame);
      frame = requestAnimationFrame(() => {
        if (covered()) return;
        watch.disconnect();
        announce(false, true);
      });
    });
    watch.observe(document.body, { attributes: true, attributeFilter: ["inert"], subtree: true });
    return () => {
      watch.disconnect();
      cancelAnimationFrame(frame);
    };
    // Only the switch to the welcome moves focus or speaks, not a later change of its text.
  }, [welcome]);
  return (
    <section
      ref={section}
      className={welcome ? "registration registration-welcome card-l1" : "registration card-l1"}
      data-testid="registration-screen"
      data-kind={model.kind}
      data-phase={phase}
      data-waiting={waiting ?? undefined}
      aria-labelledby={welcome ? undefined : `${id}-title`}
    >
      <p role="status" className="sr-only" data-testid="registration-announced">
        {announced}
      </p>
      {/* Changes as the run moves on, announced politely, as the Home card (DP1-08 D25). The
          welcome is announced by its focused title instead, so it is not read twice. */}
      <div role={welcome ? undefined : "status"} className="registration-live">
        <h2
          className="registration-title"
          id={`${id}-title`}
          ref={title}
          tabIndex={-1}
          aria-describedby={welcome ? `${id}-lines` : undefined}
        >
          {model.title}
          {model.busy && <span className="spinner registration-spinner" aria-hidden="true" />}
        </h2>
        {step && (
          <div className="dashpay-card-step" data-testid="registration-progress-step">
            <span>{tr("dashpay.step.shown", { step: `${step.n}/3`, text: step.text })}</span>
            <progress className="dashpay-card-progress" max={3} value={step.n} aria-label={tr("dashpay.step.progress")} />
          </div>
        )}
        <div data-testid="registration-lines" id={`${id}-lines`}>
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
      <ErrorLine wiz={wiz} inputs={inputs} claim={claim} fromPlatform={fromPlatform} />
      {welcome && (
        <WelcomeSteps
          disabled={wiz.running !== null}
          canEdit={inputs.main !== undefined}
          canShare={linkKind === "link"}
          onStep={(kind) => (kind === "share" ? setSharing(true) : onAction(kind))}
        />
      )}
      <div className="registration-actions">
        {model.actions.map((a, i) => (
          <DashButton
            key={a.kind}
            variant={i === 0 && !welcome ? "filledBlue" : "plainBlue"}
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
      {sharing && share && sheetOpen && (
        <ShareSheet
          walletId={share.walletId}
          identity={share.identity}
          who={share.username ? { name: null, username: share.username } : null}
          testId="welcome-share-sheet"
          returnFocus={() => {
            const button = document.getElementById("welcome-share");
            return button instanceof HTMLButtonElement && !button.disabled ? button : title.current;
          }}
          onClose={() => setSharing(false)}
        />
      )}
    </section>
  );
}
