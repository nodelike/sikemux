import { test } from "node:test";
import assert from "node:assert/strict";
import { cartTotal, mergeLines } from "../src/cart.mjs";

const mug = { variantId: "mug-blue", priceCents: 1800, quantity: 2 };
const tee = { variantId: "tee-m", priceCents: 2500, quantity: 1 };

test("adds up every line", () => assert.equal(cartTotal([mug, tee]), 6100));
test("applies tax after the discount", () => assert.equal(cartTotal([mug], { taxRate: 0.2, discountCents: 600 }), 3600));
test("never goes below zero", () => assert.equal(cartTotal([tee], { discountCents: 9000 }), 0));
test("merges lines for the same variant", () => assert.deepEqual(mergeLines([mug, mug]), [{ ...mug, quantity: 4 }]));
test("keeps different variants apart", () => assert.equal(mergeLines([mug, tee]).length, 2));
