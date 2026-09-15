// Copyright 2026 Nikola Milojević.
// SPDX-License-Identifier: Apache-2.0
//! Exact nanounit arithmetic for resource summaries and downward-API divisors.

pub(super) fn parse(input: &str) -> Option<i128> {
    let split = input
        .char_indices()
        .find(|&(i, c)| i > 0 && !(c.is_ascii_digit() || c == '.'))
        .map_or(input.len(), |(i, _)| i);
    let (number, suffix) = input.split_at(split);
    let negative = number.starts_with('-');
    let number = number.trim_start_matches(['-', '+']);
    let (whole, fraction) = number.split_once('.').unwrap_or((number, ""));
    let digits = format!("{whole}{fraction}");
    if digits.is_empty() {
        return None;
    }
    let mut value = digits.parse::<i128>().ok()?;
    let (power, binary) = match suffix {
        "" => (0, 0),
        "n" => (-9, 0),
        "u" => (-6, 0),
        "m" => (-3, 0),
        "k" | "K" => (3, 0),
        "M" => (6, 0),
        "G" => (9, 0),
        "T" => (12, 0),
        "P" => (15, 0),
        "E" => (18, 0),
        "Ki" => (0, 10),
        "Mi" => (0, 20),
        "Gi" => (0, 30),
        "Ti" => (0, 40),
        "Pi" => (0, 50),
        "Ei" => (0, 60),
        _ if suffix.starts_with(['e', 'E']) => (suffix[1..].parse::<i32>().ok()?, 0),
        _ => return None,
    };
    value = value.checked_mul(1i128.checked_shl(binary)?)?;
    let scale = power
        .checked_add(9)?
        .checked_sub(i32::try_from(fraction.len()).ok()?)?;
    if scale >= 0 {
        value = value.checked_mul(10i128.checked_pow(scale as u32)?)?;
    } else {
        let divisor = 10i128.checked_pow(scale.unsigned_abs())?;
        value = value / divisor + i128::from(value % divisor != 0);
    }
    if negative {
        value.checked_neg()
    } else {
        Some(value)
    }
}

pub(super) fn canonical(input: &str) -> String {
    let Some(nanos) = parse(input) else {
        return input.into();
    };
    if nanos == 0 {
        return "0".into();
    }
    if input.ends_with('i') && nanos.abs() >= 1_024_000_000_000 && nanos % 1_000_000_000 == 0 {
        let mut units = nanos / 1_000_000_000;
        let mut index = 0;
        let suffixes = ["", "Ki", "Mi", "Gi", "Ti", "Pi", "Ei"];
        while index + 1 < suffixes.len() && units % 1024 == 0 {
            units /= 1024;
            index += 1;
        }
        return format!("{units}{}", suffixes[index]);
    }
    let mut value = nanos;
    let mut index = 0;
    let suffixes = ["n", "u", "m", "", "k", "M", "G", "T", "P", "E"];
    while index + 1 < suffixes.len() && value % 1000 == 0 {
        value /= 1000;
        index += 1;
    }
    if input.contains('e') || (input.contains('E') && !input.ends_with('E')) {
        let exponent = index as i32 * 3 - 9;
        if exponent == 0 {
            value.to_string()
        } else {
            format!("{value}e{exponent}")
        }
    } else {
        format!("{value}{}", suffixes[index])
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn exact_resources_and_subnanounit_rounding() {
        for (input, expected) in [
            ("0.5", 500_000_000),
            ("500m", 500_000_000),
            ("1.5Gi", 1_610_612_736_000_000_000),
            ("1e3", 1_000_000_000_000),
            ("0.1n", 1),
            ("-0.1n", -1),
        ] {
            assert_eq!(parse(input), Some(expected));
        }
        for (input, expected) in [
            ("0.5", "500m"),
            ("1.5Gi", "1536Mi"),
            ("1000", "1k"),
            ("1000m", "1"),
            ("0.9765625Ki", "1k"),
            ("1e0", "1"),
            ("1000e-3", "1"),
            ("0.1n", "1n"),
        ] {
            assert_eq!(canonical(input), expected);
        }
    }
}
