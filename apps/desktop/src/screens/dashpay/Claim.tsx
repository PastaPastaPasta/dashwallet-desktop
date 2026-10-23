// The invitation claim screen (DP5-03), on the Join page: the stashed invitation's state and its
// inviter (C3–C9), the claim's confirm step with the request's recipient and its opt-out (C11),
// and Join's "Have an invitation?" paste (D14). The name step, the passphrase prompt and the
// claim row's progress and result are the registration wizard's (Registration.tsx). The view model
// is dashpay/claim.ts.

import { useQueryClient } from "@tanstack/react-query";
import { useId, useState } from "react";
import { errorMessage, hasCode } from "../../api/backend";
import { useBackend } from "../../api/queries";
import { removeErrorText } from "../../app/savedLinks";
import { navigate, ui } from "../../app/ui";
import { DashButton } from "../../components/basics";
import { CopyValue } from "../../components/detail";
import { Sheet } from "../../components/sheet";
import { CLAIM_COPY, claimModel, handleOf, recipientLines, type ClaimActionKind, type ClaimEntry, type InviterCard } from "../../dashpay/claim";
import { dashpayKeys, useClaimQuote, useInvitationStatus, useUsernameCheck } from "../../dashpay/queries";
import { REG_COPY } from "../../dashpay/registration";
import type { StatusInputs } from "../../dashpay/status";
import type { RegistrationWizard } from "../../dashpay/wizard";
import { useModel } from "../../onboarding/model";
import { SAVED_LINKS_KEY } from "../SavedLinks";
import { GrantSheet } from "../tools/common";
import { Avatar } from "../contacts/Avatar";
import { ErrorLine, StepHeader } from "./Registration";

/** Leaves the claim screen (D3): welcome goes Home, Join stays on the Join page. */
function leaveClaim(entry: ClaimEntry) {
  if (entry === "welcome") navigate("home", { invitation: null });
  else ui.set({ invitation: null });
}

/** D4: the DPNS-checked "@username" leads; the display name, the inviter's own words, follows. */
function Inviter({ card }: { card: InviterCard }) {
  const shown = card.username ? `@${card.username}` : card.name;
  return (
    <div className="claim-inviter" data-testid="claim-inviter">
      <p className="claim-label">{CLAIM_COPY.invited.from}</p>
      <div className="claim-inviter-row">
        <Avatar letters={(shown ?? "?").replace(/^@/, "").slice(0, 1).toUpperCase()} identity={card.identity} />
        <div className="claim-inviter-names">
          {shown && <span className="claim-inviter-primary" data-testid="claim-inviter-primary">{shown}</span>}
          {card.name && card.username && <span className="claim-inviter-secondary">{card.name}</span>}
        </div>
      </div>
      <p className="registration-line" data-testid="claim-inviter-check">
        {card.line}
      </p>
    </div>
  );
}

/** C3–C9 for the stashed invitation `linkId`. */
export function ClaimScreen({
  linkId,
  entry,
  wiz,
  hasIdentity,
  network,
  rows,
}: {
  linkId: string;
  entry: ClaimEntry;
  wiz: RegistrationWizard;
  hasIdentity: boolean;
  network: string;
  /** The wallet's registrations, read before the status (D5, F7). */
  rows: { data: unknown; isError: boolean; error: unknown; refetch: () => unknown };
}) {
  const id = useId();
  const b = useBackend();
  const client = useQueryClient();
  // D5 (F7): no status before the rows are read once: our own claim in flight never reads
  // "already used". A failed refetch keeps the rows read.
  const read = rows.data !== undefined;
  const status = useInvitationStatus(linkId, !hasIdentity && read);
  const [busy, setBusy] = useState(false);
  const [problem, setProblem] = useState<string | null>(null);
  const statusError = !read && rows.isError ? rows.error : status.isError ? status.error : null;
  const model = claimModel({ entry, hasIdentity, status: read ? status.data : undefined, statusError, network });
  const onAction = async (kind: ClaimActionKind) => {
    switch (kind) {
      case "accept":
        if (status.data?.kind === "valid")
          wiz.begin({ kind: "invitation", linkId, contestedAllowed: status.data.contestedAllowed, inviter: status.data.inviter });
        return;
      case "notNow":
        return leaveClaim(entry);
      case "retry":
        return void (read ? status.refetch() : rows.refetch());
      case "dismiss":
        // D16: the stash forgets it (DP5-01's Remove).
        setBusy(true);
        setProblem(null);
        try {
          await b.dismissStashedLink(linkId);
          void client.invalidateQueries({ queryKey: SAVED_LINKS_KEY });
          leaveClaim(entry);
        } catch (e) {
          setProblem(removeErrorText(e));
        } finally {
          setBusy(false);
        }
    }
  };
  return (
    <section className="registration claim card-l1" data-testid="claim" data-state={model.state} data-entry={entry} aria-labelledby={`${id}-title`}>
      <div role="status" className="registration-live">
        <h2 className="registration-title" id={`${id}-title`}>
          {model.title}
          {model.busy && <span className="spinner registration-spinner" aria-hidden="true" />}
        </h2>
        {model.inviter ? <Inviter card={model.inviter} /> : model.state === "invited" && <p className="registration-line">{CLAIM_COPY.invited.unnamed}</p>}
        <div data-testid="claim-lines">
          {model.lines.map((line) => (
            <p key={line} className="registration-line">
              {line}
            </p>
          ))}
        </div>
      </div>
      {problem && (
        <p className="registration-error" role="alert" data-testid="claim-error">
          {problem}
        </p>
      )}
      <div className="registration-actions">
        {model.actions.map((a, i) => (
          <DashButton key={a.kind} variant={i === 0 ? "filledBlue" : "plainBlue"} data-testid={`claim-${a.kind}`} disabled={busy} onClick={() => void onAction(a.kind)}>
            {a.label}
          </DashButton>
        ))}
      </div>
    </section>
  );
}

/** C11: step 2 of a claim: the name, what pays (the invitation), who the request goes to and
 * what it carries (D6), its opt-out (D7), then Claim under the claim quote's grant (D9). */
export function ClaimConfirmStep({ wiz, walletId, inputs }: { wiz: RegistrationWizard; walletId: string; inputs: StatusInputs }) {
  useModel(wiz);
  const id = useId();
  const [accepted, setAccepted] = useState(false);
  const mode = wiz.mode.kind === "invitation" ? wiz.mode : null;
  const inviter = mode?.inviter ?? null;
  const asking = wiz.requestInviter && inviter !== null;
  const quote = useClaimQuote(walletId, wiz.label, mode?.linkId ?? "", asking);
  const contested = useUsernameCheck(wiz.label).data?.contested ?? false;
  const recipient = recipientLines(inviter);
  const handle = inviter ? handleOf(inviter) : "";
  const disabled = !quote.data || wiz.running !== null || (contested && !accepted);
  return (
    <section className="registration claim card-l1" data-testid="registration-confirm" data-claim="" aria-labelledby={`${id}-title`}>
      <StepHeader n={2} step={REG_COPY.confirm.step} title={REG_COPY.confirm.title} id={`${id}-title`} />
      <p className="registration-line">{REG_COPY.confirm.message(wiz.label)}</p>
      <p className="registration-line" data-testid="claim-paid">
        {CLAIM_COPY.paid}
      </p>
      {quote.isError && <p className="registration-line registration-problem">{REG_COPY.confirm.quoteFailed}</p>}
      {contested && <p className="registration-line">{REG_COPY.confirm.contested}</p>}
      {recipient && (
        <div className="claim-recipient" data-testid="claim-recipient">
          <label className="registration-accept">
            <input type="checkbox" data-testid="claim-request" checked={wiz.requestInviter} onChange={(e) => wiz.setRequestInviter(e.target.checked)} />
            {CLAIM_COPY.confirm.request(handle)}
          </label>
          {asking && (
            <>
              <p className="registration-line" data-testid="claim-recipient-text">
                {recipient.text}
              </p>
              <p className="registration-line claim-recipient-id">
                <span className="claim-label">{CLAIM_COPY.confirm.recipientId}</span>{" "}
                <CopyValue value={recipient.identity} label="identity id" mono />
              </p>
            </>
          )}
        </div>
      )}
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
        <DashButton variant="filledBlue" data-testid="registration-pay" disabled={disabled} onClick={() => quote.data && void wiz.confirm(quote.data.grant)}>
          {wiz.running !== null && <span className="spinner" aria-hidden="true" />}
          {CLAIM_COPY.confirm.claim}
        </DashButton>
      </div>
      <GrantSheet grants={wiz.grants} />
    </section>
  );
}

/** D14: Join's "Have an invitation?": the pasted link goes into the stash at once, and the claim
 * screen opens on its id; the field is cleared. */
export function InvitationPasteSheet({ onClose }: { onClose: () => void }) {
  const b = useBackend();
  const client = useQueryClient();
  const [text, setText] = useState("");
  const [busy, setBusy] = useState(false);
  const [problem, setProblem] = useState<string | null>(null);
  const paste = async () => {
    setBusy(true);
    setProblem(null);
    try {
      const linkId = await b.stashInvitation(text.trim());
      setText("");
      void client.invalidateQueries({ queryKey: dashpayKeys.invitation(linkId) });
      onClose();
      ui.set({ invitation: { linkId, entry: "join" } });
    } catch (e) {
      setProblem(hasCode(e, "invitation.invalid") ? CLAIM_COPY.paste.refused : errorMessage(e));
    } finally {
      setBusy(false);
    }
  };
  return (
    <Sheet title={CLAIM_COPY.paste.title} width="small" testId="claim-paste" onClose={onClose}>
      <form
        className="registration-form"
        onSubmit={(e) => {
          e.preventDefault();
          if (text.trim() !== "") void paste();
        }}
      >
        <label className="form-field">
          {CLAIM_COPY.paste.field}
          <textarea
            rows={3}
            data-testid="claim-paste-field"
            value={text}
            autoFocus
            autoComplete="off"
            spellCheck={false}
            onChange={(e) => setText(e.target.value)}
          />
        </label>
        {problem && (
          <p className="form-problem" role="alert" data-testid="claim-paste-error">
            {problem}
          </p>
        )}
        <div className="sheet-actions">
          <DashButton variant="tintedGray" size="large" type="button" onClick={onClose}>
            {CLAIM_COPY.paste.cancel}
          </DashButton>
          <DashButton variant="filledBlue" size="large" type="submit" data-testid="claim-paste-ok" disabled={busy || text.trim() === ""}>
            {CLAIM_COPY.paste.ok}
          </DashButton>
        </div>
      </form>
    </Sheet>
  );
}
