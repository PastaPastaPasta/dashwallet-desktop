// Transfer (transfer/model.ts): one sheet for the moves between the wallet's own balances. The form
// picks From and To, takes the amount (Max fills it from the route's own quotes) and shows the fee
// line; Continue goes to the route's own confirmation, and from there each route's steps are its own
// sheet's (ShieldedSheet.tsx, PlatformFundSheet.tsx, PlatformMoveSheet.tsx), unchanged.

import { useEffect, useId, useRef } from "react";
import { useQuery } from "@tanstack/react-query";
import type { NetworkInfo } from "../api/generated/NetworkInfo";
import type { ShieldedStatus } from "../api/generated/ShieldedStatus";
import { CREDITS_PER_DUFF, creditsToDuffs } from "../api/pending";
import { keys, shieldedDuffs, useBackend, usePendingPlatformFunds, usePlatformActivity, usePlatformMoveQuote, usePoolBalances, useShieldedStatus, useUtxos } from "../api/queries";
import { navigate, useUi } from "../app/ui";
import { DashButton, Icon, VisuallyHidden } from "../components/basics";
import { Sheet } from "../components/sheet";
import { useDisplay } from "../format/display";
import { useModel } from "../onboarding/model";
import { FUND_COPY, fundErrorText, fundRowView } from "../platform/fund";
import { MOVE_COPY, moveErrorText, plannedWithdrawalCredits, type MoveBlock } from "../platform/move";
import { SHIELDED_COPY, shieldedErrorText } from "../shielded/flow";
import { shieldedSyncing } from "../shielded/syncing";
import {
  canSwap,
  defaultRoute,
  feeText,
  listedPools,
  noRouteText,
  ROUTES,
  routeWhy,
  shieldedRoute,
  transferFor,
  TRANSFER_COPY as T,
  type Pool,
  type Pools,
  QUOTE_ON_CONTINUE,
  type RouteId,
  type TransferModel,
} from "../transfer/model";
import { useCoinJoinView, useMixingAction } from "../coinjoin/queries";
import { FundBody } from "./PlatformFundSheet";
import { MoveBody, usePlatformHold } from "./PlatformMoveSheet";
import { ShieldedBody } from "./ShieldedSheet";
import { GrantSheet } from "./tools/common";

/** The balances the sheet shows and checks, from the reads Home's cards use. */
export function useTransferPools(walletId: string, network: NetworkInfo): Pools {
  const b = useBackend();
  const spendable = useQuery({ queryKey: [...keys.coinsAll(walletId), "transferSpendable"], queryFn: () => b.maxSpendable(walletId, { kind: "any" }, { kind: "perKb", duffsPerKb: 1000 }) });
  const coins = useUtxos(walletId, true);
  const { status: mixing } = useCoinJoinView(walletId, network);
  const statusRead = useShieldedStatus(walletId);
  const status = statusRead.isError ? null : statusRead.data;
  const pools = usePoolBalances(walletId);
  const hold = usePlatformHold(walletId, transferFor(b, walletId).flows.move);
  const fmt = useAmount(network);
  const core = spendable.data ?? null;
  const reserved = coins.data?.filter((c) => c.reserved && c.coinjoinDenominated).reduce((n, c) => n + c.amount, 0) ?? 0;
  return {
    core,
    mixing: (mixing?.balances.coinjoinAccount ?? 0) + (mixing && mixing.state !== "idle" ? reserved : 0),
    shielded: status ? { active: status.active, syncing: shieldedSyncing(status), duffs: shieldedDuffs(status) } : status,
    platform:
      pools.platform == null
        ? pools.platform
        : { duffs: pools.platform, dust: BigInt(pools.platformDust) > 0n, dustText: MOVE_COPY.card.dust(fmt(creditsToDuffs(pools.platformDust))), block: hold.block, waitingText: hold.waitingText },
  };
}

/** An amount in the display unit, masked in discreet mode. */
function useAmount(network: NetworkInfo) {
  const shown = useDisplay(network);
  const discreet = useUi((s) => s.discreet);
  return (duffs: number) => (discreet ? shown.masked() : `${shown.compact(duffs)} ${shown.name}`);
}

const balanceOf = (pool: Pool, pools: Pools): number | null => (pool === "core" ? pools.core : pool === "shielded" ? (pools.shielded?.duffs ?? null) : (pools.platform?.duffs ?? null));

/** The Running lines: another route's call that runs, or whose result has not been seen. */
function RunningLines({ t, here }: { t: TransferModel; here: RouteId | null }) {
  const { shielded, fund, move } = t.flows;
  const lines: { key: string; text: string; show: () => void }[] = [];
  const s = shieldedRoute(shielded.kind);
  if ((shielded.running || shielded.unseen) && s !== here)
    lines.push({
      key: s,
      text: shielded.running ? T.running(T.routeNames[s]) : SHIELDED_COPY.unseen,
      show: () => {
        if (s !== "send") return t.start(s);
        t.close();
        shielded.begin("send");
      },
    });
  if ((fund.running || fund.unseen) && here !== "coreToPlatform") lines.push({ key: "coreToPlatform", text: fund.running ? T.running(T.routeNames.coreToPlatform) : FUND_COPY.unseen, show: () => t.start("coreToPlatform") });
  if ((move.running || move.unseen) && here !== "platformToCore") lines.push({ key: "platformToCore", text: move.running ? T.running(T.routeNames.platformToCore) : MOVE_COPY.unseen, show: () => t.start("platformToCore") });
  return lines.map((l) => (
    <p key={l.key} className="field-hint transfer-running" data-testid="transfer-running" data-route={l.key}>
      {l.text}{" "}
      <DashButton variant="plainBlue" size="small" data-testid="transfer-running-show" onClick={l.show}>
        {T.show}
      </DashButton>
    </p>
  ));
}

/** One end: the pool's icon-less name and balance, a button that opens the picker. */
function End({ t, end, pools, fmt, pickerId, receive }: { t: TransferModel; end: "from" | "to"; pools: Pools; fmt: (d: number) => string; pickerId: string; receive?: number | null }) {
  const pool = t[end];
  const balance = balanceOf(pool, pools);
  return (
    <button
      type="button"
      className="transfer-end"
      data-testid={`transfer-${end}`}
      data-pool={pool}
      aria-expanded={t.picking === end}
      aria-controls={t.picking === end ? pickerId : undefined}
      aria-label={`${T[end]}: ${T.pools[pool]}, ${end === "to" ? `+${fmt(receive ?? 0)}` : balance === null ? "balance not known" : fmt(balance)}. ${T.pick[end]}`}
      onClick={() => t.togglePicker(end)}
    >
      <span className="transfer-end-label">{T[end]}</span>
      <span className="transfer-end-pool">{T.pools[pool]}</span>
      <span className="transfer-end-balance">{end === "to" ? `+${fmt(receive ?? 0)}` : balance === null ? T.unknownBalance : fmt(balance)}</span>
      {end === "from" && pool === "core" && !!pools.mixing && <span className="field-hint" data-testid="transfer-mixing">{fmt(pools.mixing)} mixing</span>}
    </button>
  );
}

function Picker({ t, end, pools, fmt, id }: { t: TransferModel; end: "from" | "to"; pools: Pools; fmt: (d: number) => string; id: string }) {
  return (
    <ul className="transfer-picker" id={id} data-testid="transfer-picker" aria-label={T.pick[end]}>
      {listedPools(pools).map((pool) => {
        const balance = balanceOf(pool, pools);
        return (
          <li key={pool}>
            <button type="button" className="transfer-picker-item" data-testid={`transfer-picker-${pool}`} aria-pressed={t[end] === pool} onClick={() => t.pick(end, pool, pools)}>
              <span className="transfer-end-pool">{T.pools[pool]}</span>
              <span className="transfer-end-balance">{balance === null ? T.unknownBalance : fmt(balance)}</span>
            </button>
          </li>
        );
      })}
    </ul>
  );
}

/** The amount as typed, read in the display unit: `undefined` while it is being read. */
function useParsed(walletText: string, network: NetworkInfo): number | null | undefined {
  const b = useBackend();
  const shown = useDisplay(network);
  const text = walletText.trim();
  const parsed = useQuery({ queryKey: ["parseAmount", shown.unit, text], queryFn: () => b.parseAmount(text, shown.unit), enabled: text !== "", staleTime: Infinity });
  if (text === "") return null;
  if (parsed.isPending) return undefined;
  return typeof parsed.data === "number" && parsed.data > 0 ? parsed.data : null;
}

const INTRO: Record<RouteId, { text: string; testId?: string }> = {
  coreToShielded: { text: SHIELDED_COPY.form.add },
  shieldedToCore: { text: SHIELDED_COPY.form.out },
  coreToPlatform: { text: FUND_COPY.form.intro, testId: "platform-fund-intro" },
  platformToCore: { text: T.platformToCoreIntro },
};

/** The form of a route funded by an amount (every route but Platform → Dash wallet). */
function AmountForm({ t, route, pools, why, network, walletId, status }: { t: TransferModel; route: Exclude<RouteId, "platformToCore">; pools: Pools; why: string | null; network: NetworkInfo; walletId: string; status: ShieldedStatus | null }) {
  const id = useId();
  const shown = useDisplay(network);
  const fmt = useAmount(network);
  const flow = t.flowOf(route);
  useModel(flow);
  const line = useModel(t.lineOf(route));
  const pending = usePendingPlatformFunds(walletId).data;
  const discreet = useUi((s) => s.discreet);
  const own = t.begunRoute === route;
  const text = own ? flow.text : "";
  // Max's figure is the pool's balance: in discreet mode it shows masked until it is typed over.
  const mask = shown.masked();
  const hidden = discreet && text !== "" && t.maxText[route] === text;
  const amount = useParsed(text, network);
  const blocked = why !== null || flow.running;
  const probe = balanceOf(ROUTES[route].from, pools);
  // The fee line follows the amount once it is read, and the source balance; nothing is quoted while
  // the route can't run.
  useEffect(() => {
    if (amount !== undefined) t.typed(route, blocked ? null : amount, probe);
  }, [t, route, amount, blocked, probe]);
  useEffect(() => () => line.set(null), [line]);
  const amountProblem = text.trim() !== "" && amount === null ? T.invalidAmount : route === "coreToPlatform" && amount != null && probe !== null && amount > probe ? `Not enough spendable Dash: ${fmt(probe)} available.` : null;
  const { status: mixing } = useCoinJoinView(walletId, network);
  const pause = useMixingAction(walletId, network);
  const refusal =
    line.state.kind !== "refused"
      ? null
      : route === "coreToPlatform"
        ? fundErrorText(line.state.error, (c) => fmt(creditsToDuffs(c)), fmt, { request: null, pending })
        : shieldedErrorText(line.state.error, (c) => fmt(creditsToDuffs(c)), fmt, route === "coreToShielded" ? "add" : "out", shieldedSyncing(status));
  const fee = QUOTE_ON_CONTINUE.has(route) ? T.feeOnNextStep : feeText(route, line.state, fmt);
  // A refusal of this very amount: Continue would only meet it again.
  const refusedHere = line.state.kind === "refused" && line.state.amount === amount;
  const ready = !blocked && amount != null && amountProblem === null && !refusedHere;
  const continueWhy = why ?? amountProblem ?? refusal ?? (amount == null ? T.why.amount : null);
  // Adds locked earlier that this version can't finish: said here, where Add to Platform is offered.
  const stuck = route === "coreToPlatform" ? (pending ?? []).map((p) => ({ p, row: fundRowView(p, fmt) })).filter(({ row }) => row.kind === "stuck") : [];
  return (
    <form
      className="topup-form"
      onSubmit={(e) => {
        e.preventDefault();
        if (!ready || amount == null) return;
        if (route === "coreToPlatform") t.flows.fund.next(amount);
        else if (route === "coreToShielded") t.flows.shielded.next({ kind: "add", duffs: amount });
        else t.flows.shielded.next({ kind: "out", credits: String(BigInt(amount) * CREDITS_PER_DUFF) });
      }}
    >
      <p className="sheet-text" data-testid={INTRO[route].testId}>
        {INTRO[route].text}
      </p>
      {why && (
        <p className="field-hint" role="status" id={`${id}-why`} data-testid="transfer-unavailable">
          {why}
        </p>
      )}
      {stuck.map(({ p, row }) => (
        <p key={p.txid} className="field-hint" data-testid="platform-fund-stuck" data-state={p.state}>
          {row.text}
        </p>
      ))}
      <div className="transfer-amount-row">
        <label className="form-field">
          {T.amount} ({shown.name})
          <input
            data-testid="transfer-amount"
            data-autofocus
            inputMode="decimal"
            placeholder={shown.placeholder}
            value={hidden ? mask : text}
            disabled={blocked || !own}
            aria-invalid={amountProblem !== null}
            aria-describedby={why ? `${id}-why ${id}-fee` : `${id}-fee`}
            onFocus={(e) => hidden && e.target.select()}
            onChange={(e) => {
              if (line.maxing) line.set(null);
              flow.setText(hidden ? e.target.value.split(mask).join("") : e.target.value);
            }}
          />
        </label>
        <DashButton
          variant="tintedBlue"
          size="medium"
          data-testid="transfer-max"
          aria-busy={line.maxing}
          disabled={blocked || !own || !probe || line.maxing}
          why={why ?? (probe === null ? T.loading : !probe ? T.noCoreBalance : null)}
          title={line.maxing ? T.why.max : undefined}
          whyId={why ? `${id}-why` : undefined}
          onClick={() => probe && void t.max(route, probe, (duffs) => flow.setText(shown.full(duffs)), shown.full)}
        >
          {T.max}<span className="spinner" aria-hidden="true" style={{ visibility: line.maxing ? "visible" : "hidden" }} />
        </DashButton>
      </div>
      <p className={refusal || amountProblem ? "form-problem" : "field-hint"} id={`${id}-fee`} role="status" data-testid="transfer-fee">
        {amountProblem ?? refusal ?? fee ?? ""}
      </p>
      {route === "coreToPlatform" && mixing && mixing.state !== "idle" && <div className="card-actions"><DashButton variant="plainBlue" data-testid="transfer-pause-mixing" disabled={pause.busy} onClick={() => void pause.run("stop")}>Pause mixing</DashButton></div>}
      {pause.problem && <p className="form-problem" role="alert">{pause.problem}</p>}
      <div className="sheet-actions">
        <DashButton variant="tintedGray" size="large" type="button" data-testid="transfer-cancel" onClick={() => t.close()}>
          {T.cancel}
        </DashButton>
        <DashButton variant="filledBlue" size="large" type="submit" data-testid="transfer-continue" disabled={!ready} why={continueWhy} whyId={why ? `${id}-why` : refusal || amountProblem ? `${id}-fee` : undefined}>
          {T.continue}
        </DashButton>
      </div>
    </form>
  );
}

/** Platform → Dash wallet: its whole balance, read once when the route is picked (as the move's own
 * sheet read it when it opened); Continue shows the move's confirmation, which keeps that quote. */
function WholeBalanceForm({ t, pools, why, network, walletId }: { t: TransferModel; pools: Pools; why: string | null; network: NetworkInfo; walletId: string }) {
  const id = useId();
  const shown = useDisplay(network);
  const fmt = useAmount(network);
  const move = useModel(t.flows.move);
  const discreet = useUi((s) => s.discreet);
  const activity = usePlatformActivity(walletId);
  const blocked = why !== null || move.running;
  const quote = usePlatformMoveQuote(walletId, !blocked);
  const q = quote.data;
  const credits = (c: string) => fmt(creditsToDuffs(c));
  const movable = pools.platform?.duffs ?? null;
  const refusal = quote.isError ? moveErrorText(quote.error, credits, activity.data) : null;
  const ready = !blocked && !!q && !quote.isError;
  const p = pools.platform;
  // A move Platform has not confirmed holds this route: Activity shows it.
  const waiting = why !== null && (move.pending || p?.block === "pending");
  return (
    <form
      className="topup-form"
      onSubmit={(e) => {
        e.preventDefault();
        if (ready) t.review();
      }}
    >
      <p className="sheet-text">{INTRO.platformToCore.text}</p>
      {why && (
        <p className="field-hint" role="status" id={`${id}-why`} data-testid="transfer-unavailable">
          {why}
        </p>
      )}
      {waiting && (
        <div className="card-actions">
          <DashButton
            variant="plainBlue"
            data-testid="platform-move-view"
            onClick={() => {
              t.close();
              navigate("activity");
            }}
          >
            {MOVE_COPY.waiting.action}
          </DashButton>
        </div>
      )}
      {p?.dust && why !== p.dustText && (
        <p className="field-hint" data-testid="transfer-dust">
          {p.dustText}
        </p>
      )}
      <div className="transfer-amount-row">
        <label className="form-field">
          {T.amount} ({shown.name})
          <input data-testid="transfer-amount" readOnly aria-readonly="true" aria-describedby={`${id}-whole`} value={discreet ? shown.masked() : q ? shown.full(creditsToDuffs(plannedWithdrawalCredits(q))) : movable !== null ? shown.full(movable) : ""} />
        </label>
        <DashButton variant="tintedBlue" size="medium" data-testid="transfer-max" aria-pressed="true" disabled why={T.wholeBalance} whyId={`${id}-whole`}>
          {T.max}
        </DashButton>
      </div>
      <p className="field-hint" id={`${id}-whole`} data-testid="transfer-whole">
        {T.wholeBalance}
      </p>
      <p className={refusal ? "form-problem" : "field-hint"} id={`${id}-fee`} role="status" data-testid="transfer-fee">
        {blocked ? "" : (refusal ?? (q ? T.fee(credits(q.feeCredits), fmt(q.expectedDuffs)) : T.quoting))}
      </p>
      <div className="sheet-actions">
        <DashButton variant="tintedGray" size="large" type="button" data-testid="transfer-cancel" onClick={() => t.close()}>
          {T.cancel}
        </DashButton>
        <DashButton variant="filledBlue" size="large" type="submit" data-testid="transfer-continue" disabled={!ready} why={why ?? refusal ?? T.quoting} whyId={why ? `${id}-why` : refusal ? `${id}-fee` : undefined}>
          {T.continue}
        </DashButton>
      </div>
    </form>
  );
}

function useTransferReceive(t: TransferModel, route: RouteId | "unsupported" | null, walletId: string, network: NetworkInfo): number | null {
  // Subscribe to every route so selecting another pool keeps hook order stable.
  const add = useModel(t.lines.coreToShielded);
  const fund = useModel(t.lines.coreToPlatform);
  const out = useModel(t.lines.shieldedToCore);
  const move = usePlatformMoveQuote(walletId, false);
  useModel(t.flows.shielded);
  useModel(t.flows.fund);
  const parsed = useParsed(route === "coreToPlatform" ? t.flows.fund.text : t.flows.shielded.text, network);
  if (route === "platformToCore") return move.data?.expectedDuffs ?? null;
  const line = route === "coreToShielded" ? add : route === "coreToPlatform" ? fund : out;
  if (line.state.kind !== "quoted") return parsed ?? null;
  const q = line.state.quote;
  return "expectedDuffs" in q ? q.expectedDuffs ?? creditsToDuffs(q.credits) : creditsToDuffs(q.credits);
}

function TransferForm({ t, pools, network, walletId, status }: { t: TransferModel; pools: Pools; network: NetworkInfo; walletId: string; status: ShieldedStatus | null }) {
  const fmt = useAmount(network);
  const pickerId = useId();
  const routeNow = t.route;
  const receive = useTransferReceive(t, routeNow, walletId, network);
  if (t.awaitingDefault)
    return (
      <>
        <p className="sheet-text" role="status" data-testid="transfer-loading">
          {T.loading}
        </p>
        <div className="sheet-actions">
          <DashButton variant="tintedGray" size="large" data-testid="transfer-cancel" onClick={() => t.close()}>
            {T.cancel}
          </DashButton>
        </div>
      </>
    );
  if (defaultRoute(pools) === null)
    return (
      <>
        <p className="sheet-text" role="status" data-testid="transfer-unavailable">
          {noRouteText(pools)}
        </p>
        <div className="sheet-actions">
          <DashButton variant="filledBlue" size="large" data-testid="transfer-cancel" onClick={() => t.close()}>
            {SHIELDED_COPY.close}
          </DashButton>
        </div>
      </>
    );
  const route = t.route;
  const why = route && route !== "unsupported" ? routeWhy(route, pools, t.flows) : null;
  return (
    <>
      <div className="transfer-ends">
        <End t={t} end="from" pools={pools} fmt={fmt} pickerId={pickerId} />
        <button type="button" className="transfer-swap" data-testid="transfer-swap" aria-label={T.swap} title={T.swap} disabled={!canSwap(t.from, t.to)} onClick={() => t.swap()}>
          <Icon id="transfer" size={20} />
        </button>
        <End t={t} end="to" pools={pools} fmt={fmt} pickerId={pickerId} receive={receive} />
      </div>
      {t.picking && <Picker t={t} end={t.picking} pools={pools} fmt={fmt} id={pickerId} />}
      {route === "unsupported" ? (
        <p className="field-hint" role="status" data-testid="transfer-unavailable">
          {T.unsupported}
        </p>
      ) : route === "platformToCore" ? (
        <WholeBalanceForm t={t} pools={pools} why={why} network={network} walletId={walletId} />
      ) : route ? (
        <AmountForm key={route} t={t} route={route} pools={pools} why={why} network={network} walletId={walletId} status={status} />
      ) : null}
    </>
  );
}

/** The route above its own steps. */
function RouteHeader({ route }: { route: RouteId }) {
  return (
    <p className="transfer-route" data-testid="transfer-route">
      <span>{T.pools[ROUTES[route].from]}</span>
      <span aria-hidden="true"> → </span>
      <VisuallyHidden> to </VisuallyHidden>
      <span>{T.pools[ROUTES[route].to]}</span>
    </p>
  );
}

function Steps({ t, route, walletId, network, status, block }: { t: TransferModel; route: RouteId; walletId: string; network: NetworkInfo; status: ShieldedStatus | null; block: MoveBlock | null }) {
  switch (route) {
    case "coreToShielded":
    case "shieldedToCore":
      return <ShieldedBody flow={t.flows.shielded} walletId={walletId} network={network} status={status} />;
    case "coreToPlatform":
      return <FundBody flow={t.flows.fund} walletId={walletId} network={network} />;
    case "platformToCore":
      return <MoveBody flow={t.flows.move} walletId={walletId} network={network} block={block} />;
  }
}

/** Home's Transfer sheet; it also carries the passphrase sheets of the Platform routes. */
export function TransferSheet({ walletId, network }: { walletId: string; network: NetworkInfo }) {
  const b = useBackend();
  const t = useModel(transferFor(b, walletId));
  const { shielded, fund, move } = t.flows;
  useModel(shielded);
  useModel(fund);
  useModel(move);
  const pools = useTransferPools(walletId, network);
  const status = useShieldedStatus(walletId).data ?? null;
  const { block } = usePlatformHold(walletId, move);
  const defaultHere = defaultRoute(pools);
  useEffect(() => t.settleDefault(defaultHere), [t, t.awaitingDefault, defaultHere]);
  // After a pick the picked item is gone: focus goes to the amount when the route can run, else back
  // to that end's button (after the sheet's own move to its first control).
  const body = useRef<HTMLDivElement>(null);
  const picked = t.picked;
  useEffect(() => {
    if (!picked) return;
    const el = body.current;
    (el?.querySelector<HTMLElement>("[data-autofocus]:not(:disabled)") ?? el?.querySelector<HTMLElement>(`[data-testid="transfer-${picked.end}"]`))?.focus();
  }, [picked]);
  const grants = (
    <>
      <GrantSheet grants={fund.grants} />
      <GrantSheet grants={move.grants} />
    </>
  );
  if (!t.shown) return grants;
  const steps = t.stepFlow();
  const route = t.route;
  const here = steps ?? (route && route !== "unsupported" ? route : null);
  const stepName = steps === null ? "form" : steps === "coreToPlatform" ? fund.step : steps === "platformToCore" ? move.step : shielded.step;
  return (
    <>
      <Sheet title={T.title} width="small" testId="transfer-sheet" onClose={() => t.close()} focusKey={`${here ?? ""}:${stepName}`}>
        <div ref={body} className="transfer" data-testid="transfer-body" data-route={here ?? route ?? ""} data-view={steps ? "steps" : "form"}>
          <RunningLines t={t} here={here} />
          {steps ? (
            <>
              <RouteHeader route={steps} />
              <Steps t={t} route={steps} walletId={walletId} network={network} status={status} block={block} />
              {(t.flowOf(steps).running || t.flowOf(steps).unseen) && <DashButton variant="plainBlue" data-testid="transfer-choose-route" onClick={() => t.chooseRoute()}>Another transfer</DashButton>}
            </>
          ) : (
            <TransferForm t={t} pools={pools} network={network} walletId={walletId} status={status} />
          )}
        </div>
      </Sheet>
      {grants}
    </>
  );
}
