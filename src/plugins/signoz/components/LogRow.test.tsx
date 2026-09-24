import { fireEvent, render, screen } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";
import type { LogLine } from "../api";
import { LogRow, severityTone } from "./LogRow";
import { formatMs } from "./TraceView";

const line: LogLine = {
    id: "a",
    timestamp: "2026-09-24T09:11:54.569Z",
    service: "api-gateway",
    severity: "ERROR",
    body: "Request completed with server error",
    traceId: "16db92378d1e0e36039248e129d148f5",
    spanId: null,
    attributes: { error: "upstream timed out\n  at handler", status: 503, empty: "" },
    resources: {},
};

describe("LogRow", () => {
    it("shows every attribute, whole, once opened, and offers the trace", () => {
        const onOpenTrace = vi.fn();
        const { rerender } = render(<LogRow line={line} expanded={false} onToggle={() => {}} onOpenTrace={onOpenTrace} />);
        expect(screen.queryByText("status")).toBeNull();

        rerender(<LogRow line={line} expanded onToggle={() => {}} onOpenTrace={onOpenTrace} />);
        expect(screen.getByText("status")).toBeTruthy();
        expect(screen.getByText("503")).toBeTruthy();
        expect(screen.getByText(/upstream timed out/).textContent).toContain("at handler");
        expect(screen.queryByText("empty")).toBeNull();
        fireEvent.click(screen.getByText(`trace ${line.traceId}`));
        expect(onOpenTrace).toHaveBeenCalledWith(line.traceId);
    });

    it("colours only what needs attention", () => {
        expect(severityTone("ERROR")).toBe("danger");
        expect(severityTone("fatal")).toBe("danger");
        expect(severityTone("WARN")).toBe("warn");
        expect(severityTone("INFO")).toBe("quiet");
        expect(severityTone(null)).toBe("quiet");
    });

    it("writes durations at a sensible unit", () => {
        expect(formatMs(0.25)).toBe("250µs");
        expect(formatMs(12.34)).toBe("12.3ms");
        expect(formatMs(1_530)).toBe("1.5s");
        expect(formatMs(42_000)).toBe("42s");
    });
});
