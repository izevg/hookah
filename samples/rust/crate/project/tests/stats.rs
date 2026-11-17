use stats::{clamp, describe, mean, shorten};

#[test]
fn mean_of_values() {
    assert_eq!(mean(&[1.0, 2.0, 3.0]), Some(2.0));
}

#[test]
fn mean_of_nothing() {
    assert_eq!(mean(&[]), None);
}

#[test]
fn clamp_limits_both_ends() {
    assert_eq!(clamp(-5, 0, 10), 0);
    assert_eq!(clamp(50, 0, 10), 10);
    assert_eq!(clamp(5, 0, 10), 5);
}

#[test]
fn describe_pluralizes() {
    assert_eq!(describe(1), "1 item");
    assert_eq!(describe(3), "3 items");
}

#[test]
fn shorten_cuts_long_text() {
    assert_eq!(shorten("hookah", 4), "hook");
}
