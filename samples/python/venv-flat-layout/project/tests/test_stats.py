from tally.stats import mean, spread


def test_mean() -> None:
    assert mean([1, 2, 6]) == 3
    assert mean([]) == 0


def test_spread() -> None:
    assert spread([4, 9, 1]) == 8
