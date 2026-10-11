import { describe, expect, it } from "vitest";
import type { DatabaseProfile, ProfileDraft } from "./api";
import { addressOf, blankDraft, defaultSchema, draftOf, engineLabel, isServer, missingField, parsePort, withEngine } from "./profileForm";

const postgres: ProfileDraft = {
    name: "Shop",
    readOnly: false,
    agentWrites: false,
    engine: "postgres",
    host: "db.internal",
    port: null,
    database: "shop",
    user: "app",
    tls: "prefer",
};

describe("profileForm", () => {
    it("starts a new PostgreSQL profile on localhost with encryption preferred", () => {
        expect(blankDraft()).toEqual({ ...postgres, name: "", host: "localhost", database: "", user: "" });
        expect(blankDraft("sqlite")).toEqual({ name: "", readOnly: false, agentWrites: false, engine: "sqlite", path: "" });
    });

    it("names the first thing still missing", () => {
        expect(missingField(blankDraft())).toBe("a name");
        expect(missingField({ ...postgres, host: " " })).toBe("a host");
        expect(missingField({ ...postgres, user: "" })).toBe("a user name");
        expect(missingField({ ...postgres, port: 70000 })).toBe("a port from 1 to 65535");
        expect(missingField({ ...postgres, port: Number("abc") })).toBe("a port from 1 to 65535");
        expect(missingField(postgres)).toBeNull();
        expect(missingField({ name: "Local", readOnly: false, agentWrites: false, engine: "sqlite", path: "" })).toBe("the database file");
        expect(missingField({ name: "Local", readOnly: false, agentWrites: false, engine: "sqlite", path: "/tmp/a.db" })).toBeNull();
        expect(missingField({ ...postgres, engine: "mysql", user: "" })).toBe("a user name");
    });

    it("keeps the name and read-only choice when the engine changes", () => {
        const switched = withEngine({ ...postgres, id: "1", readOnly: true }, "sqlite");
        expect(switched).toEqual({ id: "1", name: "Shop", readOnly: true, agentWrites: false, engine: "sqlite", path: "" });
        expect(withEngine(postgres, "mysql")).toMatchObject({ engine: "mysql", host: "localhost", user: "" });
        expect(withEngine(postgres, "postgres")).toBe(postgres);
    });

    it("edits a saved profile without the fields only the backend sets", () => {
        const saved: DatabaseProfile = { ...postgres, id: "1", hasPassword: true };
        expect(isServer(saved)).toBe(true);
        expect(isServer({ engine: "sqlite", path: "/a.db" })).toBe(false);
        expect(draftOf(saved)).toEqual({ ...postgres, id: "1" });
    });

    it("opens each engine on its usual schema", () => {
        expect(defaultSchema(postgres)).toBe("public");
        expect(defaultSchema({ ...postgres, engine: "mysql", database: "app" })).toBe("app");
        expect(defaultSchema({ engine: "sqlite", path: "/a.db" })).toBe("main");
    });

    it("reads an empty port as the default one", () => {
        expect(parsePort("")).toBeNull();
        expect(parsePort(" 6543 ")).toBe(6543);
    });

    it("shows where each database is", () => {
        expect(addressOf(postgres)).toBe("app@db.internal:5432/shop");
        expect(addressOf({ ...postgres, port: 6543, database: "" })).toBe("app@db.internal:6543");
        expect(addressOf({ engine: "sqlite", path: "/Users/me/data/app.db" })).toBe("app.db");
        expect(addressOf({ ...postgres, engine: "mysql", database: "app" })).toBe("app@db.internal:3306/app");
        expect(engineLabel("postgres")).toBe("PostgreSQL");
        expect(engineLabel("mysql")).toBe("MySQL");
    });
});
