import { readdirSync, readFileSync } from "node:fs";
import { join } from "node:path";
import { describe, expect, it } from "vitest";

import { schema, validator } from "../src/index.ts";

/** Fixtures of the types sikemux-wire holds, and of those only the core holds. */
const folders = ["../fixtures", "../fixtures-core"].map((folder) =>
  join(import.meta.dirname, folder),
);
const names = Object.keys(schema.$defs) as (keyof typeof schema.$defs)[];
const { validate } = validator();
const folderOf = new Map(
  folders.flatMap((folder) =>
    readdirSync(folder).map((name) => [name, folder] as const),
  ),
);

describe("contract fixtures", () => {
  it("cover every definition, so the Rust round trip sees every type", () => {
    expect([...folderOf.keys()].sort()).toEqual([...names].sort());
  });

  for (const name of names) {
    const folder = folderOf.get(name) ?? folders[0] ?? "";
    for (const file of readdirSync(join(folder, name))) {
      it(`${name}/${file} matches its definition`, () => {
        const value: unknown = JSON.parse(
          readFileSync(join(folder, name, file), "utf8"),
        );
        expect(validate(name, value)).toEqual({ ok: true, value });
      });
    }
  }
});

describe("validator", () => {
  it("names every problem with where it is", () => {
    const result = validate("Health", {
      status: "fine",
      version: 7,
      extra: true,
    });
    expect(result.ok).toBe(false);
    if (result.ok) return;
    expect(result.problems).toEqual(
      expect.arrayContaining([
        "the value must have required property 'database'",
        "the value must NOT have additional properties",
        "/status must be equal to one of the allowed values",
        "/version must be string",
      ]),
    );
  });
});
