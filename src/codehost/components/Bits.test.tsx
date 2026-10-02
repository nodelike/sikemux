import { cleanup, render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, describe, expect, it, vi } from "vitest";

const shell = vi.hoisted(() => ({ openUrl: vi.fn(() => Promise.resolve()) }));
vi.mock("../../plugin-api/host", async (importOriginal) => ({
    ...(await importOriginal<typeof import("../../plugin-api/host")>()),
    openUrl: shell.openUrl,
}));

import { InHost, registerTestHost } from "../testHost";
import { Comments, Labels, PageHead, StateMark, stateLabel, Who } from "./Bits";

const host = registerTestHost({ image: () => new Promise(() => {}) });

afterEach(cleanup);

describe("stateLabel", () => {
    it("names a pull request's states, a draft among them", () => {
        expect(stateLabel("pull", "open")).toBe("Open");
        expect(stateLabel("pull", "open", true)).toBe("Draft");
        expect(stateLabel("pull", "merged")).toBe("Merged");
        expect(stateLabel("pull", "queued")).toBe("queued");
    });

    it("tells an issue closed as not planned from one closed as done", () => {
        expect(stateLabel("issue", "closed")).toBe("Closed");
        expect(stateLabel("issue", "closed", false, "not_planned")).toBe("Closed as not planned");
        expect(stateLabel("issue", "open", false, "not_planned")).toBe("Open");
    });
});

describe("StateMark", () => {
    it("reads out the state it draws", () => {
        render(
            <>
                <StateMark kind="pull" state="merged" />
                <StateMark kind="pull" state="open" draft />
                <StateMark kind="issue" state="closed" reason="not_planned" />
                <StateMark kind="issue" state="closed" />
                <StateMark kind="issue" state="open" />
            </>,
        );
        const marks = screen.getAllByRole("img");
        expect(marks.map((mark) => [mark.getAttribute("aria-label"), mark.getAttribute("data-state")])).toEqual([
            ["Merged", "merged"],
            ["Draft", "draft"],
            ["Closed as not planned", "not_planned"],
            ["Closed", "closed"],
            ["Open", "open"],
        ]);
        expect(marks.every((mark) => mark.querySelector("svg"))).toBe(true);
    });
});

describe("small pieces", () => {
    it("draws nothing for no labels or no comments", () => {
        const { container } = render(
            <>
                <Labels labels={[]} />
                <Comments count={0} />
            </>,
        );
        expect(container.innerHTML).toBe("");
    });

    it("counts comments in the singular and the plural", () => {
        render(
            <>
                <Comments count={1} />
                <Comments count={3} />
                <Labels labels={[{ name: "bug", color: "red" } as never]} />
            </>,
        );
        expect(screen.getByTitle("1 comment")).toHaveTextContent("1");
        expect(screen.getByTitle("3 comments")).toHaveTextContent("3");
        expect(screen.getByTitle("bug")).toBeInTheDocument();
    });

    it("calls someone unnamed someone", () => {
        render(
            <InHost host={host}>
                <Who login={null} avatarUrl={null} />
            </InHost>,
        );
        expect(screen.getByText("someone")).toBeInTheDocument();
    });
});

describe("PageHead", () => {
    it("goes back and opens the page on the host", async () => {
        const onBack = vi.fn();
        render(
            <InHost host={host}>
                <PageHead mark={null} title="Fix it" number={9} url="https://example.test/9" backLabel="Issues" onBack={onBack}>
                    sub line
                </PageHead>
            </InHost>,
        );
        expect(screen.getByRole("heading", { name: "Fix it" })).toBeInTheDocument();
        expect(screen.getByText("#9")).toBeInTheDocument();
        await userEvent.click(screen.getByRole("button", { name: /Issues/ }));
        expect(onBack).toHaveBeenCalled();
        await userEvent.click(screen.getByRole("button", { name: "On Test host" }));
        expect(shell.openUrl).toHaveBeenCalledWith("https://example.test/9");
    });

    it("has no way back where the list stays beside it", () => {
        render(
            <InHost host={host}>
                <PageHead mark={null} title="Fix it" number={9} url="u" backLabel="Issues">
                    sub
                </PageHead>
            </InHost>,
        );
        expect(screen.queryByRole("button", { name: /Issues/ })).toBeNull();
    });
});
