// Add Contact's read order (DP2-08 D38) on a real QueryClient: a contacts list read begun before an
// ignore can land after the ignored hit, and must not clear it; the ignore's re-read starts a new
// list read even while the first has no answer yet.

import { QueryClient, QueryObserver } from "@tanstack/react-query";
import { afterEach, describe, expect, it, vi } from "vitest";
import type { ContactList } from "../api/generated/ContactList";
import type { ContactSummaryInfo } from "../api/generated/ContactSummaryInfo";
import type { UserRelation } from "../api/generated/UserRelation";
import { WALLET_A } from "../onboarding/testWorld";
import { candidateOfHit, cardLine, currentRelation, listedNow } from "./add";
import { contactListQuery, networkSearchQuery, refreshAfter } from "./queries";

const ALICE = "AL1cEqb2Ex9xW1nJBuT8rjaWwmYFy3TgnUjvuqeUmY2h";
const GRACE = "GrAC3eQhZmKGwKu4R7PDuBkaxWDcXuspNwGM8RHm9ZsZ";

const row = (relation: UserRelation): ContactSummaryInfo => ({
  contact: GRACE,
  username: "grace",
  displayName: null,
  alias: null,
  relation,
  hidden: relation === "ignored",
  channel: "notEstablished",
  since: null,
  lastActivityAt: null,
  unverified: false,
});
const list = (...rows: ContactSummaryInfo[]): ContactList => ({ sections: [{ section: "requests", contacts: rows }], updating: false });

/** A contacts backend whose reads answer when the test says. */
function world() {
  const reads: ((l: ContactList) => void)[] = [];
  const b = {
    dashpayContacts: vi.fn(() => new Promise<ContactList>((resolve) => reads.push(resolve))),
    searchUsers: vi.fn(async () => [{ identity: GRACE, username: "grace", displayName: null, avatarUrl: null, relation: "ignored" as const, unverified: false }]),
  };
  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  // The Contacts page's unfiltered list, observed as `ContactsPage` does.
  const observer = new QueryObserver(client, contactListQuery(b, WALLET_A, ALICE, "displayName", ""));
  const stop = observer.subscribe(() => undefined);
  const settle = () => new Promise((r) => setTimeout(r, 0));
  /** The card's line for the ignored hit against the list the page holds now. */
  const card = async () => {
    const hits = await client.fetchQuery(networkSearchQuery(b, WALLET_A, ALICE, "gra"));
    return (listed = observer.getCurrentResult().data) => {
      const relation = currentRelation(candidateOfHit(hits[0]), listedNow(listed));
      return { relation, line: cardLine(relation, { data: "ok", failed: false }) };
    };
  };
  return { b, client, observer, stop, reads, settle, card };
}

describe("the contacts list's read order (D38)", () => {
  let stop = () => undefined as void;
  afterEach(() => stop());

  it("a list read begun before the ignore that lands after the ignored hit keeps the ignore", async () => {
    const w = world();
    stop = w.stop;
    // The page's first read is under way, with nothing cached: it reads the list before the ignore.
    expect(w.b.dashpayContacts).toHaveBeenCalledTimes(1);
    expect(w.observer.getCurrentResult().data).toBeUndefined();
    // The ignore happens; a search then says Grace is ignored.
    const line = await w.card();
    // TanStack's own invalidation hands back the running read (no data, so nothing is cancelled).
    void w.client.invalidateQueries({ queryKey: ["dashpay"] });
    await w.settle();
    expect(w.b.dashpayContacts).toHaveBeenCalledTimes(1);
    // That read now lands, after the hit, without Grace (read before her request came).
    w.reads[0]!(list());
    await w.settle();
    expect(w.observer.getCurrentResult().data?.sections[0]?.contacts).toEqual([]);
    expect(line()).toEqual({ relation: "ignored", line: { text: "You ignored their contact request.", next: "showHidden" } });
  });

  it("the ignore's re-read starts a new list read while the first has no answer yet", async () => {
    const w = world();
    stop = w.stop;
    const line = await w.card();
    refreshAfter(w.client, WALLET_A, ALICE, ["contacts"]);
    await w.settle();
    // A second read began after the action; the first one's answer is dropped.
    expect(w.b.dashpayContacts).toHaveBeenCalledTimes(2);
    w.reads[0]!(list());
    await w.settle();
    expect(w.observer.getCurrentResult().data).toBeUndefined();
    expect(line().relation).toBe("ignored");
    // The new read is after the hit: it decides. Grace is still ignored…
    w.reads[1]!(list(row("ignored")));
    await w.settle();
    expect(line().relation).toBe("ignored");
    // …and a read after an unignore shows her request again.
    void w.observer.refetch();
    await w.settle();
    w.reads[2]!(list(row("pendingIncoming")));
    await w.settle();
    expect(line().relation).toBe("pendingIncoming");
  });

  it("a list that already holds data is read again by invalidation alone, and keeps its rows meanwhile", async () => {
    const w = world();
    stop = w.stop;
    w.reads[0]!(list(row("pendingIncoming")));
    await w.settle();
    refreshAfter(w.client, WALLET_A, ALICE, ["contacts"]);
    await w.settle();
    expect(w.b.dashpayContacts).toHaveBeenCalledTimes(2);
    // Not reset: the page keeps showing the last list while the new one loads.
    expect(w.observer.getCurrentResult().data?.sections[0]?.contacts).toEqual([row("pendingIncoming")]);
  });
});
