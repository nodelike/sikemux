import { useEffect, useState } from "react";
import { coreCall } from "../api/core";
import { onCoreChanged } from "../api/coreChanged";
import { swallow } from "../state/toast";
import type { Palette, Theme, ThemeSummary } from ".";

const NONE: readonly ThemeSummary[] = [];

/** Every theme to pick from, as the core lists them: the person's own, then Sikemux's, then Ghostty's. */
export function useThemeSummaries(): readonly ThemeSummary[] {
    const [summaries, setSummaries] = useState(NONE);
    useEffect(() => {
        let live = true;
        const read = () =>
            coreCall<ThemeSummary[]>("themes")
                .then((next) => {
                    if (live) setSummaries(next);
                })
                .catch(swallow("list the themes"));
        const stop = onCoreChanged("preferences", () => void read());
        void read();
        return () => {
            live = false;
            stop();
        };
    }, []);
    return summaries;
}

/** A theme in full, such as one to start a custom theme from. */
export async function themeById(id: string): Promise<Theme> {
    const palette = await coreCall<Palette>("theme", { id });
    return palette.theme;
}
