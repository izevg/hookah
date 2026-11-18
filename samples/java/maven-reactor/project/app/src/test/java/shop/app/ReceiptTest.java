package shop.app;

import static org.junit.jupiter.api.Assertions.assertEquals;

import org.junit.jupiter.api.Test;
import shop.Money;
import shop.shared.Fixtures;

class ReceiptTest {
  @Test
  void showsTheTotal() {
    assertEquals("Total: 12.05", Receipt.totalLine(new Money(Fixtures.twelveOhFive())));
  }
}
