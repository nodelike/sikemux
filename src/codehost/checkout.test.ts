import { describe, expect, it } from "vitest";
import { isOwnBranch, localBranchOf } from "./checkout";

const repo = { provider: "test.host", owner: "nodelike", name: "sikemux" };

describe("checking out a pull request", () => {
    it("uses the branch itself when it lives on the repository", () => {
        const pull = { number: 7, head: "feat/x", headLabel: "nodelike:feat/x" };
        expect(isOwnBranch(pull, repo)).toBe(true);
        expect(localBranchOf(pull, repo)).toBe("feat/x");
    });

    it("gives a fork's pull request a branch of its own, named after its number", () => {
        const pull = { number: 49, head: "feat/github-actions-plugin", headLabel: "Sujal85526:feat/github-actions-plugin" };
        expect(isOwnBranch(pull, repo)).toBe(false);
        expect(localBranchOf(pull, repo)).toBe("pr-49");
    });

    it("takes a branch whose owner the host did not say as the repository's own", () => {
        expect(isOwnBranch({ head: "feat/x", headLabel: null }, repo)).toBe(true);
        expect(isOwnBranch({ head: null, headLabel: "nodelike:feat/x" }, repo)).toBe(false);
    });
});
