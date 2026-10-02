import { cleanup, render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, expect, it, vi } from "vitest";
import { Dropdown } from "./Dropdown";

afterEach(cleanup);

const branches = ["main", "feat/run-page", "fix/rail-density", "release/0.4"].map((name) => ({ value: name, label: name }));

it("filters its options as you type and picks the best match with Enter", async () => {
    const user = userEvent.setup();
    const onChange = vi.fn();
    render(<Dropdown value="main" options={branches} onChange={onChange} title="Branch" search="Find a branch" />);

    await user.click(screen.getByRole("button", { name: "Branch" }));
    const box = screen.getByRole("textbox", { name: "Find a branch" });
    expect(box).toHaveFocus();

    await user.type(box, "rail");
    expect(screen.getAllByRole("option").map((option) => option.textContent)).toEqual(["fix/rail-density"]);
    await user.keyboard("{Enter}");
    expect(onChange).toHaveBeenCalledWith("fix/rail-density");
});

it("says so when nothing matches", async () => {
    const user = userEvent.setup();
    render(<Dropdown value="main" options={branches} onChange={() => {}} title="Branch" search="Find a branch" />);
    await user.click(screen.getByRole("button", { name: "Branch" }));
    await user.type(screen.getByRole("textbox", { name: "Find a branch" }), "zzz");
    expect(screen.queryAllByRole("option")).toHaveLength(0);
    expect(screen.getByText("No matches")).toBeInTheDocument();
});
