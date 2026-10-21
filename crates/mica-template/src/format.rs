// SPDX-License-Identifier: AGPL-3.0-only
// Provenance: adapted
// Source: prusaslicer@b028299c77 src/libslic3r/PlaceholderParser.cpp :: expr::to_string
// Source: prusaslicer@b028299c77 src/libslic3r/PlaceholderParser.cpp :: expr::digits
// Source: prusaslicer@b028299c77 src/libslic3r/Config.cpp :: escape_string_cstyle
// Changes: Re-implements the C++ stream / printf number formatting that PrusaSlicer relies on
// Changes: (`std::ostringstream << double`, `%*.*lf`, `%0*d`) on top of Rust's exact float formatting.

//! Number and string formatting that reproduces the C++ behaviour PrusaSlicer's templates depend on.

/// Formats a double the way `std::ostringstream << double` does with default stream settings,
/// i.e. like C `printf("%g")` with 6 significant digits.
///
/// This is how PrusaSlicer turns every floating point template result (and every float config
/// value expanded with the legacy `[name]` syntax) into text:
///
/// ```
/// use mica_template::format_double;
/// assert_eq!(format_double(0.036), "0.036");
/// assert_eq!(format_double(115.0), "115");
/// assert_eq!(format_double(1234567.0), "1.23457e+06");
/// assert_eq!(format_double(0.1 + 0.2), "0.3");
/// assert_eq!(format_double(0.00001), "1e-05");
/// ```
pub fn format_double(v: f64) -> String {
    const PRECISION: i32 = 6;
    if v.is_nan() {
        return if v.is_sign_negative() { "-nan" } else { "nan" }.to_string();
    }
    if v.is_infinite() {
        return if v < 0.0 { "-inf" } else { "inf" }.to_string();
    }
    if v == 0.0 {
        return if v.is_sign_negative() { "-0" } else { "0" }.to_string();
    }
    // %g picks its style from the decimal exponent *after* rounding to PRECISION significant digits,
    // which is exactly what Rust's `{:.Pe}` computes (correctly rounded, ties to even like glibc/libc++).
    let sci = format!("{:.*e}", (PRECISION - 1) as usize, v);
    let (mantissa, exp) = sci.split_once('e').unwrap_or((sci.as_str(), "0"));
    let exp: i32 = exp.parse().unwrap_or(0);
    if !(-4..PRECISION).contains(&exp) {
        let mantissa = strip_trailing_zeros(mantissa);
        let sign = if exp < 0 { '-' } else { '+' };
        format!("{mantissa}e{sign}{:02}", exp.unsigned_abs())
    } else {
        let decimals = (PRECISION - 1 - exp) as usize;
        strip_trailing_zeros(&format!("{v:.decimals$}")).to_string()
    }
}

/// Removes trailing zeros after a decimal point, and the point itself if nothing follows it.
fn strip_trailing_zeros(s: &str) -> &str {
    if s.contains('.') {
        s.trim_end_matches('0').trim_end_matches('.')
    } else {
        s
    }
}

/// C `printf("%*.*f")` / `printf("%0*.*f")` (the `digits()` / `zdigits()` template functions).
pub(crate) fn printf_fixed(v: f64, width: usize, decimals: usize, zero_pad: bool) -> String {
    if !v.is_finite() {
        // printf never zero-pads inf / nan.
        let s = if v.is_nan() {
            if v.is_sign_negative() { "-nan" } else { "nan" }
        } else if v < 0.0 {
            "-inf"
        } else {
            "inf"
        };
        return format!("{s:>width$}");
    }
    if zero_pad {
        format!("{v:0width$.decimals$}")
    } else {
        format!("{v:>width$.decimals$}")
    }
}

/// C `printf("%*d")` / `printf("%0*d")`.
pub(crate) fn printf_int(v: i64, width: usize, zero_pad: bool) -> String {
    if zero_pad {
        format!("{v:0width$}")
    } else {
        format!("{v:>width$}")
    }
}

/// PrusaSlicer's `escape_string_cstyle`: used when a scalar string option is expanded with the
/// legacy `[name]` syntax (it goes through `ConfigOptionString::serialize`).
pub(crate) fn escape_string_cstyle(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '\r' => out.push_str("\\r"),
            '\n' => out.push_str("\\n"),
            '\\' => out.push_str("\\\\"),
            c => out.push(c),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn g_format_matches_ostream() {
        let cases: &[(f64, &str)] = &[
            (0.036, "0.036"),
            (115.0, "115"),
            (1234567.0, "1.23457e+06"),
            (0.1 + 0.2, "0.3"),
            (0.4, "0.4"),
            (100000.0, "100000"),
            (999999.0, "999999"),
            (999999.5, "1e+06"),
            (1e6, "1e+06"),
            (123456.7, "123457"),
            (0.0001, "0.0001"),
            (0.00001, "1e-05"),
            (0.000123456789, "0.000123457"),
            (1.5e-7, "1.5e-07"),
            (1e100, "1e+100"),
            (-2.5, "-2.5"),
            (-0.0, "-0"),
            (0.0, "0"),
            (5500.0, "5500"),
            (13.84375892476, "13.8438"),
            (1.0 / 3.0, "0.333333"),
            (2.0 / 3.0, "0.666667"),
            (1234565.0, "1.23456e+06"), // exact tie rounds to even, like printf
            (0.5, "0.5"),
            (21.0, "21"),
            (f64::INFINITY, "inf"),
            (f64::NEG_INFINITY, "-inf"),
            (f64::NAN, "nan"),
        ];
        for &(v, expected) in cases {
            assert_eq!(format_double(v), expected, "formatting {v:e}");
        }
    }

    #[test]
    fn printf_like() {
        assert_eq!(printf_int(5, 15, false), "              5");
        assert_eq!(printf_int(5, 15, true), "000000000000005");
        assert_eq!(printf_int(-5, 5, true), "-0005");
        assert_eq!(printf_int(12345, 2, true), "12345");
        assert_eq!(printf_fixed(5.0, 15, 8, false), "     5.00000000");
        assert_eq!(printf_fixed(5.0, 15, 8, true), "000005.00000000");
        assert_eq!(printf_fixed(-1.5, 8, 3, true), "-001.500");
        assert_eq!(printf_fixed(0.125, 0, 2, false), "0.12");
        assert_eq!(printf_fixed(2.5, 0, 0, false), "2");
        assert_eq!(printf_fixed(f64::INFINITY, 5, 2, true), "  inf");
    }

    #[test]
    fn cstyle_escape() {
        assert_eq!(escape_string_cstyle("a\nb\\c\rd\"e"), "a\\nb\\\\c\\rd\"e");
    }
}
