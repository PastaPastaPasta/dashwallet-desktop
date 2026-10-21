// Masternodes ▸ Username voting (MG-11, UX-SPEC §4.25.4 over MG-07's `contested_names`): the
// contested names with search and sort, the selected one's tally, end time and the wallet's
// voters, and Vote. The vote sheet lives here, so a list that re-reads or re-sorts under it keeps
// it (MG-09 D31). The D-rows are docs/design/MG-11-contested-voting-ui.md's.

import { useState } from "react";
import { errorMessage } from "../../api/backend";
import type { ContestedName } from "../../api/generated/ContestedName";
import type { ContestedNames } from "../../api/generated/ContestedNames";
import type { NetworkInfo } from "../../api/generated/NetworkInfo";
import type { WalletSummary } from "../../api/generated/WalletSummary";
import { DashButton, Icon } from "../../components/basics";
import { EmptyState, LoadingState } from "../../components/history";
import { copyWithToast } from "../../components/toast";
import { nodeName } from "../../governance/model";
import { contestRow, endsText, isClosed, listContests, NAME_SORTS, nameErrorText, NameText, tallyView, voteBlocked, voterLine, type NameSort } from "../../governance/names";
import { useContestedNames } from "../../governance/queries";
import { Notice, StatusChip, useNowSecs } from "./common";
import { NameVoteSheet } from "./NameVoteSheet";

export function NamesTab({ wallet, network }: { wallet: WalletSummary; network: NetworkInfo }) {
  const names = useContestedNames(wallet.walletId);
  const [search, setSearch] = useState("");
  const [sort, setSort] = useState<NameSort>("ending");
  const [selected, setSelected] = useState<string | null>(null);
  /** The contest the sheet is open on, with the read it was opened from (D13). */
  const [voteOn, setVoteOn] = useState<{
    contest: ContestedName;
    names: ContestedNames;
  } | null>(null);
  const now = useNowSecs();

  let body;
  if (names.isPending) body = <LoadingState testId="names-loading" text={NameText.loading} />;
  else if (names.isError && !names.data)
    body = (
      <EmptyState
        testId="names-error"
        title={NameText.failed}
        message={nameErrorText(names.error) ?? errorMessage(names.error)}
        action={
          <DashButton variant="filledBlue" size="small" data-testid="names-retry" disabled={names.isFetching} onClick={() => void names.refetch()}>
            {NameText.tryAgain}
          </DashButton>
        }
      />
    );
  else {
    const data = names.data!;
    const listed = listContests(data.contests, search, sort);
    const current = listed.find((c) => c.label === selected) ?? listed[0] ?? null;
    body = (
      <>
        {wallet.watchOnly ? (
          <Notice tone="info" testId="names-notice-watch-only">
            <strong>{NameText.watchOnly}</strong> {NameText.watchOnlyText}
          </Notice>
        ) : (
          data.voters.length === 0 && (
            <Notice tone="info" testId="names-browsing-only">
              <strong>{NameText.browsingOnly}</strong> {NameText.browsingOnlyText}
            </Notice>
          )
        )}
        {data.contests.length === 0 ? (
          <EmptyState testId="names-empty" title={NameText.empty} message={NameText.emptyHelp} />
        ) : (
          <>
            <div className="tx-filter-bar mg-toolbar">
              <label className="search-field" data-testid="names-search">
                <Icon id="searchMagnifyingGlass" size={16} className="search-icon" />
                <input
                  type="search"
                  value={search}
                  placeholder={NameText.search}
                  aria-label={NameText.search}
                  onChange={(e) => setSearch(e.target.value)}
                  onKeyDown={(e) => {
                    if (e.key === "Escape" && search !== "") {
                      e.preventDefault();
                      setSearch("");
                    }
                  }}
                />
              </label>
              <label className="filter-select">
                <span>Sort by</span>
                <select data-testid="names-sort" value={sort} onChange={(e) => setSort(e.target.value as NameSort)}>
                  {NAME_SORTS.map((s) => (
                    <option key={s.id} value={s.id}>
                      {s.title}
                    </option>
                  ))}
                </select>
              </label>
            </div>
            {listed.length === 0 ? (
              <EmptyState testId="names-no-match" title={`No username matches “${search.trim()}”`} message={NameText.normalized} />
            ) : (
              <div className="contacts-split">
                <div className="contacts-list">
                  <section className="tx-group card-l1">
                    <ul className="mg-list" data-testid="names-list">
                      {listed.map((c) => (
                        <li key={c.label}>
                          <ContestRowButton contest={c} names={data} now={now} current={c.label === current?.label} onSelect={() => setSelected(c.label)} />
                        </li>
                      ))}
                    </ul>
                  </section>
                </div>
                {current && (
                  <NamePane
                    contest={current}
                    names={data}
                    watchOnly={wallet.watchOnly}
                    now={now}
                    onVote={() => {
                      setSelected(current.label);
                      setVoteOn({ contest: current, names: data });
                    }}
                  />
                )}
              </div>
            )}
          </>
        )}
      </>
    );
  }

  return (
    <>
      <p className="mg-muted mg-small mg-names-intro" data-testid="names-intro">
        {NameText.intro}
      </p>
      {body}
      {voteOn && (
        <NameVoteSheet
          contest={voteOn.contest}
          names={voteOn.names}
          wallet={wallet}
          network={network}
          onClose={() => setVoteOn(null)}
          reread={() => names.refetch().then((r) => r.data)}
        />
      )}
    </>
  );
}

function ContestRowButton({ contest, names, now, current, onSelect }: { contest: ContestedName; names: ContestedNames; now: number; current: boolean; onSelect: () => void }) {
  const row = contestRow(contest, names.voters, now);
  return (
    <button type="button" className="mg-row mg-row-wide" aria-current={current ? "true" : undefined} data-testid={`name-row-${row.label}`} onClick={onSelect}>
      <span className="mg-row-text">
        <span className="mg-row-title">{row.label}</span>
        <span className="mg-row-sub">
          {row.sub}
          {row.you && ` · ${row.you}`}
        </span>
      </span>
      <span className="mg-row-trailing">
        {row.chip && <StatusChip chip={row.chip} />}
        <span className="mg-row-meta">{row.total === 1 ? "1 vote" : `${row.total.toLocaleString("en-US")} votes`}</span>
      </span>
    </button>
  );
}

/** The selected contest (D5–D9): when voting ends, the tally with who leads, the wallet's voters
 * with their present choice and votes left, and Vote. */
function NamePane({ contest, names, watchOnly, now, onVote }: { contest: ContestedName; names: ContestedNames; watchOnly: boolean; now: number; onVote: () => void }) {
  const tally = tallyView(contest);
  const row = contestRow(contest, names.voters, now);
  const closed = isClosed(contest, now);
  const blocked = voteBlocked(contest, names, watchOnly, now);
  return (
    <aside className="contacts-pane" aria-label="Username vote" data-testid="name-pane">
      <section className="mg-pane card-l1">
        <div>
          <h2 className="mg-pane-title">{contest.label}</h2>
          <div className="chip-row mg-pane-chip">
            {row.chip && <StatusChip chip={row.chip} testId="name-pane-chip" />}
            <span className="mg-muted mg-small" data-testid="name-pane-ends">
              {endsText(contest, now)}
            </span>
          </div>
        </div>
        {closed && (
          <Notice tone="info" testId="name-closed">
            Voting has ended. Platform gives the name out by these votes in its next block, and then it leaves this list.
          </Notice>
        )}
        <div className="mg-stack">
          <div className="mg-spread mg-small">
            <strong>Votes, weighted</strong>
            <span className="mg-muted" data-testid="name-pane-lead">
              {tally.lead}
            </span>
          </div>
          <ul className="mg-tally" data-testid="name-tally">
            {tally.rows.map((r) => (
              <li key={r.key} className="mg-tally-row" data-testid={`name-tally-${r.key}`}>
                <span className="mg-tally-title">
                  {r.identity ? (
                    <span className="copy-value">
                      <span className="mono" title={r.identity}>
                        {r.title}
                      </span>
                      <button type="button" className="copy-button" aria-label={`Copy identity ${r.title}`} title="Copy identity" onClick={() => void copyWithToast(r.identity!)}>
                        <Icon id="copy" size={16} />
                      </button>
                    </span>
                  ) : (
                    r.title
                  )}
                  {r.leading && <span className="chip chip-success">{closed ? "Most votes" : "Leading"}</span>}
                </span>
                <span className="mg-tally-votes">{r.votes.toLocaleString("en-US")}</span>
                <span className="mg-tally-bar" aria-hidden="true">
                  <span className={r.key === "lock" ? "mg-tally-lock" : r.key === "abstain" ? "mg-tally-abstain" : "mg-tally-fill"} style={{ width: `${r.pct}%` }} />
                </span>
              </li>
            ))}
          </ul>
          {!contest.contendersComplete && (
            <p className="mg-muted mg-small mg-flush" data-testid="name-partial">
              {NameText.partial}
            </p>
          )}
          <p className="mg-muted mg-small mg-flush">{NameText.counts}</p>
        </div>
        {names.voters.length > 0 && (
          <div>
            <h3 className="mg-section-title">Your masternodes</h3>
            <ul className="mg-roles mg-roles-votes" data-testid="name-pane-masternodes">
              {names.voters.map((v) => (
                <li key={v.proTxHash} className="mg-roles-pair">
                  <span>
                    {nodeName(v)}
                    {v.weight > 1 && <span className="mg-muted"> · weight {v.weight}</span>}
                  </span>
                  <span className="mg-muted">{voterLine(v, contest.label, names.votesAllowed)}</span>
                </li>
              ))}
            </ul>
            <p className="mg-muted mg-small mg-flush">{NameText.limitNote(names.votesAllowed)}</p>
          </div>
        )}
        <div className="mg-actions">
          <DashButton variant="filledBlue" size="small" data-testid="name-vote" disabled={blocked !== null} title={blocked ?? undefined} onClick={onVote}>
            Vote
          </DashButton>
        </div>
        {blocked && (
          <p className="mg-muted mg-small mg-flush" data-testid="name-vote-blocked">
            {blocked}
          </p>
        )}
      </section>
    </aside>
  );
}
