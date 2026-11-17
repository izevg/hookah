/** Applies a percentage discount to a price in cents, clamping the rate to 0..100. */
export function applyDiscount(cents: number, percent: number): number {
  if (percent < 0) {
    return cents;
  }
  if (percent > 100) {
    return 0;
  }
  return Math.round(cents * (1 - percent / 100));
}
