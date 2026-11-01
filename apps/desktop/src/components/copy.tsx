// Copy buttons that answer on the button itself (DEC-1058 #3): a check mark and "Copied" for
// about 1.5 s after the copy worked. The toast stays (copyWithToast).

import { useCallback, useEffect, useRef, useState, type ComponentProps, type ReactNode } from "react";
import { DashButton, Icon } from "./basics";
import { copyWithToast } from "./toast";

/** How long the button says "Copied". */
export const COPIED_MS = 1500;

/** `copied` is true for `COPIED_MS` after `copy(text)` worked; a second copy restarts it. */
export function useCopy(ms: number = COPIED_MS): { copied: boolean; copy: (text: string) => Promise<void> } {
  const [copied, setCopied] = useState(false);
  const timer = useRef<number | undefined>(undefined);
  useEffect(() => () => window.clearTimeout(timer.current), []);
  const copy = useCallback(
    async (text: string) => {
      const ok = await copyWithToast(text);
      window.clearTimeout(timer.current);
      setCopied(ok);
      if (ok) timer.current = window.setTimeout(() => setCopied(false), ms);
    },
    [ms],
  );
  return { copied, copy };
}

/** The label of a copy button while it says "Copied". */
export function CopiedLabel() {
  return (
    <span className="copied-label" data-testid="copied">
      <span aria-hidden="true">✓</span> Copied
    </span>
  );
}

/** A text button (DashButton) that copies `text`. `children` is its label; "Copied" replaces it for a moment. */
export function CopyDashButton({ text, children, ...rest }: { text: string | null; children: ReactNode } & Omit<ComponentProps<typeof DashButton>, "onClick" | "children">) {
  const { copied, copy } = useCopy();
  return (
    <DashButton {...rest} onClick={() => text !== null && void copy(text)} data-copied={copied || undefined}>
      {copied ? <CopiedLabel /> : children}
    </DashButton>
  );
}

/** The small icon button beside a value; "Copied" shows beside it for a moment. */
export function CopyIconButton({ text, label, title, size = 20 }: { text: string; label: string; title?: string; size?: number }) {
  const { copied, copy } = useCopy();
  return (
    <button type="button" className="copy-button" aria-label={copied ? "Copied" : label} title={title ?? label} data-copied={copied || undefined} onClick={() => void copy(text)}>
      {copied ? <CopiedLabel /> : <Icon id="copy" size={size} />}
    </button>
  );
}
