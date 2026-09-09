import { act, cleanup, render, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

const mocks = vi.hoisted(() => ({
    scanProjectRoots: vi.fn(),
}));

vi.mock("../api/settings", () => ({
    settingsApi: {
        scanProjectRoots: mocks.scanProjectRoots,
        pickFolder: vi.fn(),
    },
}));

import * as cmd from "../state/commands";
import { invalidate } from "../state/resources";
import { getState, setState } from "../state/store";
import type { ProjectRoot } from "../state/types";
import { SeshPicker } from "./SeshPicker";

const initial = getState();

beforeEach(() => {
    setState(initial, true);
    setState({
        sessions: {},
        sessionOrder: [],
        activeSessionId: "",
        home: "/Users/friend",
        pickerMode: "projects",
        projectRoots: [],
    });
    mocks.scanProjectRoots.mockImplementation(async (roots: ProjectRoot[]) =>
        roots
            .filter((root) => root.selfIndex)
            .map((root) => ({
                name: root.path.split("/").at(-1) ?? root.path,
                path: root.path,
            })),
    );
    invalidate((kind) => kind === "settings.projectRootsScan");
});

afterEach(() => {
    cleanup();
    vi.clearAllMocks();
});

describe("SeshPicker project indexing", () => {
    it("reflects project-root setting changes while the picker is open", async () => {
        render(<SeshPicker />);
        expect(screen.getByText(/no projects configured/)).toBeInTheDocument();

        act(() => cmd.addProjectRoot("/Users/friend/personal", 1, true));

        await waitFor(() => expect(mocks.scanProjectRoots).toHaveBeenLastCalledWith([{ path: "/Users/friend/personal", depth: 1, selfIndex: true }]));
        expect(await screen.findByText("personal")).toBeInTheDocument();
        expect(screen.getByText("~/personal")).toBeInTheDocument();

        act(() => cmd.setProjectRootSelfIndex("/Users/friend/personal", false));

        await waitFor(() =>
            expect(mocks.scanProjectRoots).toHaveBeenLastCalledWith([{ path: "/Users/friend/personal", depth: 1, selfIndex: false }]),
        );
        await waitFor(() => expect(screen.queryByText("~/personal")).not.toBeInTheDocument());
    });
});
