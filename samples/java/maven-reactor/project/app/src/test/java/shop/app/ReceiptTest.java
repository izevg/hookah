package shop.app;

import static org.junit.jupiter.api.Assertions.assertEquals;

import org.junit.jupiter.api.Test;
import shop.Money;

class ReceiptTest {
  @Test
  void showsTheTotal() {
    assertEquals("Total: 12.05", Receipt.totalLine(new Money(1205)));
  }
}
