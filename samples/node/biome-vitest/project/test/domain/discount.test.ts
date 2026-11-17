import { expect, test } from "vitest";
import { applyDiscount } from "../../src/domain/discount";

test("takes a percentage off", () => {
  expect(applyDiscount(1000, 25)).toBe(750);
});

test("a negative rate changes nothing", () => {
  expect(applyDiscount(1000, -5)).toBe(1000);
});

test("a rate above 100 makes it free", () => {
  expect(applyDiscount(1000, 150)).toBe(0);
});

test("zero and full rates", () => {
  expect(applyDiscount(1000, 0)).toBe(1000);
  expect(applyDiscount(1000, 100)).toBe(0);
});
