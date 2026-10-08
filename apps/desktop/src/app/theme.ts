import { useEffect, useState } from "react";
import { useUi } from "./ui";

export type Appearance = "light" | "dark";

const media = () => window.matchMedia("(prefers-color-scheme: dark)");

/** Applies a forced appearance as `data-theme` on <html> and returns the effective one. */
export function useAppearance(): Appearance {
  const forced = useUi((s) => s.theme);
  const [system, setSystem] = useState<Appearance>(() => (media().matches ? "dark" : "light"));
  useEffect(() => {
    const m = media();
    const onChange = () => setSystem(m.matches ? "dark" : "light");
    m.addEventListener("change", onChange);
    return () => m.removeEventListener("change", onChange);
  }, []);
  useEffect(() => {
    if (forced) document.documentElement.dataset.theme = forced;
    else delete document.documentElement.dataset.theme;
  }, [forced]);
  return forced ?? system;
}
