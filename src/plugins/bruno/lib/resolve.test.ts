import { describe, expect, it } from "vitest";
import { parseEnv, parseRequest, parseScope } from "./parse";
import { buildScope, findRequest, requestVars, selectedEnvOf } from "./resolve";
import type { BruCollection, BruTreeNode } from "./types";

const collectionScope = parseScope("vars {\n  baseUrl: https://collection.test\n  shared: collection\n}\n", "collection");
const folderScope = parseScope("vars {\n  shared: folder\n  folderOnly: yes\n}\n", "folder");
const request = parseRequest("meta {\n  name: Ping\n}\nget {\n  url: {{baseUrl}}/ping\n}\nvars:pre-request {\n  requestOnly: ok\n}\n");
const env = parseEnv("vars {\n  baseUrl: https://env.test\n  envOnly: yes\n}\n", "dev", "/api", "API");

const tree: BruTreeNode[] = [
    {
        type: "folder",
        name: "v1",
        path: "/api/v1",
        seq: 0,
        scope: folderScope,
        children: [{ type: "request", name: "Ping", path: "/api/v1/ping.bru", seq: 1, method: "get", collectionPath: "/api", request }],
    },
];
const collection: BruCollection = { rootPath: "/api", name: "API", config: collectionScope, envs: [env], tree };

describe("Bruno request resolution", () => {
    it("finds requests and returns ancestor folder scopes", () => {
        const located = findRequest(tree, "/api/v1/ping.bru");
        expect(located?.request).toBe(request);
        expect(located?.collectionPath).toBe("/api");
        expect(located?.folderScopes).toEqual([folderScope]);
        expect(findRequest(tree, "/missing.bru")).toBeNull();
    });

    it("builds variable scopes in documented precedence order", () => {
        expect(buildScope({ collection, env, secretVars: { token: "secret", shared: "secret" }, folderScopes: [folderScope] })).toEqual({
            baseUrl: "https://env.test",
            shared: "secret",
            folderOnly: "yes",
            envOnly: "yes",
            token: "secret",
        });
    });

    it("extracts request vars and selected environments", () => {
        expect(requestVars(request)).toEqual({ requestOnly: "ok" });
        expect(selectedEnvOf(collection, env.id)).toBe(env);
        expect(selectedEnvOf(collection, null)).toBeUndefined();
    });
});
