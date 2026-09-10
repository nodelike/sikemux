import { describe, expect, it } from "vitest";
import { THEMES } from "../themes";
import { applyTheme } from "../themes/bus";
import { contrastRatio, readableColor } from "./themeContrast";

describe("informative theme colors", () => {
    for (const theme of THEMES)
        it(`keeps secondary information legible in ${theme.name}`, () => {
            const { bg, bgDim, bgRaised, inkDim, ink } = theme.chrome;
            const backgrounds = [bg, bgDim, bgRaised];
            applyTheme(theme.id);
            for (const token of [
                "--ink-muted",
                "--text-tertiary",
                "--danger",
                "--git-modified",
                "--git-untracked",
                "--git-added",
                "--git-deleted",
                "--git-renamed",
            ]) {
                const color = document.documentElement.style.getPropertyValue(token);
                for (const background of backgrounds) expect(contrastRatio(color, background), `${theme.name} ${token}`).toBeGreaterThanOrEqual(4.5);
            }
            const text = readableColor(inkDim, backgrounds, ink);
            for (const background of backgrounds) expect(contrastRatio(text, background)).toBeGreaterThanOrEqual(4.5);
            for (const tone of [theme.dark ? "#78dca1" : "#247345", theme.dark ? "#e9ba6c" : "#875508"])
                for (const background of backgrounds)
                    expect(contrastRatio(readableColor(tone, backgrounds, ink), background)).toBeGreaterThanOrEqual(4.5);
        });
});
