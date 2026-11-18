import { expect, test } from "vitest";
import { label, plural } from "./format.js";

test("caps big counts", () => {
  expect(label(120)).toBe("99+");
});

test("pluralizes", () => {
  expect(plural(2, "item")).toBe("items");
});
