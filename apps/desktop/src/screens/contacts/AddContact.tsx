// Add Contact (DP2-08): the Contacts detail slot's Search, Scan and My QR (D2), and the card of the
// candidate a hit, a scan or a `dashpay://user` link shows. The card's Send is DP2-09's
// `ContactActions.send` (D5); its other next steps select the contact, where DP2-09's pane acts.

import { useEffect, useMemo, useRef, useState, useSyncExternalStore, type ClipboardEvent, type DragEvent, type ReactNode, type RefObject } from "react";
import type { ContactSummaryInfo } from "../../api/generated/ContactSummaryInfo";
import { useBackend, useQrMatrix } from "../../api/queries";
import { navigate, ui, type AddContactIntent, type ContactPrefill } from "../../app/ui";
import { useDebounced } from "../../app/useDebounced";
import { Chip, DashButton, Icon } from "../../components/basics";
import { CopyValue } from "../../components/detail";
import { QrCode, QrPlaceholder } from "../../components/qr";
import { CopyDashButton } from "../../components/copy";
import type { ActionOutcome, ContactActions } from "../../contacts/actions";
import {
  ADD_TABS,
  AddContactModel,
  answersAtOnce,
  candidateName,
  cardLine,
  claimLine,
  claimNote,
  currentRelation,
  myQr,
  ownerLine,
  scanStatus,
  AddText,
  type AddTab,
  type Candidate,
  type CardLine,
  type HitFields,
  type Listed,
  watchClaim,
} from "../../contacts/add";
import { initials, NETWORK_DEBOUNCE_MS, networkPrefix, relationTitle, unverifiedWarning, type ContactsOwner } from "../../contacts/model";
import { useEligibility, useMyUserLink, useNetworkSearch, useScreenScanMode } from "../../contacts/queries";
import { useT } from "../../i18n";
import { useModel } from "../../onboarding/model";
import { Tabs } from "../Routes";
import { dropDetailFocusUnless, focusDetailWhenShown, focusInView, useActionToast, useRefocus } from "./afterAction";
import { Avatar } from "./Avatar";
import { Row } from "./ContactList";
import { EnableButton, SendRequestButton } from "./RequestButton";

/** What Add Contact acts for: the wallet, whose contacts (BL-153's status), the identity's actions
 * (`null` without one) and its unfiltered contacts (D14; `undefined` while unread). */
export interface AddContext {
  walletId: string;
  owner: ContactsOwner;
  actions: ContactActions | null;
  listed: Listed<ContactSummaryInfo> | undefined;
}

/** Selects `contact` in the list, closing Add Contact (D4's Show); the button pressed goes with
 * the slot, so the contact's detail takes focus at its name once it shows. */
function select(contact: string) {
  focusDetailWhenShown(contact);
  ui.set({ contact, addContact: null, contactPrefill: null });
}

/** Closes the slot; focus goes back to the header's Add contact (D15). */
function close() {
  dropDetailFocusUnless(null);
  ui.set({ addContact: null, contactPrefill: null });
  requestAnimationFrame(() => document.querySelector<HTMLElement>('[data-testid="contacts-add"], [data-testid="contacts-empty-search"]')?.focus());
}

export function AddContact({ ctx, intent, link }: { ctx: AddContext; intent: AddContactIntent | null; link: ContactPrefill | null }) {
  const backend = useBackend();
  const t = useT();
  const model = useModel(useMemo(() => new AddContactModel(backend, ctx.walletId), [backend, ctx.walletId]));
  const identity = ctx.owner.kind === "identity" ? ctx.owner.identity : null;
  const [tab, setTab] = useState<AddTab>("search");
  // A link verifies once per link and wallet (D13). A new intent starts afresh on Search, with a
  // network row's hit shown at once (its full hit from the cached search when there).
  const searchHits = useNetworkSearch(ctx.walletId, identity ?? "", intent?.hit && identity ? networkPrefix(intent.query) : null);
  // Search's text lives here so a tab switch keeps it. A search hit's card is for the text it was
  // picked under: other text drops it, so Send never goes to a hit the list no longer shows.
  const [text, setText] = useState(intent?.query ?? "");
  const pickedFor = useRef<string | null>(null);
  const pickHit = (hit: HitFields, under: string) => {
    pickedFor.current = networkPrefix(under);
    model.pick(hit);
  };
  const seen = useRef<AddContactIntent | ContactPrefill | null>(null);
  useEffect(() => {
    const current = link ?? intent;
    if (current === seen.current) return;
    seen.current = current;
    if (link) {
      void model.verifyLink(link.identity, link.username);
      return;
    }
    setTab("search");
    const query = intent?.query ?? "";
    setText(query);
    const h = intent?.hit;
    if (!h) return model.clear();
    pickHit(searchHits.data?.find((x) => x.identity === h.identity) ?? { ...h, displayName: null, relation: "stranger", unverified: true, readSeq: 0 }, query);
  }, [model, link, intent, searchHits.data]);
  const typed = networkPrefix(text);
  const stale = model.candidate?.source === "search" && typed !== pickedFor.current;
  useEffect(() => {
    if (stale) model.clear();
  }, [model, stale]);
  // Leaving stops a screen capture still waiting.
  useEffect(() => () => model.cancelScreen(), [model]);

  const scan = model.scan;
  const linkState = (scan.kind === "verifying" || scan.kind === "failed") && scan.source === "link" ? scan : null;
  const candidate = stale ? null : model.candidate;
  const heading = useRef<HTMLHeadingElement>(null);
  useEffect(() => {
    if (candidate) focusInView(heading.current);
  }, [candidate]);

  const result = candidate ? (
    <CandidateCard key={candidate.identity} ctx={ctx} candidate={candidate} heading={heading} />
  ) : linkState && link ? (
    <div className="add-contact-candidate" data-testid="add-contact-link">
      <p className="contact-row-username" data-testid="add-contact-username">
        <bdi>@{link.username}</bdi>
      </p>
      {linkState.kind === "verifying" ? (
        <p role="status" data-testid="add-contact-checking">
          {t.rich("contacts.addContact.checkingLink", { username: <bdi>@{link.username}</bdi> })}
        </p>
      ) : (
        <ScanProblem text={linkState.text} retry={linkState.retry ? () => void model.retry() : null} testId="add-contact-link-error" />
      )}
    </div>
  ) : null;
  return (
    <section className="add-contact add-contact-slot card-l1" aria-labelledby="add-contact-title" data-testid="add-contact">
      <header className="add-contact-header">
        <h2 id="add-contact-title" className="add-contact-title">
          {t("contacts.addContact.title")}
        </h2>
        <DashButton variant="plainBlue" size="small" data-testid="add-contact-close" onClick={close}>
          {t("contacts.addContact.close")}
        </DashButton>
      </header>
      {identity === null ? result : (
        <Tabs label={t("contacts.addContact.tabsLabel")} tabs={ADD_TABS} current={tab} onSelect={setTab} testId="add-contact-tabs">
          {tab === "search" && <SearchTab ctx={ctx} identity={identity} intent={intent} text={text} onText={setText} onPick={(h) => pickHit(h, text)} result={result} />}
          {tab === "scan" && <ScanTab model={model} result={result} />}
          {tab === "myQr" && (
            <div className="add-contact-my-qr">
              {result}
              <MyQrTab walletId={ctx.walletId} identity={identity} />
            </div>
          )}
        </Tabs>
      )}
    </section>
  );
}

/** A scan's refusal, with Try again after a network failure; it takes focus when it shows (D15). */
function ScanProblem({ text, retry, testId }: { text: string; retry: (() => void) | null; testId: string }) {
  const t = useT();
  const ref = useRef<HTMLParagraphElement>(null);
  useEffect(() => ref.current?.focus(), [text]);
  return (
    <p className="form-problem add-contact-problem" role="alert" tabIndex={-1} ref={ref} data-testid={testId}>
      {text}
      {retry && (
        <>
          {" "}
          <DashButton variant="plainBlue" size="small" onClick={retry}>
            {t("contacts.tryAgain")}
          </DashButton>
        </>
      )}
    </p>
  );
}

/** The candidate (D4–D7, D13, D14, D34, D36): who, whether Platform vouched for them, and the one next step. */
function CandidateCard({ ctx, candidate: c, heading }: { ctx: AddContext; candidate: Candidate; heading: RefObject<HTMLHeadingElement | null> }) {
  const t = useT();
  const identity = ctx.owner.kind === "identity" ? ctx.owner.identity : null;
  const relation = currentRelation(c, ctx.listed);
  const asks = identity !== null && !answersAtOnce(relation);
  const eligibility = useEligibility(ctx.walletId, identity, asks ? c.identity : null);
  const now = useClaimClock(c.claim);
  const line: CardLine | undefined =
    ctx.owner.kind !== "identity" ? ownerLine(ctx.owner) : claimLine(cardLine(relation, { data: eligibility.data, failed: eligibility.isError && !eligibility.isFetching }), c.claim, now);
  const actions = useOptionalModel(ctx.actions);
  const name = candidateName(c);

  const action = useRef<HTMLDivElement>(null);
  const toast = useActionToast(ctx.walletId);
  const arm = useRefocus((next) => line?.next === next, () => action.current?.querySelector("button") ?? heading.current);
  const done = (next: "send" | "enable", o: ActionOutcome | null) => {
    toast(o, false);
    if (o?.ok) arm(next);
  };

  let button = null;
  switch (line?.next) {
    case "send":
      button = <SendRequestButton key="send" actions={actions} contact={c.identity} name={name} scan={c.claim?.scan ?? null} testId="add-contact-send" onDone={(o) => done("send", o)} />;
      break;
    case "showContact":
    case "showRequest":
    case "showHidden":
      button = (
        <DashButton key="show" variant="tintedBlue" data-testid="add-contact-show" onClick={() => select(c.identity)}>
          {t(`contacts.addContact.show.${line.next}`)}
        </DashButton>
      );
      break;
    case "enable":
      button = <EnableButton key="enable" actions={actions} testId="add-contact-enable" onDone={(o) => done("enable", o)} />;
      break;
    case "join":
      button = (
        <DashButton key="join" variant="filledBlue" data-testid="add-contact-join" onClick={() => navigate("joinDashPay")}>
          {t("contacts.joinDashPay")}
        </DashButton>
      );
      break;
    case "retry":
      button = (
        <DashButton key="retry" variant="plainBlue" data-testid="add-contact-retry" onClick={() => void eligibility.refetch()}>
          {t("contacts.tryAgain")}
        </DashButton>
      );
      break;
  }
  const note = claimNote(c.claim, now);
  const warning = line?.next === "send" ? unverifiedWarning("send", c.unverified) : null;
  return (
    <div className="add-contact-candidate" data-testid="add-contact-candidate">
      <header className="contact-detail-header">
        <Avatar letters={initials(c.displayName || c.username || "")} size="pane" identity={c.identity} />
        <div className="contact-detail-names">
          <h3 className="add-contact-name" tabIndex={-1} ref={heading} data-testid="add-contact-name">
            <bdi>{name}</bdi>
          </h3>
          {c.username && c.displayName && (
            <p className="contact-row-username" data-testid="add-contact-username">
              <bdi>@{c.username}</bdi>
            </p>
          )}
        </div>
      </header>
      <p className="add-contact-identity" data-testid="add-contact-identity">
        <CopyValue value={c.identity} action={t("contacts.addContact.copyIdentity")} mono truncate />
      </p>
      <p className="add-contact-meta" data-testid="add-contact-verified">
        {c.unverified ? <Chip tone="warning">{t("contacts.unverified")}</Chip> : <Chip tone="info">{t("contacts.addContact.verified")}</Chip>}
        <span className="add-contact-source">{t(`contacts.addContact.sources.${c.source}`)}</span>
      </p>
      {c.claim && (
        <div className="add-contact-claim-slot">
          {note && <p className="placeholder-note" data-testid="add-contact-claim">{note}</p>}
        </div>
      )}
      <div className="add-contact-status" data-testid="add-contact-status">
        {c.unverified && (
          <div className="add-contact-unverified-slot">
            {warning && <p className="sheet-warning" role="note" data-testid="add-contact-unverified">{warning}</p>}
          </div>
        )}
        <div className="add-contact-action-row">
          <div className="add-contact-action-slot" ref={action}>
            {line === undefined ? (
              <div className="add-contact-action-skeleton" role="status">
                <span className="sr-only">{t("contacts.addContact.checking")}</span>
              </div>
            ) : button}
          </div>
          {line?.text && <p className="placeholder-note add-contact-action-note" data-testid="add-contact-line">{line.text}</p>}
        </div>
      </div>
    </div>
  );
}

/** The renderer's clock (ms) for `claim`'s note and line, re-read while it ages (D36). */
function useClaimClock(claim: Candidate["claim"]): number {
  const backend = useBackend();
  const [now, setNow] = useState(() => backend.now().getTime());
  useEffect(() => {
    if (!claim) return;
    const read = () => backend.now().getTime();
    setNow(read());
    return watchClaim(claim, read, () => setNow(read()));
  }, [backend, claim]);
  return now;
}

const NONE = { subscribe: () => () => undefined, version: () => 0 };

/** `useModel` for a model that may be absent. */
function useOptionalModel<M extends ContactActions>(model: M | null): M | null {
  useSyncExternalStore((model ?? NONE).subscribe, (model ?? NONE).version);
  return model;
}

/** Search (D3): every hit of the prefix, with how it relates to the identity. */
function SearchTab({
  ctx,
  identity,
  intent,
  text,
  onText,
  onPick,
  result,
}: {
  ctx: AddContext;
  identity: string;
  intent: AddContactIntent | null;
  text: string;
  onText: (text: string) => void;
  onPick: (hit: HitFields) => void;
  result: ReactNode;
}) {
  const t = useT();
  const input = useRef<HTMLInputElement>(null);
  useEffect(() => {
    const tabs = document.activeElement?.closest('[role="tablist"]');
    if (!tabs || !input.current?.closest(".add-contact")?.contains(tabs)) focusInView(input.current);
  }, [intent]);
  const typed = networkPrefix(text);
  const debounced = useDebounced(typed, NETWORK_DEBOUNCE_MS);
  const prefix = typed === debounced ? debounced : null;
  const search = useNetworkSearch(ctx.walletId, identity, prefix);
  const results = (): ReactNode => {
    if (typed === null)
      return (
        <p className="placeholder-note" data-testid="add-contact-search-hint">
          {t("contacts.addContact.searchHint")}
        </p>
      );
    if (prefix === null || search.isPending || (search.isError && search.isFetching))
      return (
        <p className="contacts-network-note" role="status">
          {t("contacts.searching")}
        </p>
      );
    if (search.isError)
      return (
        <p className="contacts-network-note" role="alert" data-testid="add-contact-search-error">
          {t("contacts.searchFailed")}{" "}
          <DashButton variant="plainBlue" size="small" onClick={() => void search.refetch()}>
            {t("contacts.tryAgain")}
          </DashButton>
        </p>
      );
    if (search.data.length === 0)
      return (
        <p className="contacts-network-note" data-testid="add-contact-search-none">
          {t("contacts.noUsernames")}
        </p>
      );
    return (
      <ul className="tx-group-rows" aria-label={t("contacts.addContact.usernames")}>
        {search.data.map((h) => {
          const displayName = h.displayName?.trim();
          const relation = currentRelation(h, ctx.listed);
          return (
            <Row
              key={h.identity}
              letters={initials(displayName || h.username)}
              identity={h.identity}
              name={displayName || h.username}
              sub={displayName ? `@${h.username}` : null}
              trailing={
                <>
                  {relation !== "stranger" && <span className="contact-row-note">{relationTitle(relation)}</span>}
                  {h.unverified && <Chip tone="warning">{t("contacts.unverified")}</Chip>}
                </>
              }
              data-testid="add-contact-hit"
              onClick={() => onPick(h)}
            />
          );
        })}
      </ul>
    );
  };
  return (
    <div className="add-contact-search">
      <label className="search-field" data-testid="add-contact-search">
        <Icon id="searchMagnifyingGlass" size={16} className="search-icon" />
        <input ref={input} type="search" value={text} placeholder={t("contacts.addContact.searchField")} aria-label={t("contacts.addContact.searchField")} onChange={(e) => onText(e.target.value)} />
      </label>
      <div className={typed !== null || result ? "add-contact-search-results has-space" : "add-contact-search-results"}>{results()}</div>
      {result}
    </div>
  );
}

/** The file a drag or a paste carries, else its text (a dragged link). */
function carried(e: DragEvent | ClipboardEvent): { file: File | null; text: string } {
  const data = "dataTransfer" in e ? e.dataTransfer : e.clipboardData;
  const file = [...(data?.files ?? [])][0] ?? null;
  return { file, text: file ? "" : (data?.getData("text/uri-list") || data?.getData("text/plain") || "") };
}

/** Scan (D9, D10): a drop, Open Image…, a screen region, a paste. */
function ScanTab({ model, result }: { model: AddContactModel; result: ReactNode }) {
  const t = useT();
  const mode = useScreenScanMode();
  const [over, setOver] = useState(false);
  const [typed, setTyped] = useState("");
  const scan = model.scan;
  const busy = scan.kind === "reading" || scan.kind === "capturing" || scan.kind === "verifying";
  const take = ({ file, text }: { file: File | null; text: string }) => (file ? model.imageFile(file) : model.verifyText(text));
  const onDrop = (e: DragEvent) => {
    // The document's guard (D11) leaves a drop the area took alone.
    e.preventDefault();
    setOver(false);
    void take(carried(e));
  };
  const onPaste = (e: ClipboardEvent<HTMLInputElement>) => {
    const got = carried(e);
    // A pasted image goes to the decoder; a pasted text is verified without entering the field
    // (it may carry a key, D16).
    e.preventDefault();
    void take(got);
  };
  const status = scanStatus(scan);
  return (
    <div
      className="add-contact-scan"
      onDragOver={(e) => {
        e.preventDefault();
        e.dataTransfer.dropEffect = "copy";
        setOver(true);
      }}
      onDragLeave={(e) => {
        if (!e.currentTarget.contains(e.relatedTarget as Node | null)) setOver(false);
      }}
      onDrop={onDrop}
    >
      <div className={over ? "add-contact-drop is-over" : "add-contact-drop"} data-testid="add-contact-drop">
        <Icon id="shortcutScanQR" size={32} />
        <span>{t("contacts.addContact.drop")}</span>
        <span className="add-contact-actions">
          <DashButton variant="tintedBlue" size="small" data-testid="add-contact-open-image" disabled={busy} onClick={() => void model.openImage()}>
            {t("contacts.addContact.openImage")}
          </DashButton>
          {mode.data === "picker" &&
            (scan.kind === "capturing" ? (
              <DashButton key="cancel" variant="tintedGray" size="small" data-testid="add-contact-screen-cancel" onClick={() => model.cancelScreen()}>
                {t("contacts.addContact.cancel")}
              </DashButton>
            ) : (
              <DashButton key="screen" variant="tintedBlue" size="small" data-testid="add-contact-screen" disabled={busy} onClick={() => void model.screen()}>
                {t("contacts.addContact.screen")}
              </DashButton>
            ))}
        </span>
        {mode.data === "copyShortcut" && (
          <p className="placeholder-note" data-testid="add-contact-screen-shortcut">
            {AddText.screenShortcut}
          </p>
        )}
      </div>
      <label className="form-field">
        {t("contacts.addContact.paste")}
        <input
          type="text"
          data-testid="add-contact-paste"
          value={typed}
          autoComplete="off"
          spellCheck={false}
          placeholder={t("contacts.addContact.pastePlaceholder")}
          onChange={(e) => setTyped(e.target.value)}
          onPaste={onPaste}
          onKeyDown={(e) => {
            if (e.key !== "Enter" || !typed.trim()) return;
            e.preventDefault();
            const text = typed;
            setTyped("");
            void model.verifyText(text);
          }}
        />
      </label>
      {result}
      {status && (
        <p role="status" data-testid="add-contact-scan-status">
          {status}
        </p>
      )}
      {scan.kind === "failed" && scan.source === "scan" && <ScanProblem text={scan.text} retry={scan.retry ? () => void model.retry() : null} testId="add-contact-scan-error" />}
    </div>
  );
}

/** My QR (D12): the identity's plain `dashpay://user` link. */
/** `qrSize`: the code's side in CSS px; a share sheet shows it larger, to scan across a table. */
/** `username`: the name a share sheet says it shares; a link for another is not shown. */
export function MyQrTab({ walletId, identity, qrSize = 220, username = null }: { walletId: string; identity: string; qrSize?: number; username?: string | null }) {
  const t = useT();
  const link = useMyUserLink(walletId, identity);
  const state = myQr(link, username);
  const matrix = useQrMatrix(state.kind === "link" ? state.link : undefined);
  const loadingQr = <QrPlaceholder size={qrSize} label={t("contacts.addContact.myQrLoading")} testId="add-contact-qr-loading" />;
  let body;
  switch (state.kind) {
    case "loading":
      body = loadingQr;
      break;
    case "noUsername":
      body = (
        <p className="placeholder-note" data-testid="add-contact-qr-no-username">
          {t("contacts.addContact.noUsername")}
        </p>
      );
      break;
    case "updating":
      body = (
        <p className="placeholder-note" data-testid="add-contact-qr-updating">
          {t("contacts.addContact.usernameUpdating")}{" "}
          <DashButton variant="plainBlue" size="small" onClick={() => void link.refetch()}>
            {t("contacts.tryAgain")}
          </DashButton>
        </p>
      );
      break;
    case "failed":
      body = (
        <p className="form-problem" role="alert" data-testid="add-contact-qr-error">
          {state.text}
        </p>
      );
      break;
    case "link":
      body = matrix.data
        ? <QrCode matrix={matrix.data} size={qrSize} label={t("contacts.addContact.qrLabel")} testId="add-contact-qr" />
        : loadingQr;
  }
  return (
    <div className="add-contact-my-qr" data-testid="add-contact-my-qr">
      <p className="placeholder-note">{t("contacts.addContact.myQrHint")}</p>
      {body}
      {(state.kind === "loading" || state.kind === "link") && (
        <div className="add-contact-qr-footer">
          {state.kind === "link" && (
            <>
              <p className="mono add-contact-link-text" data-testid="add-contact-qr-link">{state.link}</p>
              <CopyDashButton variant="tintedBlue" size="small" data-testid="add-contact-copy-link" text={state.link}>
                {t("contacts.addContact.copyLink")}
              </CopyDashButton>
            </>
          )}
        </div>
      )}
    </div>
  );
}
