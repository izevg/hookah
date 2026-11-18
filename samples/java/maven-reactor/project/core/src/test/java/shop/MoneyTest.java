package shop;

import static org.junit.jupiter.api.Assertions.assertEquals;

import org.junit.jupiter.api.Test;

class MoneyTest {
  @Test
  void addsAmounts() {
    assertEquals(new Money(300), new Money(100).plus(new Money(200)));
  }

  @Test
  void formatsWithTwoDecimals() {
    assertEquals("12.05", new Money(1205).format());
  }
}
