import { describe, expect, it } from "vitest";
import { buildScope, findRequest, requestVars, selectedEnvOf } from "./resolve";
import type { BruEnv, BruRequest, BruScope, BruTreeNode } from "./types";

const row = (name: string, value: string) => ({ name, value, enabled: true });
const scope = (name: string, vars: ReturnType<typeof row>[]): BruScope => ({
    meta: { name, type: "", seq: null },
    auth: { mode: "inherit", bearer: { token: "" }, basic: { username: "", password: "" }, apikey: { key: "", value: "", placement: "header" } },
    headers: [],
    vars: { req: vars, res: [] },
    scripts: { pre: "", post: "" },
    extra: [],
});
const collectionScope = scope("collection", [row("baseUrl", "https://collection.test"), row("shared", "collection")]);
const folderScope = scope("folder", [row("shared", "folder"), row("folderOnly", "yes")]);
const env: BruEnv = {
    id: "/api::dev",
    name: "dev",
    collectionPath: "/api",
    collectionName: "API",
    vars: [row("baseUrl", "https://env.test"), row("envOnly", "yes")],
    secretNames: [],
};
const ping: BruTreeNode = { type: "request", name: "Ping", path: "/api/v1/ping.bru", method: "get", collectionPath: "/api" };
const tree: BruTreeNode[] = [{ type: "folder", name: "v1", path: "/api/v1", children: [ping] }];

describe("Bruno request resolution", () => {
    it("finds a request's row by its path", () => {
        expect(findRequest(tree, "/api/v1/ping.bru")).toBe(ping);
        expect(findRequest(tree, "/missing.bru")).toBeNull();
    });

    it("builds variable scopes in documented precedence order", () => {
        expect(buildScope({ env, secretVars: { token: "secret", shared: "secret" }, scopes: [collectionScope, folderScope] })).toEqual({
            baseUrl: "https://env.test",
            shared: "secret",
            folderOnly: "yes",
            envOnly: "yes",
            token: "secret",
        });
    });

    it("extracts request vars and selected environments", () => {
        expect(requestVars({ vars: { req: [row("requestOnly", "ok")], res: [] } } as Pick<BruRequest, "vars"> as BruRequest)).toEqual({
            requestOnly: "ok",
        });
        expect(selectedEnvOf([env], env.id)).toBe(env);
        expect(selectedEnvOf([env], null)).toBeUndefined();
    });
});
