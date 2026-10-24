import { DEMO_HOME } from "./projects";

export const BRUNO_COLLECTION = `${DEMO_HOME}/api/acme-store`;

type Args = Record<string, unknown>;

const NO_AUTH = { username: "", password: "" };
const NO_KEY = { key: "", value: "", placement: "header" };

const request = (name: string, method: string, url: string, body = "") => ({
  meta: { name, type: "http", seq: null },
  method,
  url,
  params: { query: [], path: [] },
  headers: [],
  body: {
    mode: body ? "json" : "none",
    text: body,
    graphql: { query: "", variables: "" },
    form: [],
  },
  auth: {
    mode: "bearer",
    bearer: { token: "{{token}}" },
    basic: NO_AUTH,
    apikey: NO_KEY,
  },
  vars: { req: [], res: [] },
  scripts: { pre: "", post: "" },
  assertions: [],
  tests: "",
  docs: "",
  settings: { encodeUrl: null, timeout: null },
  extra: [],
});

const folder = (name: string) => ({
  meta: { name, type: "", seq: null },
  auth: {
    mode: "inherit",
    bearer: { token: "" },
    basic: NO_AUTH,
    apikey: NO_KEY,
  },
  headers: [],
  vars: { req: [], res: [] },
  scripts: { pre: "", post: "" },
  extra: [],
});

const CHECKOUT_BODY = `{\n  "cartId": "cart_8f2a91",\n  "currency": "EUR",\n  "shipping": "express"\n}`;

/** Each request by its file, with the folder it sits in. */
const REQUESTS: Record<string, [string | null, ReturnType<typeof request>]> = {
  "health.bru": [null, request("Health", "get", "{{baseUrl}}/health")],
  "catalog/search.bru": [
    "Catalog",
    request("Search products", "get", "{{baseUrl}}/v1/search?q=linen&limit=12"),
  ],
  "catalog/product.bru": [
    "Catalog",
    request("Get product", "get", "{{baseUrl}}/v1/products/prd_31c8"),
  ],
  "checkout/create.bru": [
    "Checkout",
    request(
      "Create checkout",
      "post",
      "{{baseUrl}}/v1/checkout",
      CHECKOUT_BODY,
    ),
  ],
  "checkout/status.bru": [
    "Checkout",
    request("Checkout status", "get", "{{baseUrl}}/v1/checkout/chk_4411"),
  ],
  "users/me.bru": [
    "Users",
    request("Current user", "get", "{{baseUrl}}/v1/users/me"),
  ],
};

const row = (file: string) => {
  const [, made] = REQUESTS[file];
  return {
    type: "request",
    name: made.meta.name,
    path: `${BRUNO_COLLECTION}/${file}`,
    method: made.method,
    collectionPath: BRUNO_COLLECTION,
  };
};

const env = (name: string, baseUrl: string, token: string) => ({
  id: `${BRUNO_COLLECTION}::${name}`,
  name,
  collectionPath: BRUNO_COLLECTION,
  collectionName: "acme-store",
  vars: [
    { name: "baseUrl", value: baseUrl, enabled: true },
    { name: "token", value: token, enabled: true },
  ],
  secretNames: [],
});

/** What the Bruno plugin's views would serve for the demo collection. */
export const BRUNO_VIEWS: Record<string, (params: Args) => unknown> = {
  collectionView: () => ({
    rootPath: BRUNO_COLLECTION,
    name: "Acme Store API",
    envs: [
      env("production", "https://api.acme.dev", "sk_live_demo"),
      env("staging", "https://staging.api.acme.dev", "sk_test_demo"),
    ],
    tree: [
      {
        type: "folder",
        name: "Catalog",
        path: `${BRUNO_COLLECTION}/catalog`,
        children: [row("catalog/search.bru"), row("catalog/product.bru")],
      },
      {
        type: "folder",
        name: "Checkout",
        path: `${BRUNO_COLLECTION}/checkout`,
        children: [row("checkout/create.bru"), row("checkout/status.bru")],
      },
      {
        type: "folder",
        name: "Users",
        path: `${BRUNO_COLLECTION}/users`,
        children: [row("users/me.bru")],
      },
      row("health.bru"),
    ],
  }),
  run: () => checkoutRun(),
  requestView: ({ path }) => {
    const file = String(path).slice(BRUNO_COLLECTION.length + 1);
    const [inside, made] = REQUESTS[file] ?? [
      null,
      request("Untitled", "get", ""),
    ];
    return {
      request: made,
      collectionPath: BRUNO_COLLECTION,
      scopes: inside ? [folder(inside)] : [],
    };
  },
};

const checkoutRun = () => ({
  response: {
    status: CHECKOUT_RESPONSE.status,
    statusText: CHECKOUT_RESPONSE.status_text,
    headers: CHECKOUT_RESPONSE.headers.map(([name, value]) => ({
      name,
      value,
    })),
    body: CHECKOUT_RESPONSE.body,
    isBinary: false,
    sizeBytes: CHECKOUT_RESPONSE.size_bytes,
    durationMs: CHECKOUT_RESPONSE.duration_ms,
  },
  error: null,
  sent: {
    method: "POST",
    url: "https://api.acme.dev/v1/checkout",
    headers: [{ name: "Authorization", value: "Bearer sk_live_demo" }],
  },
  logs: [],
  tests: [],
  envUpdates: {},
});

const CHECKOUT_RESPONSE = {
  status: 201,
  status_text: "Created",
  headers: [
    ["content-type", "application/json"],
    ["x-request-id", "req_7b1e2c"],
  ],
  body: JSON.stringify(
    {
      id: "chk_4411",
      status: "awaiting_payment",
      total: { amount: 12840, currency: "EUR" },
      items: 3,
      expiresAt: "2026-09-26T10:11:00Z",
    },
    null,
    2,
  ),
  is_binary: false,
  size_bytes: 214,
  duration_ms: 182,
};
