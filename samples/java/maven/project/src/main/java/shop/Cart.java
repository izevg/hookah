package shop;

import java.util.ArrayList;
import java.util.List;

/** A shopping cart that totals its items in cents. */
public final class Cart {
  private final List<Integer> prices = new ArrayList<>();

  /** Adds an item priced in cents. */
  public void add(int cents) {
    if (cents < 0) {
      throw new IllegalArgumentException("negative price");
    }
    prices.add(cents);
  }

  /** The sum of all prices. */
  public int total() {
    int sum = 0;
    for (int price : prices) {
      sum += price;
    }
    return sum;
  }

  /** Whether the cart qualifies for free shipping (at least 5000 cents). */
  public boolean freeShipping() {
    return total() >= FREE_SHIPPING_FROM;
  }

  private static final int FREE_SHIPPING_FROM = 5000;
}
