import { render, screen } from "@testing-library/react";
import { describe, expect, it } from "vitest";
import { StaleNotice } from "./Controls";

const handle = { status: "error" as const, data: ["kept"], error: "Can't reach Jira.", failure: "offline", loadedAt: Date.now() - 5 * 60_000 };

describe("StaleNotice", () => {
    it("says the data is from before the network went", () => {
        render(<StaleNotice handle={handle} />);
        expect(screen.getByRole("status").textContent).toBe("Offline · showing data from 5m ago");
    });

    it("says nothing while the data is fresh or there is none", () => {
        const { container, rerender } = render(<StaleNotice handle={{ ...handle, status: "ok", failure: undefined }} />);
        expect(container.textContent).toBe("");
        rerender(<StaleNotice handle={{ ...handle, data: undefined }} />);
        expect(container.textContent).toBe("");
    });
});
