import { describe, expect, it } from "vitest";
import { parseEnv, parseRequest, parseScope } from "./parse";
import { serializeEnv, serializeRequest, serializeScope } from "./serialize";
import { emptyRequest, emptyScope } from "./types";

describe("Bruno serializer", () => {
    it("emits a stable minimal request", () => {
        const req = emptyRequest("Ping");
        req.method = "get";
        req.url = "{{baseUrl}}/ping";
        req.headers = [{ name: "Accept", value: "application/json", enabled: true }];

        expect(serializeRequest(req)).toContain("meta {\n  name: Ping\n  type: http\n}");
        expect(serializeRequest(req)).toContain("get {\n  url: {{baseUrl}}/ping\n  body: none\n  auth: inherit\n}");
        expect(serializeRequest(req)).toContain("headers {\n  Accept: application/json\n}");
    });

    it("round-trips parsed requests at the model level", () => {
        const src = `meta {\n  name: Create\n  type: http\n}\n\npost {\n  url: /users\n  body: json\n  auth: basic\n}\n\nauth:basic {\n  username: ada\n  password: secret\n}\n\nbody:json {\n  {\"ok\":true}\n}\n`;
        const parsed = parseRequest(src);
        expect(parseRequest(serializeRequest(parsed))).toEqual(parsed);
    });

    it("serializes scopes and env files", () => {
        const scope = emptyScope("folder");
        scope.vars.req = [{ name: "baseUrl", value: "https://example.test", enabled: true }];
        expect(parseScope(serializeScope(scope))).toMatchObject({ meta: { name: "folder" }, vars: scope.vars });

        const env = parseEnv("vars {\n  token: abc\n}\nvars:secret [\n  password\n]\n", "dev", "/api", "API");
        expect(serializeEnv(env)).toContain("vars {\n  token: abc\n}");
        expect(serializeEnv(env)).toContain("vars:secret [\n  password\n]");
    });
});
