"""Descriptive statistics."""


def mean(values: list[float]) -> float:
    """Return the arithmetic mean; an empty list has mean 0."""
    if not values:
        return 0.0
    return sum(values) / len(values)


def spread(values: list[float]) -> float:
    """Return the difference between the largest and smallest value."""
    if not values:
        return 0.0
    return max(values) - min(values)
