/// <reference types="vite/client" />

/** True in fixture builds (`vite dev`, `vite build --mode fixture`, vitest): the fixture
 * backend (src/fixture) is bundled. False in `vite build`, the app's production renderer. */
declare const __DW_FIXTURES__: boolean;

interface Window {
  /** The installed fixture backend, for Playwright (fixture builds only). */
  __dwFixture?: import("./fixture").FixtureHandle;
}
