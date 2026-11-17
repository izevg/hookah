from shop.infra.store import Store


def test_place_records_the_amount() -> None:
    store = Store()
    assert store.place(200) == 180
    assert store.orders == [180]
