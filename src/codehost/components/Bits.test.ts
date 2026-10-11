import { describe, expect, it } from "vitest";
import { reviewVerdict } from "./PullsView";
import { latestOf } from "./ReleasesView";
import type { Release } from "../api";

describe("reviewVerdict", () => {
    const review = (author: string, state: string) => ({ author, state });

    it("says nothing until somebody has reviewed", () => {
        expect(reviewVerdict([])).toBeNull();
        expect(reviewVerdict([review("a", "COMMENTED")])).toBeNull();
    });

    it("reports an approval", () => {
        expect(reviewVerdict([review("a", "APPROVED")])).toBe("Approved");
    });

    it("lets requested changes outweigh an approval", () => {
        expect(reviewVerdict([review("a", "APPROVED"), review("b", "CHANGES_REQUESTED")])).toBe("Changes requested");
    });

    it("counts only a person's latest review", () => {
        expect(reviewVerdict([review("a", "CHANGES_REQUESTED"), review("a", "APPROVED")])).toBe("Approved");
    });

    it("ignores a review that was dismissed", () => {
        expect(reviewVerdict([review("a", "APPROVED"), review("b", "DISMISSED")])).toBe("Approved");
    });
});

describe("latestOf", () => {
    const release = (id: number, draft: boolean, prerelease: boolean): Release => ({
        id,
        tag: `v${id}`,
        name: `v${id}`,
        body: "",
        draft,
        prerelease,
        publishedAt: null,
        author: null,
        assets: [],
        url: "",
    });

    it("marks the newest one that is neither a draft nor a pre-release", () => {
        expect(latestOf([release(3, false, true), release(2, false, false), release(1, false, false)])).toBe(2);
    });

    it("skips drafts", () => {
        expect(latestOf([release(3, true, false), release(2, false, false)])).toBe(2);
    });

    it("marks nothing when every release is a pre-release", () => {
        expect(latestOf([release(2, false, true), release(1, true, false)])).toBeNull();
        expect(latestOf([])).toBeNull();
    });
});
