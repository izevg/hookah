package shop.app;

import shop.Money;
import shop.shared.Labels;

/** Prints a total. */
public final class Receipt {
  private Receipt() {}

  /** The line shown under the items. */
  public static String totalLine(Money total) {
    return Labels.total() + ": " + total.format();
  }
}
