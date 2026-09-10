import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, describe, expect, it, vi } from "vitest";
import { useRef, useState } from "react";
import { useModalFocus } from "../hooks/useModalFocus";
import { TreeContextMenu } from "./FileTree";
import { navigateTabs } from "../lib/tabNavigation";
import { Dropdown } from "./Dropdown";
import { DialogHost } from "./DialogHost";
import { confirmDialog, resetDialogsForTests } from "../state/dialog";
import { TabBar } from "./TabBar";
import { Tooltip } from "./Tooltip";

afterEach(() => {
    resetDialogsForTests();
    cleanup();
});

describe("shared keyboard journeys", () => {
    it("selects a dropdown with arrows and type-ahead, restoring focus", async () => {
        const user = userEvent.setup();
        const change = vi.fn();
        render(
            <Dropdown
                label="Provider"
                value="a"
                options={[
                    { value: "a", label: "Alpha" },
                    { value: "b", label: "Beta" },
                    { value: "g", label: "Gamma" },
                ]}
                onChange={change}
            />,
        );
        const trigger = screen.getByRole("button", { name: "Provider" });
        trigger.focus();
        await user.keyboard("{ArrowDown}g{Enter}");
        expect(change).toHaveBeenCalledWith("g");
        expect(trigger).toHaveFocus();
        expect(screen.queryByRole("listbox")).not.toBeInTheDocument();
        await user.keyboard("{ArrowDown}{ArrowDown}{Enter}");
        expect(change).toHaveBeenLastCalledWith("b");
    });
    it("contains modal focus and returns to the opener", async () => {
        const user = userEvent.setup();
        render(
            <>
                <button onClick={() => void confirmDialog({ title: "Confirm operation" })}>Open</button>
                <button>Outside</button>
                <DialogHost />
            </>,
        );
        const opener = screen.getByRole("button", { name: "Open" });
        await user.click(opener);
        expect(screen.getByRole("button", { name: "Confirm" })).toHaveFocus();
        await user.tab();
        expect(screen.getByRole("button", { name: "Cancel" })).toHaveFocus();
        await user.tab({ shift: true });
        expect(screen.getByRole("button", { name: "Confirm" })).toHaveFocus();
        await user.keyboard("{Escape}");
        await waitFor(() => expect(opener).toHaveFocus());
        expect(screen.getByRole("button", { name: "Outside" }).closest("[inert]")).toBeNull();
    });
    it("moves across tabs with arrows and keeps one tab stop", async () => {
        function Tabs() {
            const [selected, select] = useState("a");
            return <TabBar variant="editor" tabs={["a", "b", "c"].map((id) => ({ id, label: id, active: selected === id }))} onSelect={select} />;
        }
        const user = userEvent.setup();
        render(<Tabs />);
        screen.getByRole("tab", { name: "a" }).focus();
        await user.keyboard("{ArrowRight}");
        expect(screen.getByRole("tab", { name: "b" })).toHaveAttribute("aria-selected", "true");
        expect(screen.getByRole("tab", { name: "b" })).toHaveFocus();
        expect(screen.getAllByRole("tab").filter((tab) => tab.tabIndex === 0)).toHaveLength(1);
        await user.keyboard("{End}");
        expect(screen.getByRole("tab", { name: "c" })).toHaveFocus();
    });
    it("associates focused tooltips and dismisses them on Escape", async () => {
        render(
            <Tooltip label="Full path">
                <button>File</button>
            </Tooltip>,
        );
        fireEvent.focus(screen.getByRole("button"));
        expect(screen.getByRole("button")).toHaveAttribute("aria-describedby", screen.getByRole("tooltip").id);
        fireEvent.keyDown(window, { key: "Escape" });
        expect(screen.queryByRole("tooltip")).not.toBeInTheDocument();
    });
    it("keeps a portaled dropdown inside a modal focus scope", async () => {
        function Modal() {
            const ref = useRef<HTMLDivElement>(null);
            useModalFocus(ref);
            const [value, setValue] = useState("a");
            return (
                <div ref={ref} role="dialog" tabIndex={-1}>
                    <Dropdown
                        label="Modal choice"
                        value={value}
                        options={[
                            { value: "a", label: "Alpha" },
                            { value: "b", label: "Beta" },
                        ]}
                        onChange={setValue}
                    />
                    <button>Done</button>
                </div>
            );
        }
        const user = userEvent.setup();
        render(
            <>
                <button>Outside</button>
                <Modal />
            </>,
        );
        await user.keyboard("{ArrowDown}{ArrowDown}{Enter}");
        expect(screen.getByRole("button", { name: "Modal choice" })).toHaveTextContent("Beta");
        expect(screen.getByRole("button", { name: "Modal choice" })).toHaveFocus();
        await user.tab();
        expect(screen.getByRole("button", { name: "Done" })).toHaveFocus();
        await user.tab();
        expect(screen.getByRole("button", { name: "Modal choice" })).toHaveFocus();
    });
    it("walks vertical tabs with Up and Down", async () => {
        function Tabs() {
            const [value, select] = useState("Files");
            return (
                <div role="tablist" aria-orientation="vertical">
                    {["Files", "Changes", "Search"].map((label) => (
                        <button
                            key={label}
                            role="tab"
                            aria-selected={value === label}
                            tabIndex={value === label ? 0 : -1}
                            onKeyDown={navigateTabs}
                            onClick={() => select(label)}>
                            {label}
                        </button>
                    ))}
                </div>
            );
        }
        const user = userEvent.setup();
        render(<Tabs />);
        screen.getByRole("tab", { name: "Files" }).focus();
        await user.keyboard("{ArrowDown}");
        expect(screen.getByRole("tab", { name: "Changes" })).toHaveFocus();
        await user.keyboard("{ArrowUp}");
        expect(screen.getByRole("tab", { name: "Files" })).toHaveFocus();
    });
    it("walks context menus with arrows and restores focus on Escape", async () => {
        function Menu() {
            const [open, setOpen] = useState(false);
            return (
                <>
                    <button onClick={() => setOpen(true)}>Actions</button>
                    {open && (
                        <TreeContextMenu
                            x={0}
                            y={0}
                            items={[{ label: "Open" }, { label: "Unavailable", disabled: true }, { label: "Rename" }]}
                            onClose={() => setOpen(false)}
                        />
                    )}
                </>
            );
        }
        const user = userEvent.setup();
        render(<Menu />);
        await user.click(screen.getByRole("button", { name: "Actions" }));
        expect(screen.getByRole("menuitem", { name: "Open" })).toHaveFocus();
        await user.keyboard("{ArrowDown}");
        expect(screen.getByRole("menuitem", { name: "Rename" })).toHaveFocus();
        await user.keyboard("{Escape}");
        expect(screen.getByRole("button", { name: "Actions" })).toHaveFocus();
    });
});
