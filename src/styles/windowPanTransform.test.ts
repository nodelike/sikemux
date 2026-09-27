import { readdirSync, readFileSync } from "node:fs";
import { join } from "node:path";
import { describe, expect, it } from "vitest";

const ROOT = join(process.cwd(), "src");

function stylesheets(dir: string): string[] {
    return readdirSync(dir, { withFileTypes: true }).flatMap((entry) => {
        const path = join(dir, entry.name);
        if (entry.isDirectory()) return entry.name === "node_modules" ? [] : stylesheets(path);
        return entry.isFile() && entry.name.endsWith(".css") ? [path] : [];
    });
}

function rules(css: string): { selector: string; body: string }[] {
    const bare = css.replace(/\/\*[\s\S]*?\*\//g, "");
    return Array.from(bare.matchAll(/([^{}]+)\{([^{}]*)\}/g), (match) => ({ selector: match[1].trim(), body: match[2] }));
}

describe("window pan transform", () => {
    // A transformed box becomes the containing block for `position: fixed`
    // children, and panes put dropdown scrims and drag ghosts on screen that way.
    // The track and its screens may only be offset while a slide is running.
    it("only a panning track and its layers carry a transform", () => {
        const offenders = stylesheets(ROOT).flatMap((path) =>
            rules(readFileSync(path, "utf8"))
                .filter((rule) => /\.window-(track|layer)\b/.test(rule.selector) && !rule.selector.includes(".panning"))
                .filter((rule) => /(^|[\s;])transform:/.test(rule.body))
                .map((rule) => `${path}: ${rule.selector}`),
        );
        expect(offenders, "put the transform under `.window-track.panning` instead").toEqual([]);
    });

    // A hidden box can still be scrolled by focus, and the stage and its screens
    // have nothing that would ever scroll them back.
    it("the stage and its screens clip instead of hiding their overflow", () => {
        const offenders = stylesheets(ROOT).flatMap((path) =>
            rules(readFileSync(path, "utf8"))
                .filter((rule) => rule.selector.split(",").some((part) => /(\.stage|\.window-area|\.window-layer)\s*$/.test(part.trim())))
                .filter((rule) => /(^|[\s;])overflow(-x)?:\s*(hidden|auto|scroll)/.test(rule.body))
                .map((rule) => `${path}: ${rule.selector}`),
        );
        expect(offenders, "use `overflow: clip`").toEqual([]);
    });
});
