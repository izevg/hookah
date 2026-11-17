//! Money amounts in cents.

/// An amount of money in cents.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Money(pub i64);

impl Money {
    /// Sum of all amounts.
    pub fn total(items: &[Money]) -> Money {
        Money(items.iter().map(|m| m.0).sum())
    }

    /// Formats the amount as `dollars.cents`.
    pub fn display(self) -> String {
        format!("{}.{:02}", self.0 / 100, self.0 % 100)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn total_adds_up() {
        assert_eq!(Money::total(&[Money(100), Money(250)]), Money(350));
    }

    #[test]
    fn display_pads_cents() {
        assert_eq!(Money(105).display(), "1.05");
    }
}
