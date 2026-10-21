import { describe, expect, it } from "vitest";

import {
  bundle,
  collect,
  openapi,
  rust,
  typescript,
} from "../scripts/generator.mjs";

function document(file: string, $defs: Record<string, unknown>) {
  return {
    [file]: {
      $schema: "https://json-schema.org/draft/2020-12/schema",
      $id: file,
      $defs,
    },
  };
}

const device = {
  ...document("devices.json", {
    Device: {
      description: "A registered device.",
      type: "object",
      properties: {
        key: { $ref: "common.json#/$defs/DeviceKey" },
        role: { $ref: "#/$defs/DeviceRole" },
        name: { type: "string", maxLength: 64 },
        lastSeenAt: { type: ["string", "null"], format: "date-time" },
        channel: { type: "string" },
        tags: { type: "array", items: { type: "string" } },
        load: { type: "number" },
      },
      required: ["key", "role", "name", "lastSeenAt", "tags", "load"],
      additionalProperties: false,
    },
    DeviceRole: { type: "string", enum: ["host", "client", "device.revoked"] },
  }),
  ...document("common.json", {
    DeviceKey: {
      description: "An iroh public key, in hex.",
      type: "string",
      pattern: "^[0-9a-f]{64}$",
    },
  }),
};

const message = (type: string, properties: Record<string, unknown> = {}) => ({
  type: "object",
  properties: { type: { const: type }, ...properties },
  required: ["type", ...Object.keys(properties)],
  additionalProperties: false,
});

const live = document("live.json", {
  ServerMessage: {
    description: "What the server sends.",
    type: "object",
    oneOf: [{ $ref: "#/$defs/Hello" }, { $ref: "#/$defs/Ping" }],
    discriminator: { propertyName: "type" },
  },
  Hello: message("hello", { nonce: { type: "string" } }),
  Ping: message("ping"),
});

describe("collect", () => {
  it("accepts the supported shapes", () => {
    expect([...collect(device).keys()].sort()).toEqual([
      "Device",
      "DeviceKey",
      "DeviceRole",
    ]);
  });

  const rejects: [string, Record<string, unknown>, RegExp][] = [
    [
      "open objects",
      { A: { type: "object", properties: {} } },
      /additionalProperties: false/,
    ],
    [
      "inline enums",
      {
        A: {
          type: "object",
          properties: { a: { type: "string", enum: ["x"] } },
          additionalProperties: false,
        },
      },
      /enums need their own definition/,
    ],
    [
      "inline objects",
      {
        A: {
          type: "object",
          properties: { a: { type: "object" } },
          additionalProperties: false,
        },
      },
      /objects and enums need their own definition/,
    ],
    [
      "optional nullable properties",
      {
        A: {
          type: "object",
          properties: { a: { type: ["string", "null"] } },
          additionalProperties: false,
        },
      },
      /optional or nullable, not both/,
    ],
    [
      "names that do not survive snake_case",
      {
        A: {
          type: "object",
          properties: { deviceID: { type: "string" } },
          additionalProperties: false,
        },
      },
      /camelCase words/,
    ],
    [
      "dangling references",
      {
        A: {
          type: "object",
          properties: { a: { $ref: "#/$defs/B" } },
          additionalProperties: false,
        },
      },
      /points at nothing/,
    ],
    [
      "the reserved enum value",
      { A: { type: "string", enum: ["unknown"] } },
      /reserved/,
    ],
    [
      "unsupported keywords",
      { A: { type: "string", allOf: [] } },
      /"allOf" is not supported/,
    ],
    ["lowercase definition names", { a: { type: "string" } }, /PascalCase/],
    [
      "constants outside a union's tag",
      { A: message("a") },
      /only for the property that tags a union's members/,
    ],
    [
      "unions that do not say they are objects",
      {
        U: {
          oneOf: [{ $ref: "#/$defs/A" }],
          discriminator: { propertyName: "type" },
        },
        A: message("a"),
      },
      /type: "object"/,
    ],
    [
      "unions without a discriminator",
      {
        U: { type: "object", oneOf: [{ $ref: "#/$defs/A" }] },
        A: message("a"),
      },
      /discriminator/,
    ],
    [
      "union members without the tag",
      {
        U: {
          type: "object",
          oneOf: [{ $ref: "#/$defs/A" }],
          discriminator: { propertyName: "type" },
        },
        A: { type: "object", properties: {}, additionalProperties: false },
      },
      /require "type" as a constant string/,
    ],
    [
      "two members with one tag",
      {
        U: {
          type: "object",
          oneOf: [{ $ref: "#/$defs/A" }, { $ref: "#/$defs/B" }],
          discriminator: { propertyName: "type" },
        },
        A: message("a"),
        B: message("a"),
      },
      /same Rust variant/,
    ],
    [
      "the reserved tag",
      {
        U: {
          type: "object",
          oneOf: [{ $ref: "#/$defs/A" }],
          discriminator: { propertyName: "type" },
        },
        A: message("unknown"),
      },
      /reserved/,
    ],
    [
      "inline union members",
      {
        U: {
          type: "object",
          oneOf: [message("a")],
          discriminator: { propertyName: "type" },
        },
      },
      /each member of oneOf is a/,
    ],
  ];
  for (const [what, $defs, message] of rejects) {
    it(`rejects ${what}`, () => {
      expect(() => collect(document("a.json", $defs))).toThrow(message);
    });
  }

  it("rejects a name defined twice", () => {
    const twice = {
      ...document("a.json", { A: { type: "string" } }),
      ...document("b.json", { A: { type: "string" } }),
    };
    expect(() => collect(twice)).toThrow(/already defined in a.json/);
  });
  it("rejects a member shared by two unions", () => {
    const shared = document("a.json", {
      U: {
        type: "object",
        oneOf: [{ $ref: "#/$defs/A" }],
        discriminator: { propertyName: "type" },
      },
      V: {
        type: "object",
        oneOf: [{ $ref: "#/$defs/A" }],
        discriminator: { propertyName: "type" },
      },
      A: message("a"),
    });
    expect(() => collect(shared)).toThrow(/already a member of U/);
  });
});

describe("typescript", () => {
  it("writes interfaces, unions and a definition map", () => {
    const out = typescript(collect(device));
    expect(out).toContain(
      "/** A registered device. */\nexport interface Device {",
    );
    expect(out).toContain(
      "  lastSeenAt: string | null;\n  channel?: string;\n  tags: string[];",
    );
    expect(out).toContain(
      'export type DeviceRole = "host" | "client" | "device.revoked";',
    );
    expect(out).toContain(
      "/** An iroh public key, in hex. */\nexport type DeviceKey = string;",
    );
    expect(out).toContain(
      "export interface Definitions {\n  Device: Device;\n  DeviceKey: DeviceKey;",
    );
  });
});

describe("tagged unions", () => {
  const definitions = collect(live);

  it("are a union of their members in TypeScript, each with its tag as a literal", () => {
    const out = typescript(definitions);
    expect(out).toContain(
      "/** What the server sends. */\nexport type ServerMessage =\n  | Hello\n  | Ping;",
    );
    expect(out).toContain('export interface Ping {\n  type: "ping";\n}');
  });

  it("are internally tagged enums in Rust that skip what they do not know", () => {
    const out = rust(definitions);
    expect(out).toContain(
      '#[serde(tag = "type")]\npub enum ServerMessage {\n    #[serde(rename = "hello")]\n    Hello(Hello),\n    #[serde(rename = "ping")]\n    Ping(Ping),\n',
    );
    expect(out).toContain("    #[serde(other)]\n    Unknown,\n}");
    expect(out).toContain("pub struct Hello {\n    pub nonce: String,\n}");
    expect(out).toContain("pub struct Ping {}");
    expect(out).toContain('"Ping" => through::<ServerMessage>(json)');
  });

  it("keep their discriminator in the bundle and OpenAPI", () => {
    expect(bundle(definitions).$defs.ServerMessage).toEqual({
      description: "What the server sends.",
      type: "object",
      oneOf: [{ $ref: "#/$defs/Hello" }, { $ref: "#/$defs/Ping" }],
      discriminator: { propertyName: "type" },
    });
  });
});

describe("rust", () => {
  const out = rust(collect(device));

  it("maps optional and nullable fields to Option, skipping only the optional ones", () => {
    expect(out).toContain("    pub last_seen_at: Option<String>,\n");
    expect(out).toContain(
      '    #[serde(skip_serializing_if = "Option::is_none")]\n    pub channel: Option<String>,\n',
    );
  });

  it("leaves Eq off structs holding floats", () => {
    expect(out).toContain(
      "#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]\n#[serde(rename_all",
    );
  });

  it("gives every enum an Unknown variant for values added later", () => {
    expect(out).toContain(
      '    #[serde(rename = "device.revoked")]\n    DeviceRevoked,\n',
    );
    expect(out).toContain("    #[serde(other)]\n    Unknown,\n}");
  });

  it("keeps code within rustfmt's width, so cargo fmt leaves the file alone", () => {
    const code = out
      .split("\n")
      .filter((line) => !line.trimStart().startsWith("//"));
    for (const line of code) expect(line.length).toBeLessThanOrEqual(100);
  });
});

describe("bundle and openapi", () => {
  it("rewrites references for each document", () => {
    const definitions = collect(device);
    expect(bundle(definitions).$defs.Device.properties.key).toEqual({
      $ref: "#/$defs/DeviceKey",
    });
    const api = openapi(
      new Map([
        ...definitions,
        ...collect(document("common.json", { ApiError: { type: "string" } })),
      ]),
      {
        routes: [
          {
            method: "get",
            path: "/v1/devices/{key}",
            operationId: "getDevice",
            summary: "One device",
            auth: "session",
            responses: {
              "200": { description: "The device.", body: "Device" },
            },
          },
        ],
      },
    );
    const operation = api.paths["/v1/devices/{key}"].get;
    expect(operation.parameters).toEqual([
      { name: "key", in: "path", required: true, schema: { type: "string" } },
    ]);
    expect(
      operation.responses["200"].content["application/json"].schema,
    ).toEqual({
      $ref: "#/components/schemas/Device",
    });
    expect(api.components.schemas.Device.properties.key).toEqual({
      $ref: "#/components/schemas/DeviceKey",
    });
  });

  it("describes query parameters by their definition", () => {
    const withErrors = new Map([
      ...collect(device),
      ...collect(document("common.json", { ApiError: { type: "string" } })),
    ]);
    const api = openapi(withErrors, {
      routes: [
        {
          method: "get",
          path: "/v1/devices",
          operationId: "listDevices",
          summary: "",
          auth: "session",
          query: { role: "DeviceRole" },
          responses: {},
        },
      ],
    });
    expect(api.paths["/v1/devices"].get.parameters).toEqual([
      {
        name: "role",
        in: "query",
        required: false,
        schema: { $ref: "#/components/schemas/DeviceRole" },
      },
    ]);
  });

  const withLive = new Map([
    ...collect(live),
    ...collect(document("common.json", { ApiError: { type: "string" } })),
  ]);
  const socket = {
    method: "get",
    path: "/v1/live",
    operationId: "openLive",
    summary: "",
    auth: "device",
    upgrade: "websocket",
    messages: { send: "Hello", receive: "ServerMessage" },
    responses: { "101": { description: "Switching to a WebSocket." } },
  };

  it("describes a WebSocket route by the messages each side sends", () => {
    const operation = openapi(withLive, { routes: [socket] }).paths["/v1/live"]
      .get;
    expect(operation.security).toEqual([]);
    expect(operation["x-websocket"]).toEqual({
      send: { $ref: "#/components/schemas/Hello" },
      receive: { $ref: "#/components/schemas/ServerMessage" },
    });
    expect(operation.responses["101"]).toEqual({
      description: "Switching to a WebSocket.",
    });
  });

  it("leaves webhooks out of the session security", () => {
    const api = openapi(withLive, {
      routes: [
        {
          method: "post",
          path: "/v1/webhooks/clerk",
          operationId: "receiveClerkWebhook",
          summary: "",
          auth: "webhook",
          responses: { "204": { description: "Done." } },
        },
      ],
    });
    expect(api.paths["/v1/webhooks/clerk"].post.security).toEqual([]);
  });

  it("refuses a WebSocket route that is not a get, or names no messages", () => {
    expect(() =>
      openapi(withLive, { routes: [{ ...socket, method: "post" }] }),
    ).toThrow(/on a get/);
    expect(() =>
      openapi(withLive, { routes: [{ ...socket, messages: undefined }] }),
    ).toThrow(/names the messages/);
    expect(() =>
      openapi(withLive, {
        routes: [{ ...socket, upgrade: undefined, auth: "session" }],
      }),
    ).toThrow(/only WebSocket routes have messages/);
  });

  it("refuses an unknown kind of auth", () => {
    expect(() =>
      openapi(withLive, { routes: [{ ...socket, auth: "cookie" }] }),
    ).toThrow(
      /auth is one of none, signedIn, session, sessionOrHost, host, device, webhook/,
    );
  });

  it("marks a request body the caller may leave out", () => {
    const withErrors = new Map([
      ...collect(device),
      ...collect(document("common.json", { ApiError: { type: "string" } })),
    ]);
    const route = {
      method: "post",
      path: "/v1/devices",
      operationId: "registerDevice",
      summary: "",
      auth: "session",
      request: "Device",
      responses: {},
    };
    const body = (extra: object) =>
      openapi(withErrors, { routes: [{ ...route, ...extra }] }).paths[
        "/v1/devices"
      ].post.requestBody.required;
    expect(body({})).toBe(true);
    expect(body({ requestOptional: true })).toBe(false);
    expect(() =>
      openapi(withErrors, {
        routes: [{ ...route, request: undefined, requestOptional: true }],
      }),
    ).toThrow(/requestOptional needs a request/);
  });

  it("refuses routes outside /v1", () => {
    expect(() =>
      openapi(collect(device), {
        routes: [
          {
            method: "get",
            path: "/devices",
            operationId: "x",
            summary: "",
            auth: "none",
            responses: {},
          },
        ],
      }),
    ).toThrow(/versioned under \/v1/);
  });
});
