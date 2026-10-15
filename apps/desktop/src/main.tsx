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
import { bridgeEvents } from "./api/events";
import { BackendContext } from "./api/queries";
import { inTauri, IpcBackend } from "./api/tauri";
import { App, LaunchContext } from "./app/App";
import { installDropGuard } from "./app/dropGuard";
import { exposeSelftest, runTauriSelftest } from "./app/selftest";
import { milestones } from "./app/startup";
import { loadPrefs } from "./settings/prefs";

/** Inside Tauri: dw-app and the engine. In a fixture build (`vite dev`, `pnpm build:fixture`)
 * outside Tauri: the fixture backend, chosen with `?scenario=&theme=&fail=&delay=`. A
 * production build has no fixture code at all (`__DW_FIXTURES__` is false, so Vite drops the
 * import), and outside Tauri it only says where it runs. */
async function connect(): Promise<{ backend: IpcBackend; fixturePassphrase: string | null }> {
  if (inTauri()) return { backend: new IpcBackend("tauri"), fixturePassphrase: null };
  if (__DW_FIXTURES__) {
    const { installFixture, fixtureOptionsFromUrl } = await import("./fixture");
    const fixture = installFixture(fixtureOptionsFromUrl(location.search));
    window.__dwFixture = fixture;
    return { backend: new IpcBackend("fixture", () => fixture.now, fixture.locale), fixturePassphrase: fixture.world.passphrase };
  }
  throw new Error("this page runs only inside the Dash Wallet app");
}

async function boot() {
  const { backend, fixturePassphrase } = await connect();
  const launch = await backend.launchInfo();
  milestones.launchInfo = performance.now();
  await loadPrefs(backend, launch);

  const client = new QueryClient({
    // Engine data changes only when a signal says so: no polling, no refetch on focus.
    defaultOptions: { queries: { staleTime: Infinity, refetchOnWindowFocus: false, retry: false } },
  });
  await bridgeEvents(backend, client);
  if (fixturePassphrase !== null) exposeSelftest(backend, client, fixturePassphrase);

  installDropGuard();
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
  if (launch.selftest) void runTauriSelftest(backend, client, launch.selftestPauseMs, launch.selftestUnlock);
}

boot().catch((e) => {
  const root = document.getElementById("root")!;
  root.textContent = `Dash Wallet could not start: ${e?.message ?? JSON.stringify(e)}`;
  root.className = "boot-error";
});
