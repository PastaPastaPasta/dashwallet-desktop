// Pieces the Governance screens share (MG-09): the clock that re-reads times, status chips, the
// weighted vote bar and a More button's menu.

import { useEffect, useId, useRef, useState, type ReactNode } from "react";
import type { VoteTally } from "../../api/generated/VoteTally";
import { DashButton } from "../../components/basics";
import { voteBar, type ChipView } from "../../governance/model";

const TICK_MS = 30_000;

/** Seconds since the epoch, re-read every 30 s: the clock card's "left" counts down with it. */
export function useNowSecs(): number {
  const [now, setNow] = useState(() => Math.floor(Date.now() / 1000));
  useEffect(() => {
    const t = setInterval(() => setNow(Math.floor(Date.now() / 1000)), TICK_MS);
    return () => clearInterval(t);
  }, []);
  return now;
}

export function StatusChip({ chip, testId }: { chip: ChipView; testId?: string }) {
  return (
    <span className={`chip chip-${chip.tone}`} title={chip.tooltip || undefined} data-testid={testId}>
      {chip.text}
    </span>
  );
}

/** Yes / no / abstain by weight with the funding line; the text beside it says the same. */
export function VoteBar({ tally, threshold, weight }: { tally: VoteTally; threshold: number; weight: number }) {
  const b = voteBar(tally, threshold, weight);
  return (
    <span className="mg-votebar" aria-hidden="true">
      <span className="mg-vote-yes" style={{ width: `${b.yes}%` }} />
      <span className="mg-vote-no" style={{ width: `${b.no}%` }} />
      <span className="mg-vote-abstain" style={{ width: `${b.abstain}%` }} />
      <span className="mg-threshold" style={{ left: `${b.line}%` }} />
    </span>
  );
}

export interface MenuItem {
  id: string;
  title: string;
  onSelect: () => void;
}

/** "More": a menu button whose items close it; Escape and a click elsewhere close it too. */
export function MoreMenu({ items, testId, label = "More" }: { items: MenuItem[]; testId: string; label?: string }) {
  const [open, setOpen] = useState(false);
  const anchor = useRef<HTMLSpanElement>(null);
  const button = useRef<HTMLButtonElement>(null);
  const id = useId();
  useEffect(() => {
    if (!open) return;
    const away = (e: PointerEvent) => {
      if (!anchor.current?.contains(e.target as Node)) setOpen(false);
    };
    document.addEventListener("pointerdown", away);
    anchor.current?.querySelector<HTMLButtonElement>('[role="menuitem"]')?.focus();
    return () => document.removeEventListener("pointerdown", away);
  }, [open]);
  const close = () => {
    setOpen(false);
    button.current?.focus();
  };
  const move = (e: React.KeyboardEvent) => {
    const all = [...(anchor.current?.querySelectorAll<HTMLButtonElement>('[role="menuitem"]') ?? [])];
    const at = all.indexOf(document.activeElement as HTMLButtonElement);
    if (e.key === "Escape") {
      e.preventDefault();
      e.stopPropagation();
      close();
    } else if (e.key === "ArrowDown" || e.key === "ArrowUp") {
      e.preventDefault();
      all[(at + (e.key === "ArrowDown" ? 1 : all.length - 1)) % all.length]?.focus();
    } else if (e.key === "Tab") setOpen(false);
  };
  return (
    <span className="mg-menu-anchor" ref={anchor}>
      <DashButton
        ref={button}
        variant="tintedGray"
        size="small"
        aria-haspopup="menu"
        aria-expanded={open}
        aria-controls={open ? id : undefined}
        data-testid={testId}
        onClick={() => setOpen((o) => !o)}
      >
        {label}
      </DashButton>
      {open && (
        <ul className="mg-menu" role="menu" id={id} aria-label={label} onKeyDown={move}>
          {items.map((item) => (
            <li key={item.id} role="none">
              <button
                type="button"
                role="menuitem"
                data-testid={`${testId}-${item.id}`}
                onClick={() => {
                  close();
                  item.onSelect();
                }}
              >
                {item.title}
              </button>
            </li>
          ))}
        </ul>
      )}
    </span>
  );
}

/** An info, warning or danger notice with an optional action at its end. */
export function Notice({ tone, testId, children, action }: { tone: "info" | "warning" | "danger"; testId: string; children: ReactNode; action?: ReactNode }) {
  return (
    <div className={`mg-notice mg-notice-${tone} ${action ? "mg-notice-row" : ""}`} role={tone === "info" ? undefined : "status"} data-testid={testId}>
      <span>{children}</span>
      {action}
    </div>
  );
}
