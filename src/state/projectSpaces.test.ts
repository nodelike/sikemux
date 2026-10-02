import { beforeEach, describe, expect, it } from "vitest";
import * as cmd from "./commands";
import { firstGrapheme, isProjectShown, spaceBadge } from "./projectSpaces";
import { getState, setState } from "./store";

const initial = getState();
beforeEach(() => setState(initial, true));

describe("spaces", () => {
    it("are made, renamed and given an icon by the person", () => {
        const id = cmd.createSpace("  Client A  ")!;
        expect(getState().spaces).toEqual([{ id, name: "Client A", icon: "" }]);
        expect(cmd.createSpace("   ")).toBeNull();

        cmd.renameSpace(id, "Client B");
        cmd.renameSpace(id, "  ");
        cmd.setSpaceIcon(id, "🧑‍💻 laptop");
        expect(getState().spaces[0]).toEqual({ id, name: "Client B", icon: "🧑‍💻" });
    });

    it("never puts a project in a space that does not exist", () => {
        cmd.setProjectSpace("/repo", "nowhere");
        expect(getState().projectSpaces).toEqual({});
        cmd.showSpace("nowhere");
        expect(getState().activeSpaceId).toBeNull();
    });

    it("shows a space only the projects put in it, and All every project", () => {
        const spaces = { "/office": "work" };
        expect(isProjectShown("/office", spaces, null)).toBe(true);
        expect(isProjectShown("/loose", spaces, null)).toBe(true);
        expect(isProjectShown("/office", spaces, "work")).toBe(true);
        expect(isProjectShown("/office", spaces, "home")).toBe(false);
        expect(isProjectShown("/loose", spaces, "work")).toBe(false);
    });

    it("keeps a project opened inside a space in view", () => {
        const work = cmd.createSpace("Work")!;
        const home = cmd.createSpace("Home")!;
        cmd.setProjectSpace("/repo/side", home);
        cmd.showSpace(work);

        cmd.createProjectSession("/repo/new");
        expect(getState().projectSpaces["/repo/new"]).toBe(work);

        cmd.createProjectSession("/repo/side");
        expect(getState().projectSpaces["/repo/side"]).toBe(home);
        expect(getState().activeSpaceId).toBe(home);
    });

    it("badges a space with its emoji, or else the first letter of its name", () => {
        expect(spaceBadge({ id: "a", name: "work", icon: "" })).toBe("W");
        expect(spaceBadge({ id: "b", name: "work", icon: "💼" })).toBe("💼");
        expect(firstGrapheme("👩🏽‍🚀 crew")).toBe("👩🏽‍🚀");
    });
});
