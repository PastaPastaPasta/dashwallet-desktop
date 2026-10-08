// Start-up milestones (U5). `shell` is the first frame of the window (splash or shell);
// `first-paint` is the first Overview frame that shows the balance and history. Times are ms
// since the webview's navigation start; under Tauri dw-app also logs ms since `main`.

import { inTauri, tauriTools } from "../api/tauri";

export const milestones: Record<string, number> = { scriptStart: performance.now() };
const waiting = new Map<string, (ms: number) => void>();
const reached = new Map<string, Promise<number>>();

function deferred(name: string): Promise<number> {
  let p = reached.get(name);
  if (!p) {
    p = new Promise<number>((resolve) => waiting.set(name, resolve));
    reached.set(name, p);
  }
  return p;
}

/** Records `name` once, two frames after the call (so the frame is on screen). */
export function mark(name: string) {
  if (name in milestones) return;
  milestones[name] = -1;
  requestAnimationFrame(() =>
    requestAnimationFrame(() => {
      milestones[name] = performance.now();
      deferred(name);
      const done = waiting.get(name)!;
      if (inTauri()) void tauriTools.startupMark(name).then(done);
      else done(milestones[name]);
    }),
  );
}

/** Resolves with the milestone's time since `main` (Tauri) or navigation start (browser). */
export const whenMarked = (name: string) => deferred(name);
