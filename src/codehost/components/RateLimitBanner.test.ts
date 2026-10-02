import { act, cleanup, render, screen } from "@testing-library/react";
import { createElement } from "react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { invalidate } from "../../plugin-api/resources";
import { AccountProvider } from "../registry";
import { InHost, registerTestHost } from "../testHost";
import type { RateLimit } from "../types";
import { RateLimitBanner, rateLimitNote, waitInWords } from "./RateLimitBanner";

const api = { rateLimit: vi.fn() };
const host = registerTestHost(api);

const plenty: RateLimit = { limited: false, resetsAt: null, remaining: 4800, limit: 5000, near: false };
const now = Date.UTC(2026, 8, 29, 10, 0, 0);

describe("rateLimitNote", () => {
    it("says nothing while there is plenty left", () => {
        expect(rateLimitNote("GitHub", plenty, now)).toBeNull();
    });

    it("says how long requests are held back once the limit is spent", () => {
        const spent = { ...plenty, limited: true, remaining: 0, resetsAt: now / 1000 + 12 * 60 };
        const note = rateLimitNote("GitHub", spent, now);
        expect(note?.tone).toBe("danger");
        expect(note?.text).toMatch(/^GitHub's rate limit is used up\. Sikemux is holding requests until .+, in 12 min\.$/);
    });

    it("counts what is left when the host says", () => {
        const low = { ...plenty, remaining: 120, near: true };
        expect(rateLimitNote("GitHub", low, now)).toEqual({ tone: "warn", text: "120 of 5000 GitHub requests left this hour." });
    });

    it("says when the hour's count starts over, when the host says", () => {
        const low = { ...plenty, remaining: 120, near: true, resetsAt: now / 1000 + 600 };
        expect(rateLimitNote("GitHub", low, now)?.text).toMatch(/^120 of 5000 GitHub requests left this hour, until .+\.$/);
    });

    it("does not claim requests are held when it cannot say until when", () => {
        const spent = { ...plenty, limited: true, remaining: 0, near: true };
        expect(rateLimitNote("GitHub", spent, now)?.tone).toBe("warn");
    });

    it("warns without numbers when the host only says it is close", () => {
        const close = { limited: false, resetsAt: null, remaining: null, limit: null, near: true };
        expect(rateLimitNote("Bitbucket", close, now)).toEqual({ tone: "warn", text: "Close to Bitbucket's hourly request limit." });
    });
});

describe("waitInWords", () => {
    it("rounds up to what a person would wait", () => {
        expect(waitInWords(400)).toBe("1s");
        expect(waitInWords(45_000)).toBe("45s");
        expect(waitInWords(61_000)).toBe("2 min");
        expect(waitInWords(125 * 60_000)).toBe("2 h 5 min");
    });
});

describe("RateLimitBanner", () => {
    async function renderBanner(budget: RateLimit) {
        api.rateLimit.mockResolvedValue(budget);
        const banner = createElement(RateLimitBanner, { active: true });
        const view = render(createElement(InHost, { host, children: createElement(AccountProvider, { value: "work" }, banner) }));
        await act(async () => {});
        return view;
    }

    beforeEach(() => {
        invalidate(() => true);
        api.rateLimit.mockReset();
    });

    afterEach(() => {
        cleanup();
        vi.useRealTimers();
    });

    it("stays out of the way while there is plenty left", async () => {
        const view = await renderBanner(plenty);
        expect(api.rateLimit).toHaveBeenCalledWith("work");
        expect(view.container.textContent).toBe("");
    });

    it("warns when little is left", async () => {
        await renderBanner({ ...plenty, remaining: 90, near: true });
        const banner = screen.getByRole("status");
        expect(banner.dataset.tone).toBe("warn");
        expect(banner.textContent).toBe("90 of 5000 Test host requests left this hour.");
    });

    it("reads everything again a second after the limit resets", async () => {
        vi.useFakeTimers({ toFake: ["setTimeout", "clearTimeout"] });
        await renderBanner({ ...plenty, limited: true, remaining: 0, resetsAt: Math.floor(Date.now() / 1000) + 2 });
        expect(screen.getByRole("status").dataset.tone).toBe("danger");
        api.rateLimit.mockResolvedValue(plenty);
        await act(async () => {
            vi.advanceTimersByTime(1_000);
        });
        expect(api.rateLimit).toHaveBeenCalledTimes(1);
        await act(async () => {
            vi.advanceTimersByTime(3_000);
        });
        expect(api.rateLimit).toHaveBeenCalledTimes(2);
        expect(screen.queryByRole("status")).toBeNull();
    });
});
