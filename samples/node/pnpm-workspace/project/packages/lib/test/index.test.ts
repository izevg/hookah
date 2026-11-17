import { expect, test } from "vitest";
import { greet } from "../src/index";

test("greets by name", () => {
  expect(greet({ name: "Ada" })).toBe("Hello, Ada!");
});
