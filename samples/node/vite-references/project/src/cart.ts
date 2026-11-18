import { cents } from "../lib/money";

export function total(prices: number[]): number {
  return prices.reduce((sum, price) => sum + cents(price), 0);
}
