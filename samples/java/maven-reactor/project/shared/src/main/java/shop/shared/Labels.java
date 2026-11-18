package shop.shared;

/** Texts shown to customers. It does not depend on any other module. */
public final class Labels {
  private Labels() {}

  /** The word in front of an order total. */
  public static String total() {
    return "Total";
  }
}
