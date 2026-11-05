import { readFileSync } from "node:fs";

import { describe, expect, it } from "vitest";

import { HOOKS_PATH } from "../src/hooks/options.ts";
import { LIVE_PATH } from "../src/live/server.ts";
import { SHARE_PATH } from "../src/share/options.ts";
import { testApp, unreachableDatabase } from "./support.ts";

interface TableRoute {
  method: string;
  path: string;
  upgrade?: string;
}

const table = (
  JSON.parse(
    readFileSync(
      new URL("../../protocol/routes.json", import.meta.url),
      "utf8",
    ),
  ) as { routes: TableRoute[] }
).routes;

/** Routes the table leaves out: Expo's update protocol, and the relay's own question. */
const NOT_IN_THE_TABLE = [
  "GET /updates/manifest",
  "POST /internal/relay-access",
];

const shape = (method: string, path: string) =>
  `${method.toUpperCase()} ${path.replace(/\{\w+\}|:\w+/g, ":_")}`;

describe("routes.json", () => {
  it("lists every HTTP route the API serves, and no other", () => {
    const served = testApp(unreachableDatabase())
      .routes.filter((route) => route.method !== "ALL")
      .map((route) => shape(route.method, route.path));
    const listed = table
      .filter((route) => !route.upgrade)
      .map((route) => shape(route.method, route.path));
    expect([...new Set(served)].sort()).toEqual(
      [
        ...listed,
        ...NOT_IN_THE_TABLE.map((route) => route.replace(/:\w+/g, ":_")),
      ].sort(),
    );
  });

  it("lists every socket the API serves, and no other", () => {
    expect(
      table
        .filter((route) => route.upgrade)
        .map((route) => route.path)
        .sort(),
    ).toEqual([HOOKS_PATH, LIVE_PATH, SHARE_PATH].sort());
  });
});
