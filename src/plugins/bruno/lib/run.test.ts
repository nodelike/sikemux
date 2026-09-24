import { describe, expect, it } from "vitest";
import { parseRequest, parseScope } from "./parse";
import { effectiveAuth } from "./run";

describe("Bruno run helpers", () => {
    it("resolves auth inheritance request -> leaf folder -> root -> collection", () => {
        const request = parseRequest("get {\n  url: /\n  auth: inherit\n}\n");
        const collection = parseScope("auth {\n  mode: bearer\n}\nauth:bearer {\n  token: collection\n}\n", "collection");
        const rootFolder = parseScope("auth {\n  mode: basic\n}\nauth:basic {\n  username: root\n  password: pw\n}\n", "root");
        const leafFolder = parseScope("vars {\n  ignored: true\n}\n", "leaf");

        expect(effectiveAuth(request, [collection, rootFolder, leafFolder])).toEqual(rootFolder.auth);

        request.auth.mode = "bearer";
        request.auth.bearer.token = "request";
        expect(effectiveAuth(request, [collection, rootFolder])).toEqual(request.auth);
    });
});
