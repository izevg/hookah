package shop;

import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.junit.jupiter.api.Assertions.assertFalse;
import static org.junit.jupiter.api.Assertions.assertThrows;
import static org.junit.jupiter.api.Assertions.assertTrue;

import org.junit.jupiter.api.Test;

class CartTest {
  @Test
  void totalsItems() {
    Cart cart = new Cart();
    cart.add(250);
    cart.add(750);
    assertEquals(1000, cart.total());
  }

  @Test
  void acceptsFreeItems() {
    Cart cart = new Cart();
    cart.add(0);
    assertEquals(0, cart.total());
  }

  @Test
  void rejectsNegativePrices() {
    assertThrows(IllegalArgumentException.class, () -> new Cart().add(-1));
  }

  @Test
  void freeShippingStartsAtFiftyUnits() {
    Cart cart = new Cart();
    cart.add(4999);
    assertFalse(cart.freeShipping());
    cart.add(1);
    assertTrue(cart.freeShipping());
  }
}
