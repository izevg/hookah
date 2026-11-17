import { expect, test } from "vitest";
import { orderTotal } from "../../src/domain/order";

test("sums lines and discounts the total", () => {
  const lines = [
    { cents: 500, quantity: 2 },
    { cents: 250, quantity: 4 },
  ];
  expect(orderTotal(lines, 10)).toBe(1800);
});
