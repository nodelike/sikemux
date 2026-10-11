import { beforeEach, describe, expect, it } from "vitest";
import * as cmd from "./commands";
import { isProjectShown } from "./projectSpaces";
import { getState, setState } from "./store";

const initial = getState();
beforeEach(() => setState(initial, true));

describe("spaces", () => {
    it("are made, renamed and given an icon by the person", () => {
        const id = cmd.createSpace("  Client A  ")!;
        expect(getState().spaces).toEqual([{ id, name: "Client A", icon: "folder" }]);
        expect(cmd.createSpace("   ")).toBeNull();

        cmd.renameSpace(id, "Client B");
        cmd.renameSpace(id, "  ");
        cmd.setSpaceIcon(id, "briefcase");
        expect(getState().spaces[0]).toEqual({ id, name: "Client B", icon: "briefcase" });
        cmd.setSpaceIcon(id, "");
        expect(getState().spaces[0].icon).toBe("");
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

    it("are put in the order the person drags them to", () => {
        const a = cmd.createSpace("A")!;
        const b = cmd.createSpace("B")!;
        const c = cmd.createSpace("C")!;
        cmd.moveSpace(c, 0);
        expect(getState().spaces.map((space) => space.id)).toEqual([c, a, b]);
        cmd.moveSpace(c, 99);
        expect(getState().spaces.map((space) => space.id)).toEqual([a, b, c]);
    });
});
