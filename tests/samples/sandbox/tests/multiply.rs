use sandbox::multiply;

#[test]
fn three_times_four_is_twelve() {
    assert_eq!(multiply(3, 4), 12);
}

#[test]
fn negatives_multiply_to_positive() {
    assert_eq!(multiply(-2, -5), 10);
}

#[test]
fn anything_times_zero_is_zero() {
    assert_eq!(multiply(7, 0), 0);
    assert_eq!(multiply(0, 7), 0);
}
