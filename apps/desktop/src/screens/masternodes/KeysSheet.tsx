// Masternode keys (IOS-083, MG-10 §1.5, MG-00 D7): segments Owner · Voting · Operator · Platform
// node, twenty keypairs a page, each "Matched to" the known masternodes that carry it (D8), and one
// keypair's secret revealed behind the RevealSecret gate. The keypair panel keeps the window out of
// screen captures while it is open; its secret is dropped on hide, on another keypair, on a role
// change and on close.

import { useEffect, useRef, useState } from "react";
import { useQueries } from "@tanstack/react-query";
import type { MasternodeDetail } from "../../api/generated/MasternodeDetail";
import type { MasternodeKeyInfo } from "../../api/generated/MasternodeKeyInfo";
import type { MasternodeKeyRoleName } from "../../api/generated/MasternodeKeyRoleName";
import { hasCode } from "../../api/backend";
import { useBackend, useLockStatus } from "../../api/queries";
import { useSecretScreen } from "../../app/ui";
import { DashButton } from "../../components/basics";
import { CopyValue, DetailRow } from "../../components/detail";
import { PassphrasePrompt } from "../../components/passphrase";
import { noCopy, ProtectionPending } from "../../components/phrase";
import { Sheet } from "../../components/sheet";
import { shortAddress } from "../../format/tx";
import { useT } from "../../i18n";
import { canCopySecret, KEY_ROLES, KEYS_PAGE, KeyRevealModel, keyMatches, keysErrorText, knownMatches, type KeyMatch, type MatchState } from "../../masternodes/keys";
import { rowName } from "../../masternodes/model";
import { mnKeys, useMasternodes } from "../../masternodes/queries";
import { useModel } from "../../onboarding/model";
import { useLockTimer } from "../LockScreen";

const P = "masternodes.keys";

export function MasternodeKeysSheet({
  walletId,
  watchOnly: noSeed,
  role: initialRole,
  onClose,
}: {
  walletId: string;
  /** The wallet holds no seed (watch-only, or left so by Forgot Passphrase): public keys only. */
  watchOnly: boolean;
  role: MasternodeKeyRoleName | null;
  onClose: () => void;
}) {
  const t = useT();
  const [role, setRole] = useState<MasternodeKeyRoleName>(initialRole ?? "owner");
  const [pages, setPages] = useState(1);
  const [open, setOpen] = useState<number | null>(null);
  const known = useKnownDetails(walletId);
  const { details } = known;
  const keys = useKeyPages(walletId, role, pages);
  // Try again leaves once the page is read: focus goes to the list, not the page body.
  const list = useRef<HTMLUListElement>(null);
  const refocusList = useRef(false);
  useEffect(() => {
    // A retry that failed again leaves nothing to refocus later.
    if (!keys.retrying && keys.error !== null) refocusList.current = false;
  }, [keys.retrying, keys.error]);
  useEffect(() => {
    if (!refocusList.current || keys.error !== null) return;
    refocusList.current = false;
    // Only when focus fell to the page; a role picked meanwhile keeps it.
    if (document.activeElement === null || document.activeElement === document.body) list.current?.focus();
  }, [keys.error]);
  const choose = (r: MasternodeKeyRoleName) => {
    setRole(r);
    setPages(1);
    setOpen(null);
  };
  const rows = keys.keys.map((k) => ({ key: k, matches: keyMatches(k, details, (d) => rowName(d.row)) }));
  const matched = rows.filter((r) => r.matches.length > 0).length;
  return (
    <Sheet title={t(`${P}.title`)} width="large" testId="mn-keys" onClose={onClose}>
      {keys.watchOnly ? (
        <p className="sheet-text" data-testid="mn-keys-watch-only">
          {t(`${P}.watchOnly`)}
        </p>
      ) : (
        <>
          <p className="sheet-text" data-testid="mn-keys-intro">
            {t(`${P}.intro`)} {known.state === "done" && matched > 0 && t(`${P}.matchedCount`, { count: matched })}
          </p>
          {noSeed && (
            <p className="sheet-warning" role="note" data-testid="mn-keys-no-seed">
              {t(`${P}.noSeed`)}
            </p>
          )}
          {known.state === "failed" && (
            <p className="mg-notice-row" data-testid="mn-keys-check-failed">
              <span className="field-error">{t(`${P}.checkFailedLine`)}</span>
              <DashButton variant="tintedBlue" size="small" data-testid="mn-keys-check-retry" onClick={known.retry}>
                {t("components.phrase.tryAgain")}
              </DashButton>
            </p>
          )}
          <div className="segmented" role="group" aria-label={t(`${P}.rolesLabel`)} data-testid="mn-keys-roles">
            {KEY_ROLES.map((r) => (
              <button key={r} type="button" className="segment" aria-pressed={role === r} data-testid={`mn-keys-role-${r}`} onClick={() => choose(r)}>
                {t(`${P}.roles.${r}`)}
              </button>
            ))}
          </div>
          {keys.error !== null && (
            <p className="mg-notice-row" data-testid="mn-keys-failed">
              <span className="field-error" role="alert">
                {keys.error}
              </span>
              <DashButton
                variant="tintedBlue"
                size="small"
                data-testid="mn-keys-retry"
                aria-disabled={keys.retrying || undefined}
                onClick={() => {
                  if (keys.retrying) return;
                  refocusList.current = true;
                  keys.retry();
                }}
              >
                {t("components.phrase.tryAgain")}
              </DashButton>
            </p>
          )}
          {keys.loading && rows.length === 0 ? (
            <p className="sheet-text" role="status">
              {t(`${P}.loading`)}
            </p>
          ) : (
            <ul role="list" className="mg-key-list card-l1" data-testid="mn-keys-list" ref={list} tabIndex={-1}>
              {rows.map(({ key, matches }) => (
                <li key={`${key.role}-${key.index}`} className="mg-key-row" data-testid={`mn-key-${key.index}`}>
                  <div className="mg-notice-row">
                    <span>
                      <span>{t(`${P}.keypair`, { n: key.index + 1 })}</span>
                      {" · "}
                      <span className="mono">{shortKey(key)}</span>
                      {" · "}
                      <span className="mg-muted" data-testid={`mn-key-${key.index}-match`}>
                        {known.state === "done"
                          ? matchText(t, matches)
                          : t(known.state === "checking" ? `${P}.checking` : `${P}.checkFailed`)}
                      </span>
                    </span>
                    <DashButton
                      variant="plainBlue"
                      size="small"
                      aria-expanded={open === key.index}
                      data-testid={`mn-key-${key.index}-show`}
                      onClick={() => setOpen(open === key.index ? null : key.index)}
                    >
                      {open === key.index ? t(`${P}.hide`) : t(`${P}.show`)}
                    </DashButton>
                  </div>
                  {open === key.index && <KeyPanel key={`${walletId}-${key.role}-${key.index}`} walletId={walletId} k={key} noSeed={noSeed} />}
                </li>
              ))}
            </ul>
          )}
          {keys.more && (
            <DashButton variant="plainBlue" data-testid="mn-keys-more" disabled={keys.loading} onClick={() => setPages((n) => n + 1)}>
              {t(`${P}.showMore`)}
            </DashButton>
          )}
        </>
      )}
    </Sheet>
  );
}

type Translate = ReturnType<typeof useT>;

function matchText(t: Translate, matches: KeyMatch[]): string {
  if (matches.length === 0) return t(`${P}.notMatched`);
  const m = matches[0];
  const name = matches.map((x) => x.name).join(t("common.listSeparator"));
  return matches.length === 1 && m.service ? t(`${P}.matchedToService`, { name, service: m.service }) : t(`${P}.matchedTo`, { name });
}

/** The row's short form: the address, else the node id, else the public key. */
function shortKey(k: MasternodeKeyInfo): string {
  if (k.address) return shortAddress(k.address);
  const long = k.platformNodeId ?? k.publicKeyHex;
  return `${long.slice(0, 6)}…${long.slice(-4)}`;
}

/** Every page of `role` up to `pages`; one more page is offered while the last came back full. */
function useKeyPages(walletId: string, role: MasternodeKeyRoleName, pages: number) {
  const b = useBackend();
  const results = useQueries({
    queries: Array.from({ length: pages }, (_, i) => ({
      queryKey: mnKeys.keychain(walletId, role, i * KEYS_PAGE, KEYS_PAGE),
      queryFn: () => b.mnKeys(walletId, role, i * KEYS_PAGE, KEYS_PAGE),
    })),
  });
  const failed = results.find((r) => r.isError)?.error ?? null;
  const watchOnly = failed !== null && hasCode(failed, "masternode.watch_only");
  const keys = results.flatMap((r) => r.data ?? []);
  const last = results[results.length - 1];
  const error = failed !== null && !watchOnly ? keysErrorText(failed) : null;
  // A page that never loaded drops its error while it is read again, and an offline read is paused
  // rather than fetching: the retried error and an inert Try again stay until this role's read
  // settles, and are forgotten once it does or the role changes.
  const [retried, setRetried] = useState<{ role: MasternodeKeyRoleName; error: string } | null>(null);
  // A refetch dispatches its fetch synchronously, so the render after Try again already reads.
  const reading = results.some((r) => r.isFetching || r.isPaused);
  const retrying = retried !== null && retried.role === role && reading;
  useEffect(() => {
    if (retried !== null && !retrying) setRetried(null);
  }, [retried, retrying]);
  return {
    keys,
    watchOnly,
    error: error ?? (retrying ? retried.error : null),
    loading: results.some((r) => r.isPending),
    more: last?.data?.length === KEYS_PAGE,
    /** Reads the failed pages again; the pages already in stay on screen. */
    retry: () => {
      if (retrying || error === null) return;
      setRetried({ role, error });
      for (const r of results) if (r.isError) void r.refetch();
    },
    retrying,
  };
}

/** The details of the masternodes this wallet knows (its own and tracked), for matching
 * (`knownMatches`), with `retry` after a failed read. */
function useKnownDetails(walletId: string): { details: MasternodeDetail[]; state: MatchState; retry: () => void } {
  const b = useBackend();
  const list = useMasternodes(walletId);
  const hashes = (list.data?.masternodes ?? []).map((r) => r.proTxHash);
  const details = useQueries({
    queries: hashes.map((h) => ({ queryKey: mnKeys.detail(walletId, h), queryFn: () => b.mnMasternode(walletId, h) })),
  });
  const retry = () => {
    if (list.isError) void list.refetch();
    for (const d of details) if (d.isError) void d.refetch();
  };
  return { ...knownMatches(list, details), retry };
}

function KeyPanel({ walletId, k, noSeed }: { walletId: string; k: MasternodeKeyInfo; noSeed: boolean }) {
  const t = useT();
  const protection = useSecretScreen();
  return (
    <dl className="detail-list" data-testid={`mn-key-${k.index}-panel`}>
      <DetailRow label={t(`${P}.path`)}>
        <CopyValue value={k.derivationPath} label={t(`${P}.path`)} mono />
      </DetailRow>
      {k.address && (
        <DetailRow label={t(`${P}.address`)} testId={`mn-key-${k.index}-address`}>
          <CopyValue value={k.address} label={t(`${P}.address`)} mono truncate />
        </DetailRow>
      )}
      {k.platformNodeId && (
        <DetailRow label={t(`${P}.nodeId`)} testId={`mn-key-${k.index}-node-id`}>
          <CopyValue value={k.platformNodeId} label={t(`${P}.nodeId`)} mono truncate />
        </DetailRow>
      )}
      {!k.address && (
        <DetailRow label={t(`${P}.publicKey`)} testId={`mn-key-${k.index}-public-key`}>
          <CopyValue value={k.publicKeyHex} label={t(`${P}.publicKey`)} mono truncate />
        </DetailRow>
      )}
      {k.legacyPublicKeyHex && (
        <DetailRow label={t(`${P}.legacyPublicKey`)} testId={`mn-key-${k.index}-legacy-key`}>
          <CopyValue value={k.legacyPublicKeyHex} label={t(`${P}.legacyPublicKey`)} mono truncate />
        </DetailRow>
      )}
      <DetailRow label={t(`${P}.privateKey`)} testId={`mn-key-${k.index}-secret`}>
        {noSeed ? (
          <span className="mg-small mg-muted" data-testid={`mn-key-${k.index}-no-seed`}>
            {t(`${P}.noSeed`)}
          </span>
        ) : protection !== "on" ? (
          <ProtectionPending protection={protection} />
        ) : (
          <Reveal walletId={walletId} k={k} />
        )}
      </DetailRow>
    </dl>
  );
}

function Reveal({ walletId, k }: { walletId: string; k: MasternodeKeyInfo }) {
  const t = useT();
  const backend = useBackend();
  const [model] = useState(() => new KeyRevealModel(backend, walletId, k.role, k.index));
  const [asking, setAsking] = useState(false);
  useModel(model);
  useLockTimer(model.lock);
  useEffect(() => {
    void model.load();
    return () => model.dispose();
  }, [model]);
  // A shown secret hides when the window loses focus and when the vault locks.
  const lock = useLockStatus();
  const locked = lock.data?.state === "locked" || lock.data?.state === "unlockedMixingOnly";
  useEffect(() => {
    if (locked) model.hide();
  }, [locked, model]);
  useEffect(() => {
    const away = () => model.hide();
    window.addEventListener("blur", away);
    return () => window.removeEventListener("blur", away);
  }, [model]);
  const s = model.secret;
  if (model.step === "shown" && s) {
    return (
      <div className="mg-key-secret" data-testid={`mn-key-${k.index}-revealed`}>
        {s.wif && <SecretLine label={t(`${P}.wif`)} value={s.wif} testId="wif" role={k.role} />}
        <SecretLine label={t(`${P}.hex`)} value={s.privateKeyHex} testId="hex" role={k.role} />
        {s.tenderdashKey && <SecretLine label={t(`${P}.tenderdash`)} value={s.tenderdashKey} testId="tenderdash" role={k.role} />}
        <p className="mg-small mg-muted" data-testid={canCopySecret(k.role) ? "mn-key-copy-warning" : "mn-key-no-copy"}>
          {t(canCopySecret(k.role) ? `${P}.copyWarning` : `${P}.noCopy`)}
        </p>
        <DashButton
          variant="plainBlue"
          size="small"
          data-testid={`mn-key-${k.index}-hide-secret`}
          onClick={() => {
            model.hide();
            setAsking(false);
          }}
        >
          {t(`${P}.hideSecret`)}
        </DashButton>
      </div>
    );
  }
  return (
    <div>
      <p className="mg-small">
        <span data-testid={`mn-key-${k.index}-hidden`}>{t(`${P}.hidden`)}</span>
      </p>
      <p className="mg-small mg-muted">
        {model.encrypted && `${t(`${P}.needsPassphrase`)} `}
        {t(`${P}.revealNote.${k.role}`)}
      </p>
      <p className="mg-small mg-muted" data-testid={`mn-key-${k.index}-screen-warning`}>
        {t(`${P}.screenWarning`)}
      </p>
      {model.error && (
        <p className="form-problem" role="alert" data-testid={`mn-key-${k.index}-error`}>
          {model.error}
        </p>
      )}
      {!asking ? (
        <DashButton variant="tintedBlue" size="small" data-testid={`mn-key-${k.index}-reveal`} disabled={model.step === "loading" || model.step === "failed"} onClick={() => setAsking(true)}>
          {t(`${P}.reveal`)}
        </DashButton>
      ) : model.encrypted ? (
        <PassphrasePrompt
          testId={`mn-key-${k.index}-passphrase`}
          error={model.lock.message}
          busy={model.lock.working || model.step === "working"}
          disabled={model.lock.blocked}
          submitLabel={t(`${P}.revealShow`)}
          onSubmit={(p) => model.reveal(p)}
        />
      ) : (
        <DashButton
          variant="filledBlue"
          size="small"
          data-autofocus
          data-testid={`mn-key-${k.index}-reveal-confirm`}
          disabled={model.step === "working"}
          onClick={() => void model.reveal("")}
        >
          {model.step === "working" ? t(`${P}.revealing`) : t(`${P}.revealShow`)}
        </DashButton>
      )}
    </div>
  );
}

function SecretLine({ label, value, testId, role }: { label: string; value: string; testId: string; role: MasternodeKeyRoleName }) {
  return (
    <div className="mg-key-secret-line" data-testid={`mn-key-secret-${testId}`}>
      <span className="mg-small mg-muted">{label}</span>
      {canCopySecret(role) ? (
        <CopyValue value={value} label={label} mono />
      ) : (
        <span className="mono mg-key-no-copy" translate="no" onCopy={noCopy} onCut={noCopy}>
          {value}
        </span>
      )}
    </div>
  );
}
