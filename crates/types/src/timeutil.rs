//! Shared time/duration parsing.

/// Parse a human interval like `"30m"`, `"2h"`, `"45s"`, or compound forms
/// like `"2h30m"` / `"1h30m15s"`. A trailing bare number means MINUTES
/// (heartbeat convention). Unparseable input yields a zero duration —
/// callers treat zero as invalid and skip/reject.
///
/// This is THE interval parser for heartbeat configs. The display path
/// (`nextFireAt`) and the firing path (agent worker) must agree, or the UI
/// shows fire times that never happen.
pub fn parse_duration(s: &str) -> std::time::Duration {
    let s = s.trim();
    let mut total_secs: u64 = 0;
    let mut num_buf = String::new();

    for c in s.chars() {
        if c.is_ascii_digit() {
            num_buf.push(c);
        } else {
            let n: u64 = num_buf.parse().unwrap_or(0);
            num_buf.clear();
            match c {
                'h' => total_secs += n * 3600,
                'm' => total_secs += n * 60,
                's' => total_secs += n,
                _ => {}
            }
        }
    }

    // If there's a trailing number with no unit, treat as minutes
    if !num_buf.is_empty() {
        let n: u64 = num_buf.parse().unwrap_or(0);
        total_secs += n * 60;
    }

    std::time::Duration::from_secs(total_secs)
}

#[cfg(test)]
mod tests {
    use super::parse_duration;
    use std::time::Duration;

    #[test]
    fn parses_simple_and_compound_intervals() {
        assert_eq!(parse_duration("30m"), Duration::from_secs(1800));
        assert_eq!(parse_duration("2h"), Duration::from_secs(7200));
        assert_eq!(parse_duration("45s"), Duration::from_secs(45));
        assert_eq!(parse_duration("2h30m"), Duration::from_secs(9000));
        assert_eq!(parse_duration("1h30m15s"), Duration::from_secs(5415));
        assert_eq!(parse_duration("15"), Duration::from_secs(900)); // bare = minutes
        assert_eq!(parse_duration(" 5m "), Duration::from_secs(300));
    }

    #[test]
    fn garbage_and_zero_yield_zero() {
        assert_eq!(parse_duration(""), Duration::ZERO);
        assert_eq!(parse_duration("soon"), Duration::ZERO);
        assert_eq!(parse_duration("0m"), Duration::ZERO);
    }
}
