// Static Inter faces, one per weight the type scale uses (UX-SPEC §2.2). Not the variable font:
// WebKitGTK synthesised bold on top of its weight axis, widening bold text by ~1 px per glyph.
import "@fontsource/inter/400.css";
import "@fontsource/inter/500.css";
import "@fontsource/inter/600.css";
import "@fontsource/inter/700.css";
import "./styles/tokens.generated.css";
import "./styles/app.css";

import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { StrictMode } from "react";
import { createRoot } from "react-dom/client";
import type { Backend } from "./api/backend";
import { bridgeEvents } from "./api/events";
import { FixtureBackend, type FixtureVariant } from "./api/fixture";
import { BackendContext } from "./api/queries";
import { inTauri, TauriBackend } from "./api/tauri";
import { App, LaunchContext } from "./app/App";
import { exposeSelftest, runTauriSelftest } from "./app/selftest";
import { milestones } from "./app/startup";
import { ui, type ThemeChoice } from "./app/ui";

/** Browser runs (Playwright, `vite dev`) choose the fixture with `?variant=&theme=&locked=1`. */
function urlOptions() {
  const p = new URLSearchParams(location.search);
  const t = p.get("theme");
  const theme: ThemeChoice = t === "light" || t === "dark" ? t : null;
  return {
    variant: (p.get("variant") ?? "default") as FixtureVariant,
    theme,
    locked: p.get("locked") === "1",
  };
}

async function boot() {
  const opts = urlOptions();
  const tauri = inTauri() ? new TauriBackend() : null;
  const launch = tauri ? await tauri.launchInfo() : FixtureBackend.browserLaunch();
  milestones.launchInfo = performance.now();
  const backend: Backend = tauri && !launch.fixture ? tauri : new FixtureBackend(launch, opts.variant, { locked: opts.locked });
  ui.set({ theme: launch.theme ?? opts.theme });

  const client = new QueryClient({
    // Engine data changes only when a signal says so: no polling, no refetch on focus.
    defaultOptions: { queries: { staleTime: Infinity, refetchOnWindowFocus: false, retry: false } },
  });
  await bridgeEvents(backend, client);
  exposeSelftest(backend, client);

  createRoot(document.getElementById("root")!).render(
    <StrictMode>
      <QueryClientProvider client={client}>
        <BackendContext.Provider value={backend}>
          <LaunchContext.Provider value={launch}>
            <App />
          </LaunchContext.Provider>
        </BackendContext.Provider>
      </QueryClientProvider>
    </StrictMode>,
  );
  if (launch.selftest) void runTauriSelftest(backend, client, launch.selftestPauseMs);
}

boot().catch((e) => {
  const root = document.getElementById("root")!;
  root.textContent = `Dash Wallet could not start: ${e?.message ?? JSON.stringify(e)}`;
  root.className = "boot-error";
});
