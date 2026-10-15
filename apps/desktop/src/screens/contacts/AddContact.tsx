// Add Contact (DP2-08): the Contacts detail slot's Search, Scan and My QR (D2), and the card of the
// candidate a hit, a scan or a `dashpay://user` link shows. The card's Send is DP2-09's
// `ContactActions.send` (D5); its other next steps select the contact, where DP2-09's pane acts.

import { useEffect, useMemo, useRef, useState, useSyncExternalStore, type ClipboardEvent, type DragEvent } from "react";
import type { ContactSummaryInfo } from "../../api/generated/ContactSummaryInfo";
import type { UserHit } from "../../api/generated/UserHit";
import { useBackend, useQrMatrix } from "../../api/queries";
import { navigate, ui, type AddContactIntent, type ContactPrefill } from "../../app/ui";
import { useDebounced } from "../../app/useDebounced";
import { Chip, DashButton, Icon } from "../../components/basics";
import { CopyValue } from "../../components/detail";
import { QrCode } from "../../components/qr";
import { copyWithToast } from "../../components/toast";
import type { ActionOutcome, ContactActions } from "../../contacts/actions";
import {
  ADD_TABS,
  AddContactModel,
  candidateName,
  cardLine,
  claimNote,
  currentRelation,
  myQr,
  ownerLine,
  SCREEN_SHORTCUT,
  type AddTab,
  type Candidate,
  type CardLine,
} from "../../contacts/add";
import { initials, NETWORK_DEBOUNCE_MS, networkPrefix, relationTitle, type ContactsOwner } from "../../contacts/model";
import { useEligibility, useMyUserLink, useNetworkSearch, useScreenScanMode } from "../../contacts/queries";
import { useModel } from "../../onboarding/model";
import { Outcome } from "../tools/common";
import { Tabs } from "../Routes";
import { useActionToast, useRefocus } from "./afterAction";
import { Avatar } from "./Avatar";

/** What Add Contact acts for: the wallet, whose contacts (BL-153's status), the identity's actions
 * (`null` without one) and its unfiltered contacts (D14; `undefined` while unread). */
export interface AddContext {
  walletId: string;
  owner: ContactsOwner;
  actions: ContactActions | null;
  listed: ContactSummaryInfo[] | undefined;
}

/** Selects `contact` in the list, closing Add Contact (D4's Show). */
const select = (contact: string) => ui.set({ contact, addContact: null, contactPrefill: null });

/** Closes the slot; focus goes back to the header's Add contact (D15). */
function close() {
  ui.set({ addContact: null, contactPrefill: null });
  requestAnimationFrame(() => document.querySelector<HTMLElement>('[data-testid="contacts-add"]')?.focus());
}

export function AddContact({ ctx, intent, link }: { ctx: AddContext; intent: AddContactIntent | null; link: ContactPrefill | null }) {
  const backend = useBackend();
  const model = useModel(useMemo(() => new AddContactModel(backend, ctx.walletId), [backend, ctx.walletId]));
  const identity = ctx.owner.kind === "identity" ? ctx.owner.identity : null;
  const [tab, setTab] = useState<AddTab>("search");
  // A link verifies once per link and wallet (D13). A new intent starts afresh on Search, with a
  // network row's hit shown at once (its full hit from the cached search when there).
  const searchHits = useNetworkSearch(ctx.walletId, identity ?? "", intent?.hit && identity ? networkPrefix(intent.query) : null);
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
    const h = intent?.hit;
    if (!h) return model.clear();
    model.pick(searchHits.data?.find((x) => x.identity === h.identity) ?? { ...h, displayName: null, avatarUrl: null, relation: "stranger", unverified: false });
  }, [model, link, intent, searchHits.data]);
  // Leaving stops a screen capture still waiting.
  useEffect(() => () => model.cancelScreen(), [model]);

  const scan = model.scan;
  const linkState = scan.kind === "verifying" || scan.kind === "failed" ? (scan.source === "link" ? scan : null) : null;
  return (
    <section className="add-contact add-contact-slot card-l1" aria-labelledby="add-contact-title" data-testid="add-contact">
      <header className="add-contact-header">
        <h2 id="add-contact-title" className="add-contact-title">
          Add a New Contact
        </h2>
        <DashButton variant="plainBlue" size="small" data-testid="add-contact-close" onClick={close}>
          Close
        </DashButton>
      </header>
      {model.candidate && <CandidateCard ctx={ctx} candidate={model.candidate} />}
      {linkState && link && (
        <div className="add-contact-candidate" data-testid="add-contact-link">
          <p className="contact-row-username" data-testid="add-contact-username">
            @{link.username}
          </p>
          {linkState.kind === "verifying" ? (
            <p role="status" data-testid="add-contact-checking">
              Checking @{link.username} on the Dash network…
            </p>
          ) : (
            <ScanProblem text={linkState.text} retry={linkState.retry ? () => void model.retry() : null} testId="add-contact-link-error" />
          )}
        </div>
      )}
      {identity !== null && (
        <Tabs label="Add a contact by" tabs={ADD_TABS} current={tab} onSelect={setTab} testId="add-contact-tabs">
          {tab === "search" && <SearchTab ctx={ctx} identity={identity} query={intent?.query ?? ""} onPick={(h) => model.pick(h)} />}
          {tab === "scan" && <ScanTab model={model} />}
          {tab === "myQr" && <MyQrTab walletId={ctx.walletId} identity={identity} />}
        </Tabs>
      )}
    </section>
  );
}

/** A scan's refusal, with Try again after a network failure; it takes focus when it shows (D15). */
function ScanProblem({ text, retry, testId }: { text: string; retry: (() => void) | null; testId: string }) {
  const ref = useRef<HTMLParagraphElement>(null);
  useEffect(() => ref.current?.focus(), [text]);
  return (
    <p className="form-problem add-contact-problem" role="alert" tabIndex={-1} ref={ref} data-testid={testId}>
      {text}
      {retry && (
        <>
          {" "}
          <DashButton variant="plainBlue" size="small" onClick={retry}>
            Try again
          </DashButton>
        </>
      )}
    </p>
  );
}

const SOURCES: Record<Candidate["source"], string> = {
  search: "Found by username search",
  scan: "From a QR code",
  link: "From a link",
};

/** The candidate (D4–D7, D13, D14): who, whether Platform vouched for them, and the one next step. */
function CandidateCard({ ctx, candidate: c }: { ctx: AddContext; candidate: Candidate }) {
  const backend = useBackend();
  const identity = ctx.owner.kind === "identity" ? ctx.owner.identity : null;
  const relation = currentRelation(c, ctx.listed);
  const asks = identity !== null && relation !== "ignored" && relation !== "isSelf";
  const eligibility = useEligibility(ctx.walletId, identity, asks ? c.identity : null);
  const line: CardLine | undefined =
    ctx.owner.kind !== "identity" ? ownerLine(ctx.owner) : cardLine(relation, { data: eligibility.data, failed: eligibility.isError && !eligibility.isFetching });
  const actions = useOptionalModel(ctx.actions);
  const name = candidateName(c);
  const outcome = actions?.outcome?.kind === "send" && actions.outcome.contact === c.identity ? actions.outcome : null;

  // A new candidate takes focus (D15).
  const heading = useRef<HTMLHeadingElement>(null);
  useEffect(() => heading.current?.focus(), [c]);
  // Send leaves once the re-read says the request went: focus goes to the line that says so (D5).
  const status = useRef<HTMLDivElement>(null);
  const toast = useActionToast(ctx.walletId);
  const arm = useRefocus(() => line?.next === "send", () => status.current);
  const sent = (o: ActionOutcome | null) => {
    toast(o, true);
    if (o?.ok) arm("send");
  };

  let button = null;
  switch (line?.next) {
    case "send":
      button = (
        <DashButton
          key="send"
          variant="filledBlue"
          data-testid="add-contact-send"
          disabled={!actions || actions.busy}
          onClick={() => void actions?.send(c.identity, name, c.claim?.scan ?? null).then(sent)}
        >
          {actions?.isRunning("send", c.identity) ? "Sending…" : "Send Contact Request"}
        </DashButton>
      );
      break;
    case "showContact":
    case "showRequest":
    case "showHidden":
      button = (
        <DashButton key="show" variant="tintedBlue" data-testid="add-contact-show" onClick={() => select(c.identity)}>
          {line.next === "showContact" ? "Show Contact" : line.next === "showRequest" ? "Show Request" : "Show in Hidden"}
        </DashButton>
      );
      break;
    case "enable":
      button = (
        <DashButton key="enable" variant="tintedBlue" data-testid="add-contact-enable" disabled={!actions || actions.busy} onClick={() => void actions?.enable().then((o) => toast(o, true))}>
          {actions?.isRunning("enable", null) ? "Enabling…" : "Enable"}
        </DashButton>
      );
      break;
    case "join":
      button = (
        <DashButton key="join" variant="filledBlue" data-testid="add-contact-join" onClick={() => navigate("joinDashPay")}>
          Join DashPay
        </DashButton>
      );
      break;
    case "retry":
      button = (
        <DashButton key="retry" variant="plainBlue" data-testid="add-contact-retry" onClick={() => void eligibility.refetch()}>
          Try again
        </DashButton>
      );
      break;
  }
  const enableOutcome = actions?.outcome?.kind === "enable" ? actions.outcome : null;
  const note = claimNote(c.claim, backend.now().getTime());
  return (
    <div className="add-contact-candidate" data-testid="add-contact-candidate">
      <header className="contact-detail-header">
        <Avatar letters={initials(c.displayName || c.username || "")} size="pane" />
        <div className="contact-detail-names">
          <h3 className="add-contact-name" tabIndex={-1} ref={heading} data-testid="add-contact-name">
            {name}
          </h3>
          {c.username && c.displayName && (
            <p className="contact-row-username" data-testid="add-contact-username">
              @{c.username}
            </p>
          )}
        </div>
      </header>
      <p className="add-contact-identity" data-testid="add-contact-identity">
        <CopyValue value={c.identity} label="identity" mono />
      </p>
      <p className="add-contact-meta" data-testid="add-contact-verified">
        {c.unverified ? <Chip tone="warning">Unverified</Chip> : <Chip tone="info">Verified on the Dash network</Chip>}
        <span className="add-contact-source">{SOURCES[c.source]}</span>
      </p>
      {note && (
        <p className="placeholder-note" data-testid="add-contact-claim">
          {note}
        </p>
      )}
      <div className="add-contact-status" ref={status} data-testid="add-contact-status">
        {line === undefined ? (
          <p role="status">Checking…</p>
        ) : (
          line.text && (
            <p className="placeholder-note" data-testid="add-contact-line">
              {line.text}
            </p>
          )
        )}
        {button && <span className="add-contact-actions">{button}</span>}
        <Outcome ok={outcome?.ok ?? true} text={outcome?.text ?? null} testId="add-contact-outcome" />
        {enableOutcome && !enableOutcome.ok && <Outcome ok={false} text={enableOutcome.text} testId="add-contact-enable-outcome" />}
      </div>
    </div>
  );
}

const NONE = { subscribe: () => () => undefined, version: () => 0 };

/** `useModel` for a model that may be absent. */
function useOptionalModel<M extends ContactActions>(model: M | null): M | null {
  useSyncExternalStore((model ?? NONE).subscribe, (model ?? NONE).version);
  return model;
}

/** Search (D3): every hit of the prefix, with how it relates to the identity. */
function SearchTab({ ctx, identity, query, onPick }: { ctx: AddContext; identity: string; query: string; onPick: (hit: UserHit) => void }) {
  const [text, setText] = useState(query);
  // Another intent's search text (Show more on another search).
  useEffect(() => setText(query), [query]);
  const typed = networkPrefix(text);
  const debounced = useDebounced(typed, NETWORK_DEBOUNCE_MS);
  // Clearing the field hides the hits at once.
  const prefix = typed === null ? null : debounced;
  const search = useNetworkSearch(ctx.walletId, identity, prefix);
  const hits = search.data ?? [];
  return (
    <div className="add-contact-search">
      <label className="search-field" data-testid="add-contact-search">
        <Icon id="searchMagnifyingGlass" size={16} className="search-icon" />
        <input type="search" value={text} placeholder="Search username" aria-label="Search username" onChange={(e) => setText(e.target.value)} autoFocus={query !== ""} />
      </label>
      {prefix === null ? (
        <p className="placeholder-note" data-testid="add-contact-search-hint">
          Search for a User on the Dash Network by their username.
        </p>
      ) : search.isPending || (search.isError && search.isFetching) ? (
        <p className="contacts-network-note" role="status">
          Searching…
        </p>
      ) : search.isError ? (
        <p className="contacts-network-note" role="alert" data-testid="add-contact-search-error">
          Could not search the Dash network.{" "}
          <DashButton variant="plainBlue" size="small" onClick={() => void search.refetch()}>
            Try again
          </DashButton>
        </p>
      ) : hits.length === 0 ? (
        <p className="contacts-network-note" data-testid="add-contact-search-none">
          No usernames found
        </p>
      ) : (
        <ul className="tx-group-rows" aria-label="Usernames">
          {hits.map((h) => {
            const displayName = h.displayName?.trim();
            const relation = currentRelation({ identity: h.identity, relation: h.relation }, ctx.listed);
            return (
              <li key={h.identity}>
                <button type="button" className="contact-row tx-row" data-testid="add-contact-hit" onClick={() => onPick({ ...h, relation })}>
                  <Avatar letters={initials(displayName || h.username)} />
                  <span className="contact-row-text tx-row-text">
                    <span className="contact-row-name tx-row-title">{displayName || h.username}</span>
                    {displayName && <span className="contact-row-username">@{h.username}</span>}
                  </span>
                  <span className="contact-row-trailing">
                    {relation !== "stranger" && <span className="contact-row-note">{relationTitle(relation)}</span>}
                    {h.unverified && <Chip tone="warning">Unverified</Chip>}
                  </span>
                </button>
              </li>
            );
          })}
        </ul>
      )}
    </div>
  );
}

/** The files a drag carries, or its text (a dragged link). */
function dropped(e: DragEvent | ClipboardEvent): { file: File | null; text: string } {
  const data = "dataTransfer" in e ? e.dataTransfer : e.clipboardData;
  const file = [...(data?.files ?? [])][0] ?? null;
  return { file, text: file ? "" : (data?.getData("text/uri-list") || data?.getData("text/plain") || "") };
}

/** Scan (D9, D10): a drop, Open Image…, a screen region, a paste. */
function ScanTab({ model }: { model: AddContactModel }) {
  const mode = useScreenScanMode();
  const [over, setOver] = useState(false);
  const [typed, setTyped] = useState("");
  const scan = model.scan;
  const busy = scan.kind === "reading" || scan.kind === "capturing" || scan.kind === "verifying";
  const take = async ({ file, text }: { file: File | null; text: string }) => {
    if (file) await model.imageBytes(new Uint8Array(await file.arrayBuffer()));
    else if (text.trim()) await model.verifyText(text);
  };
  const onDrop = (e: DragEvent) => {
    // The document's guard (D11) leaves a drop the area took alone.
    e.preventDefault();
    setOver(false);
    void take(dropped(e));
  };
  const onPaste = (e: ClipboardEvent<HTMLInputElement>) => {
    const got = dropped(e);
    // A pasted image goes to the decoder; a pasted text is verified without entering the field
    // (it may carry a key, D16).
    e.preventDefault();
    void take(got);
  };
  const status =
    scan.kind === "reading"
      ? "Reading the image…"
      : scan.kind === "capturing"
        ? "Select a region of the screen…"
        : scan.kind === "verifying" && scan.source === "scan"
          ? "Checking the code on the Dash network…"
          : null;
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
        <span>Drop a QR code image here</span>
        <span className="add-contact-actions">
          <DashButton variant="tintedBlue" size="small" data-testid="add-contact-open-image" disabled={busy} onClick={() => void model.openImage()}>
            Open Image…
          </DashButton>
          {mode.data === "picker" &&
            (scan.kind === "capturing" ? (
              <DashButton key="cancel" variant="tintedGray" size="small" data-testid="add-contact-screen-cancel" onClick={() => model.cancelScreen()}>
                Cancel
              </DashButton>
            ) : (
              <DashButton key="screen" variant="tintedBlue" size="small" data-testid="add-contact-screen" disabled={busy} onClick={() => void model.screen()}>
                Scan a Screen Region
              </DashButton>
            ))}
        </span>
        {mode.data === "copyShortcut" && (
          <p className="placeholder-note" data-testid="add-contact-screen-shortcut">
            {SCREEN_SHORTCUT}
          </p>
        )}
      </div>
      <label className="form-field">
        Paste a link or a QR code image
        <input
          type="text"
          data-testid="add-contact-paste"
          value={typed}
          autoComplete="off"
          spellCheck={false}
          placeholder="dashpay://user?… or a copied image"
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
function MyQrTab({ walletId, identity }: { walletId: string; identity: string }) {
  const link = useMyUserLink(walletId, identity);
  const state = myQr(link);
  const matrix = useQrMatrix(state.kind === "link" ? state.link : undefined);
  let body;
  switch (state.kind) {
    case "loading":
      body = <p role="status">Loading…</p>;
      break;
    case "noUsername":
      body = (
        <p className="placeholder-note" data-testid="add-contact-qr-no-username">
          Your username is not registered yet, so there is no link to share.
        </p>
      );
      break;
    case "updating":
      body = (
        <p className="placeholder-note" data-testid="add-contact-qr-updating">
          Your username is being updated…{" "}
          <DashButton variant="plainBlue" size="small" onClick={() => void link.refetch()}>
            Try again
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
      body = (
        <>
          {matrix.data ? (
            <QrCode matrix={matrix.data} size={220} label="Your DashPay QR code" testId="add-contact-qr" />
          ) : (
            <div className="qr-placeholder" style={{ width: 220, height: 220 }} aria-hidden="true" />
          )}
          <p className="mono add-contact-link-text" data-testid="add-contact-qr-link">
            {state.link}
          </p>
          <span className="add-contact-actions">
            <DashButton variant="tintedBlue" size="small" data-testid="add-contact-copy-link" onClick={() => void copyWithToast(state.link)}>
              Copy Link
            </DashButton>
          </span>
        </>
      );
  }
  return (
    <div className="add-contact-my-qr" data-testid="add-contact-my-qr">
      <p className="placeholder-note">Others scan this code or open the link to add you.</p>
      {body}
    </div>
  );
}
