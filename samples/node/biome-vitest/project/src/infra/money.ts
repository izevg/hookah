/** Renders cents as a decimal string such as `12.50`. */
export function formatMoney(cents: number): string {
  return (cents / 100).toFixed(2);
}
