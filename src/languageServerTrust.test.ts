import { beforeEach, describe, expect, it, vi } from "vitest";
import { languageServersAllowed } from "./languageServerTrust";
import { getState, setState } from "./state/store";

describe("languageServersAllowed", () => {
    beforeEach(() => setState({ languageServerTrust: {} }));

    it("asks once per project, even when several files open together, and remembers the answer", async () => {
        const ask = vi.fn().mockResolvedValue(true);

        const answers = await Promise.all([languageServersAllowed("/repo", ask), languageServersAllowed("/repo", ask)]);

        expect(answers).toEqual([true, true]);
        expect(ask).toHaveBeenCalledOnce();
        expect(ask.mock.calls[0][0]).toMatchObject({ title: "Start language servers for repo?" });
        expect(getState().languageServerTrust).toEqual({ "/repo": true });
        await expect(languageServersAllowed("/repo", ask)).resolves.toBe(true);
        expect(ask).toHaveBeenCalledOnce();
    });

    it("keeps a refusal without asking again", async () => {
        const ask = vi.fn().mockResolvedValue(false);

        await expect(languageServersAllowed("/untrusted", ask)).resolves.toBe(false);
        await expect(languageServersAllowed("/untrusted", ask)).resolves.toBe(false);

        expect(ask).toHaveBeenCalledOnce();
        expect(getState().languageServerTrust).toEqual({ "/untrusted": false });
    });
});
