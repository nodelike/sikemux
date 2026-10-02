import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { expect, it, vi } from "vitest";
import { ErrorBoundary } from "./ErrorBoundary";

it("isolates a crashed pane and allows retry", async () => {
    const user = userEvent.setup();
    const consoleError = vi.spyOn(console, "error").mockImplementation(() => {});
    let shouldThrow = true;
    function Child() {
        if (shouldThrow) throw new Error("broken pane");
        return <span>recovered</span>;
    }

    render(
        <ErrorBoundary label="editor pane">
            <Child />
        </ErrorBoundary>,
    );

    expect(screen.getByRole("alert")).toHaveTextContent("editor pane crashed");
    shouldThrow = false;
    await user.click(screen.getByRole("button", { name: "retry" }));
    expect(screen.getByText("recovered")).toBeInTheDocument();
    consoleError.mockRestore();
});
