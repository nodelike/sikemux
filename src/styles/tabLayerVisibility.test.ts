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

describe("tab layer visibility", () => {
    // Only the active tab's `.window-layer` is on screen; the rest are hidden with
    // `visibility: hidden`. A descendant that declares `visibility: visible` undoes
    // that for its own subtree, so the hidden tab paints over the live one.
    it("no stylesheet re-enables visibility inside a hidden layer", () => {
        const offenders = stylesheets(ROOT).flatMap((path) =>
            readFileSync(path, "utf8")
                .split("\n")
                .flatMap((line, index) => (/^\s*visibility:\s*visible\s*(!important)?\s*;/.test(line) ? [`${path}:${index + 1}`] : [])),
        );
        expect(offenders, "hide with `:not(.visible) { visibility: hidden }` instead").toEqual([]);
    });
});
