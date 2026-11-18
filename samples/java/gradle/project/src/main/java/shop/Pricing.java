package shop;

/** Discount rules. */
public final class Pricing {
  private Pricing() {}

  /** Applies a percentage discount, rounding down. */
  public static int discount(int cents, int percent) {
    return cents - cents * percent / 100;
  }
}
