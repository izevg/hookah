package shop;

import static org.junit.jupiter.api.Assertions.assertEquals;

import org.junit.jupiter.api.Test;

/** Slow by design: failsafe runs it in `verify`, hookah never does. */
class CartIT {
  @Test
  void bigCart() {
    Cart cart = new Cart();
    cart.add(1);
    assertEquals(1, cart.total());
  }
}
