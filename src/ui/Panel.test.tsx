import { cleanup, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { EmptyState, Panel, PanelBody, PanelHeader } from "./Panel";

afterEach(cleanup);

describe("Panel", () => {
    it("focuses from the header without swallowing action clicks", () => {
        const onFocus = vi.fn();
        const onAction = vi.fn();
        render(
            <Panel focused flex={2}>
                <PanelHeader index={1} label="Files" onFocus={onFocus} actions={[{ key: "s", label: "stage", onClick: onAction }]} />
                <PanelBody>rows</PanelBody>
            </Panel>,
        );

        fireEvent.click(screen.getByText("Files"));
        expect(onFocus).toHaveBeenCalledTimes(1);

        fireEvent.click(screen.getByRole("button", { name: /stage/i }));
        expect(onAction).toHaveBeenCalledTimes(1);
        // The action stops propagation so acting never also re-focuses.
        expect(onFocus).toHaveBeenCalledTimes(1);
    });

    it("is inert as a header when it labels nothing focusable", () => {
        render(<PanelHeader label="Limits" rule />);
        expect(screen.queryByRole("button", { name: /Focus Limits/ })).not.toBeInTheDocument();
    });
});

describe("EmptyState", () => {
    it("announces the error tone", () => {
        render(<EmptyState tone="error" message="failed to load remotes" />);
        expect(screen.getByRole("alert")).toHaveTextContent("failed to load remotes");
    });

    it("collapses to one clickable line in the inline variant", () => {
        const onClick = vi.fn();
        const { container } = render(<EmptyState variant="inline" message="no projects" action={{ label: "add", onClick }} />);

        const button = screen.getByRole("button", { name: "no projects" });
        expect(button).toHaveClass("empty-state", "inline", "interactive");
        expect(container.querySelector(".empty-state-title")).toBeNull();
        fireEvent.click(button);
        expect(onClick).toHaveBeenCalledTimes(1);
    });
});
