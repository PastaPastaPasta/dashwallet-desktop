// The Pay page's engine draft (U-03 D6, E0-13 D8): made on first use, closed through
// `closeSendDraft` when the page goes, the wallet changes or the vault locks. A broadcast of
// this page still running at that moment may reach the network, and DEC-67's line says so.
//
// Writing a draft takes several calls (recipients, coins, fee, change), so every write runs on
// one promise chain: an estimate never interleaves with another, and a prepare signs exactly the
// spec it was given, written again right before it.

import { useCallback, useEffect, useRef } from "react";
import { hasCode } from "../../api/backend";
import type { BroadcastOutcome } from "../../api/generated/BroadcastOutcome";
import type { ChangePolicy } from "../../api/generated/ChangePolicy";
import type { CoinSource } from "../../api/generated/CoinSource";
import type { FeeMode } from "../../api/generated/FeeMode";
import type { PreparedTxHandle } from "../../api/generated/PreparedTxHandle";
import type { Recipient } from "../../api/generated/Recipient";
import type { TxEstimate } from "../../api/generated/TxEstimate";
import { closeSendDraft } from "../../api/send";
import { useBackend } from "../../api/queries";
import { showToast } from "../../components/toast";

export interface DraftSpec {
  recipient: Recipient;
  source: CoinSource;
  fee: FeeMode;
  change: ChangePolicy;
}

export interface DraftApi {
  /** Writes `spec` into the draft and estimates it. */
  estimate(spec: DraftSpec): Promise<TxEstimate>;
  /** Writes `spec` into the draft and signs it with `grantId` (nothing is sent). */
  prepare(spec: DraftSpec, grantId: string): Promise<{ draft: number; prepared: PreparedTxHandle }>;
  /** `draftBroadcast`, counted as running until it settles (DEC-67's line on close). */
  broadcast(draft: number, prepared: number): Promise<BroadcastOutcome>;
  /** Releases a prepared transaction that will not be sent. */
  abandon(draft: number, prepared: number): Promise<void>;
  /** Closes the draft (if any); the next write makes a new one. */
  close(): Promise<void>;
}

export function useDraft(walletId: string, locked: boolean): DraftApi {
  const b = useBackend();
  const draft = useRef<Promise<number> | null>(null);
  const chain = useRef<Promise<unknown>>(Promise.resolve());
  const broadcasts = useRef(0);

  /** Runs `job` after every write queued before it. */
  const serial = useCallback(<T,>(job: () => Promise<T>): Promise<T> => {
    const run = chain.current.then(job, job);
    chain.current = run.catch(() => undefined);
    return run;
  }, []);

  const close = useCallback(async () => {
    const pending = draft.current;
    draft.current = null;
    const handle = await pending?.catch(() => null);
    if (handle === null || handle === undefined) return;
    try {
      const { warning } = await closeSendDraft(handle, broadcasts.current > 0);
      if (warning) showToast(warning);
    } catch {
      // A handle a network switch already dropped: nothing left to close.
    }
  }, []);

  /** Writes `spec` into the open draft (a new one if none), then runs `then` on it. A draft a
   * network switch dropped (`app.unknown_handle`) is replaced once. */
  const write = useCallback(
    async <T,>(spec: DraftSpec, then: (d: number) => Promise<T>): Promise<T> => {
      for (let attempt = 0; ; attempt++) {
        if (!draft.current) draft.current = b.newTxDraft(walletId);
        const made = draft.current;
        let d: number;
        try {
          d = await made;
        } catch (e) {
          if (draft.current === made) draft.current = null;
          throw e;
        }
        try {
          await b.draftSetRecipients(d, [spec.recipient]);
          await b.draftSetSource(d, spec.source);
          await b.draftSetFee(d, spec.fee);
          await b.draftSetChange(d, spec.change);
          return await then(d);
        } catch (e) {
          if (attempt === 0 && hasCode(e, "app.unknown_handle")) {
            if (draft.current === made) draft.current = null;
            continue;
          }
          throw e;
        }
      }
    },
    [b, walletId],
  );

  const estimate = useCallback((spec: DraftSpec) => serial(() => write(spec, (d) => b.draftEstimate(d))), [b, serial, write]);

  const prepare = useCallback(
    (spec: DraftSpec, grantId: string) => serial(() => write(spec, async (d) => ({ draft: d, prepared: await b.draftPrepare(d, grantId) }))),
    [b, serial, write],
  );

  const broadcast = useCallback(
    async (d: number, prepared: number) => {
      broadcasts.current++;
      try {
        return await b.draftBroadcast(d, prepared);
      } finally {
        broadcasts.current--;
      }
    },
    [b],
  );

  const abandon = useCallback(async (d: number, prepared: number) => void (await b.draftAbandon(d, prepared).catch(() => null)), [b]);

  // The page goes or shows another wallet: its draft goes too.
  useEffect(() => () => void close(), [close, walletId]);
  // A lock stops new signatures; a new draft is made after the unlock.
  useEffect(() => {
    if (locked) void close();
  }, [close, locked]);

  return { estimate, prepare, broadcast, abandon, close };
}
