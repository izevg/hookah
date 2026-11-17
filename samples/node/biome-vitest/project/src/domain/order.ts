import { applyDiscount } from "./discount";

export interface Line {
  cents: number;
  quantity: number;
}

/** Sums the lines and applies one discount to the whole order. */
export function orderTotal(lines: Line[], percent: number): number {
  const sum = lines.reduce((acc, line) => acc + line.cents * line.quantity, 0);
  return applyDiscount(sum, percent);
}
