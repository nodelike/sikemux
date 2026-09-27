import { DEMO_HOME } from "./projects";

export const BRUNO_COLLECTION = `${DEMO_HOME}/api/acme-store`;

const request = (
  name: string,
  method: string,
  url: string,
  seq: number,
  body = "",
) =>
  `meta {\n  name: ${name}\n  type: http\n  seq: ${seq}\n}\n\n${method} {\n  url: ${url}\n  body: ${body ? "json" : "none"}\n  auth: bearer\n}\n\nauth:bearer {\n  token: {{token}}\n}\n${body ? `\nbody:json {\n${body}\n}\n` : ""}`;

const CHECKOUT_BODY = `  {\n    "cartId": "cart_8f2a91",\n    "currency": "EUR",\n    "shipping": "express"\n  }`;

export const BRUNO_FILES: Record<string, string> = {
  [`${BRUNO_COLLECTION}/bruno.json`]:
    '{ "version": "1", "name": "Acme Store API", "type": "collection" }',
  [`${BRUNO_COLLECTION}/environments/production.bru`]:
    "vars {\n  baseUrl: https://api.acme.dev\n  token: sk_live_demo\n}\n",
  [`${BRUNO_COLLECTION}/environments/staging.bru`]:
    "vars {\n  baseUrl: https://staging.api.acme.dev\n  token: sk_test_demo\n}\n",
  [`${BRUNO_COLLECTION}/health.bru`]: request(
    "Health",
    "get",
    "{{baseUrl}}/health",
    1,
  ),
  [`${BRUNO_COLLECTION}/catalog/folder.bru`]: "meta {\n  name: Catalog\n}\n",
  [`${BRUNO_COLLECTION}/catalog/search.bru`]: request(
    "Search products",
    "get",
    "{{baseUrl}}/v1/search?q=linen&limit=12",
    1,
  ),
  [`${BRUNO_COLLECTION}/catalog/product.bru`]: request(
    "Get product",
    "get",
    "{{baseUrl}}/v1/products/prd_31c8",
    2,
  ),
  [`${BRUNO_COLLECTION}/checkout/folder.bru`]: "meta {\n  name: Checkout\n}\n",
  [`${BRUNO_COLLECTION}/checkout/create.bru`]: request(
    "Create checkout",
    "post",
    "{{baseUrl}}/v1/checkout",
    1,
    CHECKOUT_BODY,
  ),
  [`${BRUNO_COLLECTION}/checkout/status.bru`]: request(
    "Checkout status",
    "get",
    "{{baseUrl}}/v1/checkout/chk_4411",
    2,
  ),
  [`${BRUNO_COLLECTION}/users/folder.bru`]: "meta {\n  name: Users\n}\n",
  [`${BRUNO_COLLECTION}/users/me.bru`]: request(
    "Current user",
    "get",
    "{{baseUrl}}/v1/users/me",
    1,
  ),
};

export function brunoDir(path: string) {
  const names = new Set<string>();
  for (const file of Object.keys(BRUNO_FILES)) {
    if (file.startsWith(`${path}/`))
      names.add(file.slice(path.length + 1).split("/")[0]);
  }
  return [...names].sort().map((name) => ({
    name,
    path: `${path}/${name}`,
    is_dir: !BRUNO_FILES[`${path}/${name}`],
  }));
}

export const CHECKOUT_RESPONSE = {
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
