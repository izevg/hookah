"""An in-memory order store."""

from shop.domain.cart import payable


class Store:
    """Keeps the amounts of placed orders."""

    def __init__(self) -> None:
        self.orders: list[int] = []

    def place(self, total: int) -> int:
        amount = payable(total)
        self.orders.append(amount)
        return amount
