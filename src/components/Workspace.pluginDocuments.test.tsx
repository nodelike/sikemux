import { act, cleanup, render, screen } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it } from "vitest";
import { create } from "zustand";
import { registerFrontendPlugin } from "../plugins/registry";
import * as cmd from "../state/commands";
import { getState, setState } from "../state/store";
import { Workspace } from "./Workspace";

const PAD = "test.strip:pad";
const pads = create<{ open: string[]; active: string | null }>(() => ({ open: [], active: null }));

registerFrontendPlugin({
    id: "test.strip",
    surfaces: [
        {
            kind: PAD,
            title: "Pad",
            icon: () => null,
            render: () => <div data-testid="pad" />,
            documents: {
                list: () => ({ ids: pads.getState().open, activeId: pads.getState().active }),
                describe: (_pane, id) => ({ label: `Doc ${id}`, dirty: id === "b" }),
                select: (_pane, id) => pads.setState({ active: id }),
                close: () => {},
                subscribe: (listener) => pads.subscribe(listener),
            },
        },
    ],
    open: () => cmd.openPluginSession(PAD),
    openTitle: "Open Pad",
});

const initial = getState();

beforeEach(() => {
    setState(initial, true);
    pads.setState({ open: ["a", "b"], active: "a" });
    cmd.openPluginSession(PAD);
});
afterEach(cleanup);

describe("the workspace strip", () => {
    it("shows a plugin's documents as tabs and follows them as they change", () => {
        render(<Workspace />);
        expect(screen.getByRole("tab", { name: "Doc a" })).toHaveAttribute("aria-selected", "true");
        expect(screen.getByRole("tab", { name: "Doc b, unsaved changes" })).toHaveAttribute("aria-selected", "false");

        act(() => pads.setState({ open: ["a", "b", "c"], active: "c" }));
        expect(screen.getByRole("tab", { name: "Doc c" })).toHaveAttribute("aria-selected", "true");

        act(() => screen.getByRole("tab", { name: "Doc a" }).click());
        expect(pads.getState().active).toBe("a");
    });
});
