"""Cart pricing."""


def discount(total: int) -> int:
    """Return the discount in cents: 10 percent for carts of 100 or more."""
    if total >= 100:
        return total // 10
    return 0


def payable(total: int) -> int:
    """Return what the customer pays, in cents."""
    return total - discount(total)
