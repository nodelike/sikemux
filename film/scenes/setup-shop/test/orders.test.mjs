import { test } from "node:test";
import assert from "node:assert/strict";

const next = { cart: "placed", placed: "paid", paid: "shipped" };

test("an order moves from cart to shipped", () => {
  let status = "cart";
  while (next[status]) status = next[status];
  assert.equal(status, "shipped");
});
test("a shipped order goes no further", () => assert.equal(next.shipped, undefined));
