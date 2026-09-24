//! Spend, cost, run classification and context accounting. WP1.5 moves them
//! here from `runner.rs`; the turn's token ledger is declared here now.

/// The tokens a turn has used across its calls.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct TokenLedger {
    pub input: u64,
    pub output: u64,
    pub cache_read: u64,
    pub cache_write: u64,
}

impl TokenLedger {
    /// Add one call's usage.
    pub fn add(&mut self, u: &ai::UsageInfo) {
        let n = |v: i32| u64::try_from(v).unwrap_or(0);
        self.input += n(u.input_tokens);
        self.output += n(u.output_tokens);
        self.cache_read += n(u.cache_read_input_tokens);
        self.cache_write += n(u.cache_creation_input_tokens);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ledger_sums_calls_and_ignores_negative_counts() {
        let mut ledger = TokenLedger::default();
        let call = ai::UsageInfo {
            input_tokens: 100,
            output_tokens: 20,
            cache_read_input_tokens: 80,
            cache_creation_input_tokens: 5,
            ..Default::default()
        };
        ledger.add(&call);
        ledger.add(&call);
        ledger.add(&ai::UsageInfo {
            input_tokens: -1,
            ..Default::default()
        });
        assert_eq!(
            ledger,
            TokenLedger {
                input: 200,
                output: 40,
                cache_read: 160,
                cache_write: 10
            }
        );
    }
}
