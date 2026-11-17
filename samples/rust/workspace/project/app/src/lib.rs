//! Receipt printing on top of `ledger-core`.

use ledger_core::Money;

/// Renders a one-line receipt for prices given in cents.
pub fn receipt(prices: &[i64]) -> String {
    let items: Vec<Money> = prices.iter().map(|&cents| Money(cents)).collect();
    format!("total: {}", Money::total(&items).display())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn receipt_shows_total() {
        assert_eq!(receipt(&[199, 1]), "total: 2.00");
    }
}
