import { describe, expect, it } from "vitest";
import { dedent, lexBlocks, parseEnv, parseKeyVals, parseRequest, parseScope } from "./parse";

const REQUEST = `
meta {
  name: Get User
  type: http
  seq: 3
}

post {
  url: {{baseUrl}}/users
  body: json
  auth: bearer
}

params:query {
  enabled: true
  ~disabled: no
}

headers {
  Content-Type: application/json
}

auth:bearer {
  token: {{token}}
}

body:json {
  {
    "name": "Ada"
  }
}

script:pre-request {
  console.log("hi")
}

example {
  keep: me
}
`;

describe("Bruno parser", () => {
    it("lexes top-level blocks and ignores braces inside heredocs", () => {
        const blocks = lexBlocks("script {\n  '''\n  { not a block }\n  '''\n}\nmeta {\n  name: ok\n}\n");
        expect(blocks.map((b) => b.name)).toEqual(["script", "meta"]);
    });

    it("dedents verbatim block content", () => {
        expect(dedent("\n    line one\n      line two\n")).toBe("line one\n  line two");
    });

    it("parses enabled and disabled key/value rows", () => {
        expect(parseKeyVals("a: 1\n~b: 2\nflag\n")).toEqual([
            { name: "a", value: "1", enabled: true },
            { name: "b", value: "2", enabled: false },
            { name: "flag", value: "", enabled: true },
        ]);
    });

    it("parses request blocks into a typed model while preserving unknown blocks", () => {
        const req = parseRequest(REQUEST);

        expect(req.meta).toEqual({ name: "Get User", type: "http", seq: 3 });
        expect(req.method).toBe("post");
        expect(req.url).toBe("{{baseUrl}}/users");
        expect(req.body.mode).toBe("json");
        expect(req.body.text).toContain('"name": "Ada"');
        expect(req.auth.mode).toBe("bearer");
        expect(req.auth.bearer.token).toBe("{{token}}");
        expect(req.params.query).toContainEqual({ name: "disabled", value: "no", enabled: false });
        expect(req.headers).toEqual([{ name: "Content-Type", value: "application/json", enabled: true }]);
        expect(req.scripts.pre).toBe('console.log("hi")');
        expect(req.extra.map((b) => b.name)).toEqual(["example"]);
    });

    it("parses collection/folder scopes and environments", () => {
        const scope = parseScope("meta {\n  name: folder\n}\nvars {\n  baseUrl: https://example.test\n}\n", "fallback");
        expect(scope.meta.name).toBe("folder");
        expect(scope.vars.req).toEqual([{ name: "baseUrl", value: "https://example.test", enabled: true }]);

        const env = parseEnv("vars {\n  token: abc\n}\nvars:secret [\n  password,\n  otp\n]\n", "dev", "/api", "API");
        expect(env.id).toBe("/api::dev");
        expect(env.vars).toEqual([{ name: "token", value: "abc", enabled: true }]);
        expect(env.secretNames).toEqual(["password", "otp"]);
    });
});
