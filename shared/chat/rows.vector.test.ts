import { readFileSync } from "node:fs";
import { join } from "node:path";
import { describe, expect, it } from "vitest";
import { applyChanges, emptyChat, fromView } from "./rows";
import type { ChatChange, ChatView } from "./types";

/* Written by the fold's own test from a real chat, so the fold and this applier cannot drift. */
const vector = JSON.parse(readFileSync(join(import.meta.dirname, "../../crates/sikemux-fold/vectors/changes.json"), "utf8")) as {
    steps: ChatChange[][];
    view: ChatView;
};

describe("the fold's changes", () => {
    it("add up to the view the core would send", () => {
        const applied = vector.steps.reduce(applyChanges, emptyChat);
        expect(applied.messages).toEqual(fromView(vector.view).messages);
        expect(applied.running).toEqual(fromView(vector.view).running);
    });
});
