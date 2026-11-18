package shop;

/** An amount in cents. */
public record Money(int cents) {

  /** Adds two amounts. */
  public Money plus(Money other) {
    return new Money(cents + other.cents);
  }

  /** The amount as text, such as {@code 12.50}. */
  public String format() {
    return String.format("%d.%02d", cents / 100, cents % 100);
  }
}
