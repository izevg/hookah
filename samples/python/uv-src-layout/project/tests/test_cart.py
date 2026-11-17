from shop.domain.cart import discount, payable


def test_discount_from_threshold() -> None:
    assert discount(100) == 10
    assert discount(99) == 0


def test_payable() -> None:
    assert payable(200) == 180
    assert payable(50) == 50
