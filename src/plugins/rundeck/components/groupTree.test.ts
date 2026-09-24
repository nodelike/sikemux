import { describe, expect, it } from "vitest";
import { ancestorPaths, buildGroupTree } from "./groupTree";

describe("buildGroupTree", () => {
    it("nests groups to any depth and counts every job below a folder", () => {
        const tree = buildGroupTree([
            { group: "prod/backend/api" },
            { group: "prod/backend" },
            { group: "prod" },
            { group: "Staging/web" },
            { group: null },
            { group: "" },
        ]);
        expect(tree.total).toBe(6);
        expect(tree.ungrouped).toBe(2);
        expect(tree.children.map((node) => [node.name, node.count])).toEqual([
            ["prod", 3],
            ["Staging", 1],
        ]);
        const backend = tree.children[0].children[0];
        expect(backend).toMatchObject({ name: "backend", path: "prod/backend", count: 2 });
        expect(backend.children[0]).toMatchObject({ name: "api", path: "prod/backend/api", count: 1 });
    });

    it("lists the folders to open to reveal a path", () => {
        expect(ancestorPaths("prod/backend/api")).toEqual(["prod", "prod/backend", "prod/backend/api"]);
        expect(ancestorPaths(null)).toEqual([]);
    });
});
