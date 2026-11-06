use humansize::{DECIMAL, FormatSizeOptions, format_size};

/// Decimal units with one decimal place, e.g. `28.4 GB`. Cleanup confirmation
/// strings are compared against this exact rendering.
pub fn bytes(n: u64) -> String {
    if n < 1000 {
        return format!("{n} B");
    }
    format_size(
        n,
        FormatSizeOptions::from(DECIMAL)
            .decimal_places(1)
            .decimal_zeroes(1),
    )
}

pub fn days(d: i64) -> String {
    match d {
        d if d >= 365 => format!("{}y", d / 365),
        d if d >= 60 => format!("{}mo", d / 30),
        d => format!("{d}d"),
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn formats_decimal_one_place() {
        assert_eq!(super::bytes(28_400_000_000), "28.4 GB");
        assert_eq!(super::bytes(1_000), "1.0 kB");
        assert_eq!(super::bytes(0), "0 B");
    }
}
