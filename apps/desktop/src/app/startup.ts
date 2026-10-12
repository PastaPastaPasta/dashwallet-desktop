// Start-up milestones (U5). `shell` is the first frame of the window (splash or shell);
// `first-paint` is the first Overview frame that shows the balance and history. Times are ms
// since the webview's navigation start; under Tauri dw-app also logs ms since `main`.

import { inTauri, tauriTools } from "../api/tauri";

export const milestones: Record<string, number> = { scriptStart: performance.now() };
const marks = new Map<string, { promise: Promise<number>; resolve: (ms: number) => void }>();

function entry(name: string) {
  let e = marks.get(name);
  if (!e) {
    let resolve!: (ms: number) => void;
    const promise = new Promise<number>((r) => (resolve = r));
    e = { promise, resolve };
    marks.set(name, e);
  }
  return e;
}

/** Records `name` once, two frames after the call (so the frame is on screen). */
export function mark(name: string) {
  if (name in milestones) return;
  milestones[name] = -1;
  requestAnimationFrame(() =>
    requestAnimationFrame(() => {
      milestones[name] = performance.now();
      const { resolve } = entry(name);
      if (inTauri()) void tauriTools.startupMark(name).then(resolve);
      else resolve(milestones[name]);
    }),
  );
}

/** Resolves with the milestone's time since `main` (Tauri) or navigation start (browser). */
export const whenMarked = (name: string) => entry(name).promise;
