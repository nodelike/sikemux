import { act, cleanup, fireEvent, render, screen, within } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { getState, setState } from "../state/store";
import type { Session, SessionKind } from "../state/types";
import "../plugins/builtin";
import { useLeaveSettingsOnNavigation } from "../settings/leaveSettings";
import { SideRail } from "./SideRail";
import * as cmd from "../state/commands";
import { acceptDialog, useDialogs } from "../state/dialog";

const initial = getState();

function session(id: string, kind: SessionKind): Session {
    return {
        id,
        name: id,
        kind,
        cwd: `/${id}`,
        pinned: false,
        activeWindowId: "",
    };
}

beforeEach(() => {
    setState(initial, true);
    const sessions = {
        alpha: session("alpha", "project"),
        ssh: session("ssh", "ssh"),
        beta: session("beta", "project"),
        command: session("command", "command"),
        gamma: session("gamma", "project"),
    };
    setState({
        sessions,
        sessionOrder: ["alpha", "ssh", "beta", "command", "gamma"],
        activeSessionId: "command",
        windows: {},
        windowsBySession: Object.fromEntries(Object.keys(sessions).map((id) => [id, []])),
        agents: {},
    });
});

afterEach(() => {
    cleanup();
    vi.restoreAllMocks();
});

describe("project sorting", () => {
    it("drags a project before another project and shows the insertion point", () => {
        render(<SideRail />);
        const source = screen.getByRole("button", { name: "gamma" });
        const target = screen.getByRole("button", { name: "alpha" });
        let ghostWasHiddenDuringHitTest = false;
        Object.defineProperty(document, "elementFromPoint", {
            configurable: true,
            value: vi.fn(() => {
                ghostWasHiddenDuringHitTest ||= document.querySelector<HTMLElement>("[data-project-drag-ghost]")?.style.visibility === "hidden";
                return target;
            }),
        });
        vi.spyOn(source, "getBoundingClientRect").mockReturnValue({ left: 8, top: 80, width: 210, height: 26 } as DOMRect);
        vi.spyOn(target, "getBoundingClientRect").mockReturnValue({ top: 20, bottom: 48, height: 28 } as DOMRect);

        fireEvent.pointerDown(source, { button: 0, clientX: 0, clientY: 0 });
        fireEvent.pointerMove(window, { clientX: 0, clientY: 22 });
        fireEvent.pointerMove(window, { clientX: 0, clientY: 23 });

        const ghost = document.querySelector<HTMLElement>("[data-project-drag-ghost]");
        expect(ghost).toHaveStyle({ width: "210px", height: "26px" });
        expect(ghost?.querySelector(".project-drag-ghost-row")).toHaveTextContent("gamma");
        expect(ghost?.querySelector(".project-drag-ghost-card")).not.toBeInTheDocument();
        /*
         * The ghost is styled by class — `.project-drag-ghost *` takes the
         * pointer events away. It used to arrive with every computed style of
         * every element written back as an inline property, which is hundreds
         * of reads at the moment a drag starts.
         */
        for (const element of ghost?.querySelectorAll<HTMLElement>("*") ?? []) {
            expect(element.getAttribute("style")).toBeNull();
        }
        expect(ghostWasHiddenDuringHitTest).toBe(true);
        expect(ghost?.style.visibility).toBe("");
        expect(getState().sessionOrder).toEqual(["gamma", "ssh", "alpha", "command", "beta"]);
        expect(screen.getByRole("button", { name: "alpha" }).closest("[data-project-id]")).toHaveClass("project-drop-before");

        fireEvent.pointerUp(window, { clientX: 0, clientY: 22 });

        expect(getState().sessionOrder).toEqual(["gamma", "ssh", "alpha", "command", "beta"]);
    });
});

const MANIFESTS = [
    { id: "sikemux.aws", name: "AWS" },
    { id: "sikemux.bruno", name: "Bruno" },
    { id: "sikemux.rundeck", name: "Rundeck" },
    { id: "sikemux.signoz", name: "SigNoz" },
].map((plugin) => ({ ...plugin, version: "0.1.0", sikemux: ">=0.4" }));

describe("plugins group", () => {
    it("always lists every enabled plugin, and opens one only when it is clicked", () => {
        setState({
            sessions: { ...getState().sessions, aws: session("aws", "sikemux.aws:console") },
            sessionOrder: [...getState().sessionOrder, "aws"],
            windowsBySession: { ...getState().windowsBySession, aws: [] },
            pluginManifests: MANIFESTS,
        });
        render(<SideRail />);

        expect(screen.getByText("Plugins")).toBeTruthy();
        expect(screen.getByRole("button", { name: "aws" })).toBeTruthy();
        for (const name of ["Bruno", "Rundeck", "SigNoz"]) expect(screen.getByRole("button", { name })).toBeTruthy();
        expect(screen.queryByText(/^open /)).toBeNull();
        const before = getState().sessionOrder.length;
        expect(Object.values(getState().sessions).some((each) => each.kind === "sikemux.signoz:explore")).toBe(false);

        fireEvent.click(screen.getByRole("button", { name: "SigNoz" }));
        expect(getState().sessionOrder.length).toBe(before + 1);
        expect(getState().sessions[getState().activeSessionId].kind).toBe("sikemux.signoz:explore");
    });

    it("leaves out a plugin that is switched off", () => {
        setState({ pluginManifests: MANIFESTS, disabledPlugins: ["sikemux.rundeck"] });
        render(<SideRail />);
        expect(screen.getByRole("button", { name: "Bruno" })).toBeTruthy();
        expect(screen.queryByRole("button", { name: "Rundeck" })).toBeNull();
    });
});

function SettingsOpenBeside() {
    useLeaveSettingsOnNavigation();
    return null;
}

describe("leaving settings from the rail", () => {
    for (const [what, name] of [
        ["switching project", "beta"],
        ["closing a project", "Close beta"],
        ["opening a project", /^Open project/],
        ["starting a terminal", "New terminal"],
    ] as const) {
        it(`closes settings when ${what}`, () => {
            setState({ settingsOpen: true });
            render(
                <>
                    <SettingsOpenBeside />
                    <SideRail />
                </>,
            );

            fireEvent.click(screen.getByRole("button", { name }));

            expect(getState().settingsOpen).toBe(false);
        });
    }

    it("leaves the rail's clicks alone while settings is closed", () => {
        render(<SideRail />);

        fireEvent.click(screen.getByRole("button", { name: "beta" }));

        expect(getState().settingsOpen).toBe(false);
        expect(getState().activeSessionId).toBe("beta");
    });
});

describe("project spaces", () => {
    function withSpaces() {
        const work = cmd.createSpace("Work", "briefcase")!;
        const home = cmd.createSpace("Home")!;
        cmd.setProjectSpace("/alpha", work);
        cmd.setProjectSpace("/beta", home);
        return { work, home };
    }

    it("shows only the projects put in the chosen space, and every project under All", () => {
        withSpaces();
        setState({ activeSessionId: "beta" });
        render(<SideRail />);

        fireEvent.click(screen.getByRole("radio", { name: "Work" }));

        expect(screen.getByRole("button", { name: "alpha" })).toBeInTheDocument();
        expect(screen.queryByRole("button", { name: "beta" })).not.toBeInTheDocument();
        expect(screen.queryByRole("button", { name: "gamma" })).not.toBeInTheDocument();
        expect(getState().sessions.beta).toBeDefined();
        expect(getState().sessionOrder).toEqual(["alpha", "ssh", "beta", "command", "gamma"]);
        expect(getState().activeSessionId).toBe("alpha");

        fireEvent.click(screen.getByRole("radio", { name: "All" }));
        expect(screen.getByRole("button", { name: "beta" })).toBeInTheDocument();
        expect(screen.getByRole("button", { name: "gamma" })).toBeInTheDocument();
    });

    it("creates a space from the switch and shows it", async () => {
        render(<SideRail />);
        expect(screen.queryByRole("radiogroup", { name: "Projects shown" })).not.toBeInTheDocument();

        fireEvent.click(screen.getByRole("button", { name: "New space" }));
        const dialog = useDialogs.getState().dialog!;
        expect(dialog).toMatchObject({ kind: "prompt", title: "Create space" });
        await act(async () => acceptDialog(dialog.id, "Client A"));

        expect(getState().spaces.map((space) => space.name)).toEqual(["Client A"]);
        expect(screen.getByRole("radio", { name: "Client A" })).toHaveAttribute("aria-checked", "true");
    });

    it("moves a project between spaces from its right-click menu", () => {
        const { home } = withSpaces();
        render(<SideRail />);

        fireEvent.contextMenu(screen.getByRole("button", { name: "gamma" }));
        fireEvent.click(screen.getByRole("menuitem", { name: "Home" }));
        expect(getState().projectSpaces["/gamma"]).toBe(home);

        fireEvent.contextMenu(screen.getByRole("button", { name: "gamma" }));
        fireEvent.click(screen.getByText("No Space"));
        expect(getState().projectSpaces["/gamma"]).toBeUndefined();
    });

    it("deletes a space from its menu after asking, leaving its projects in no space", async () => {
        const { work } = withSpaces();
        cmd.showSpace(work);
        render(<SideRail />);

        fireEvent.contextMenu(screen.getByRole("radio", { name: "Work" }));
        fireEvent.click(screen.getByText("Delete Space…"));
        await act(async () => acceptDialog(useDialogs.getState().dialog!.id));

        expect(getState().spaces.map((space) => space.name)).toEqual(["Home"]);
        expect(getState().projectSpaces["/alpha"]).toBeUndefined();
        expect(getState().activeSpaceId).toBeNull();
        expect(screen.getByRole("button", { name: "beta" })).toBeInTheDocument();
    });

    it("changes a space's icon from its right-click menu", () => {
        const { work } = withSpaces();
        render(<SideRail />);

        fireEvent.contextMenu(screen.getByRole("radio", { name: "Work" }));
        fireEvent.click(screen.getByRole("menuitem", { name: "Change Icon…" }));
        fireEvent.click(within(screen.getByRole("dialog", { name: "Space icon" })).getByRole("button", { name: "rocket" }));
        expect(getState().spaces.find((space) => space.id === work)?.icon).toBe("rocket");

        fireEvent.contextMenu(screen.getByRole("radio", { name: "Work" }));
        fireEvent.click(screen.getByRole("menuitem", { name: "Remove Icon" }));
        expect(getState().spaces.find((space) => space.id === work)?.icon).toBe("");
    });

    it("renames a space and picks its icon in the space editor, then goes back to the editor", () => {
        const { home } = withSpaces();
        render(<SideRail />);

        fireEvent.contextMenu(screen.getByRole("radio", { name: "All" }));
        fireEvent.click(screen.getByRole("menuitem", { name: "Edit Spaces…" }));
        const editor = screen.getByRole("dialog", { name: "Edit spaces" });
        const name = within(editor).getByRole("textbox", { name: "Name of Home" });
        fireEvent.change(name, { target: { value: "House" } });
        fireEvent.blur(name);
        expect(getState().spaces.find((space) => space.id === home)?.name).toBe("House");

        fireEvent.click(within(editor).getByRole("button", { name: "Icon for House" }));
        fireEvent.click(within(screen.getByRole("dialog", { name: "Space icon" })).getByRole("button", { name: "home" }));
        expect(getState().spaces.find((space) => space.id === home)?.icon).toBe("home");
        expect(screen.getByRole("dialog", { name: "Edit spaces" })).toBeInTheDocument();
    });
});
