import { round } from "./money.ts";

export function total(prices: number[]): number {
  return round(prices.reduce((sum, price) => sum + price, 0));
}
