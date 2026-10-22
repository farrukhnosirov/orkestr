import assert from "node:assert/strict";
import { test } from "node:test";

import { greeter } from "../src/greeter.mjs";

test("greeter greets by name", () => {
  assert.equal(greeter("Orkestr"), "Hello, Orkestr!");
});

test("greeter has a default", () => {
  assert.equal(greeter(), "Hello, world!");
});
