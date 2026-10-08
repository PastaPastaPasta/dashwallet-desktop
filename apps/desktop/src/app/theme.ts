import { useEffect } from "react";
import type { ThemeName } from "../api/generated/ThemeName";
import { ui, useUi } from "./ui";

const media = typeof window === "undefined" ? null : window.matchMedia("(prefers-color-scheme: dark)");
if (media) {
  const sync = () => ui.set({ system: media.matches ? "dark" : "light" });
  sync();
  media.addEventListener("change", sync);
}

/** The effective appearance: the forced one, else the system's. */
export function useAppearance(): ThemeName {
  return useUi((s) => s.theme ?? s.system);
}

/** Mirrors a forced appearance into `<html data-theme>` (the token CSS keys off it). Call once. */
export function useThemeAttribute() {
  const forced = useUi((s) => s.theme);
  useEffect(() => {
    if (forced) document.documentElement.dataset.theme = forced;
    else delete document.documentElement.dataset.theme;
  }, [forced]);
}
