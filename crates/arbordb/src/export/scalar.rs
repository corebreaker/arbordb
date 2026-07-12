//! Scalar leaf rendering shared by the JSON and YAML writers — the single place
//! that fixes each scalar's textual form, and the only lossy step of an export.
//!
//! Numbers and booleans are emitted verbatim; `null` covers null and the
//! non-finite floats (which neither format can spell as a number); everything
//! else is double-quoted — dates and times as ISO 8601 / RFC 3339, a UUID
//! hyphenated, raw bytes as Base64, a duration as its (possibly fractional)
//! seconds count, a rational as `num/den`.

use super::{base64, string::quote};
use crate::data::Scalar;

use chrono::TimeDelta;
use std::fmt::{Display, Write};

/// Appends `scalar`'s rendering to `out`.
pub(super) fn write_scalar(out: &mut String, scalar: &Scalar) {
    match scalar {
        Scalar::Null => out.push_str("null"),
        Scalar::Bool(true) => out.push_str("true"),
        Scalar::Bool(false) => out.push_str("false"),

        Scalar::I8(v) => number(out, v),
        Scalar::I16(v) => number(out, v),
        Scalar::I32(v) => number(out, v),
        Scalar::I64(v) => number(out, v),
        Scalar::I128(v) => number(out, v),
        Scalar::U8(v) => number(out, v),
        Scalar::U16(v) => number(out, v),
        Scalar::U32(v) => number(out, v),
        Scalar::U64(v) => number(out, v),
        Scalar::U128(v) => number(out, v),

        // The non-finite floats have no JSON/YAML number spelling.
        Scalar::F32(v) if v.is_finite() => number(out, v),
        Scalar::F32(_) => out.push_str("null"),
        Scalar::F64(v) if v.is_finite() => number(out, v),
        Scalar::F64(_) => out.push_str("null"),

        Scalar::Str(v) => quote(out, v),
        Scalar::Bytes(v) => quote(out, &base64::encode(v)),
        Scalar::Uuid(v) => quote(out, &v.to_string()),

        Scalar::Date(v) => quote(out, &v.to_string()),
        Scalar::Time(v) => quote(out, &v.to_string()),
        Scalar::DateTime(v) => quote(out, &v.to_rfc3339()),
        Scalar::Duration(v) => out.push_str(&duration_seconds(*v)),

        #[cfg(feature = "bigint-as-scalar")]
        Scalar::BigInt(v) => number(out, v),

        #[cfg(feature = "bigfloat-as-scalar")]
        Scalar::BigFloat(v) => write_big_float(out, v),

        #[cfg(feature = "rational-as-scalar")]
        Scalar::Rational(v) => quote(out, &format!("{}/{}", v.numer(), v.denom())),
    }
}

/// Appends a value's `Display` form, without an intermediate `String`.
fn number(out: &mut String, value: impl Display) {
    let _ = write!(out, "{value}");
}

/// Renders a duration as a decimal number of seconds, keeping a sub-second part
/// only when present (so a whole-second duration stays an integer).
fn duration_seconds(d: TimeDelta) -> String {
    let secs = d.num_seconds();
    let nanos = d.subsec_nanos();

    if nanos == 0 {
        return secs.to_string();
    }

    // `num_seconds` truncates toward zero and `subsec_nanos` carries the matching
    // sign, so a single leading '-' (even when the whole-seconds part is zero)
    // reconstructs the value. `TimeDelta` normalises a delta so the two parts never
    // disagree in sign (`nanos` is zero or shares `secs`' sign), hence testing
    // either being negative is enough to detect a negative duration.
    let sign = if secs < 0 || nanos < 0 { "-" } else { "" };
    let fraction = format!("{:09}", nanos.unsigned_abs());

    format!("{sign}{}.{}", secs.unsigned_abs(), fraction.trim_end_matches('0'))
}

/// A finite big float renders as its decimal literal; the special values (NaN,
/// ±∞), which num-bigfloat carries as `BigFloat`, share the float fallback to
/// `null`.
#[cfg(feature = "bigfloat-as-scalar")]
fn write_big_float(out: &mut String, v: &num_bigfloat::BigFloat) {
    if v.is_nan() || v.is_inf_pos() || v.is_inf_neg() {
        out.push_str("null");
    } else {
        number(out, v);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use chrono::{DateTime, NaiveDate, NaiveDateTime, NaiveTime, Utc};
    use uuid::Uuid;

    fn render(scalar: Scalar) -> String {
        let mut out = String::new();
        write_scalar(&mut out, &scalar);

        out
    }

    #[test]
    fn numbers_are_verbatim() {
        assert_eq!(render(Scalar::Bool(true)), "true");
        assert_eq!(render(Scalar::Bool(false)), "false");
        assert_eq!(render(Scalar::I8(-8)), "-8");
        assert_eq!(render(Scalar::I16(-16)), "-16");
        assert_eq!(render(Scalar::I32(-32)), "-32");
        assert_eq!(render(Scalar::I64(-64)), "-64");
        assert_eq!(render(Scalar::I128(i128::MIN)), i128::MIN.to_string());
        assert_eq!(render(Scalar::U8(7)), "7");
        assert_eq!(render(Scalar::U16(16)), "16");
        assert_eq!(render(Scalar::U32(32)), "32");
        assert_eq!(render(Scalar::U64(64)), "64");
        assert_eq!(render(Scalar::U128(u128::MAX)), u128::MAX.to_string());
        assert_eq!(render(Scalar::F32(1.5)), "1.5");
        assert_eq!(render(Scalar::F64(1.5)), "1.5");
    }

    #[cfg(feature = "bigfloat-as-scalar")]
    #[test]
    fn big_float_renders_as_a_literal_or_null() {
        use num_bigfloat::{BigFloat, INF_POS, NAN};

        // A finite value renders as its (fixed-precision) decimal literal, not `null`.
        let finite = render(Scalar::BigFloat(BigFloat::from_f64(1.5)));
        assert!(finite.starts_with("1.5"), "got {finite}");

        // The non-finite values share the float fallback to `null`.
        assert_eq!(render(Scalar::BigFloat(NAN)), "null");
        assert_eq!(render(Scalar::BigFloat(INF_POS)), "null");
    }

    #[test]
    fn non_finite_floats_are_null() {
        assert_eq!(render(Scalar::F64(f64::NAN)), "null");
        assert_eq!(render(Scalar::F64(f64::INFINITY)), "null");
        assert_eq!(render(Scalar::F32(f32::NEG_INFINITY)), "null");
    }

    #[test]
    fn strings_and_bytes_are_quoted() {
        assert_eq!(render(Scalar::Str("a\"b".into())), "\"a\\\"b\"");
        assert_eq!(render(Scalar::Bytes(vec![0, 1, 2, 255])), "\"AAEC/w==\"");
    }

    #[test]
    fn uuid_is_quoted_and_hyphenated() {
        assert_eq!(
            render(Scalar::Uuid(Uuid::from_u128(0))),
            "\"00000000-0000-0000-0000-000000000000\""
        );
    }

    #[test]
    fn temporal_scalars_are_iso_text() {
        let date = NaiveDate::from_ymd_opt(2026, 6, 28).unwrap();
        assert_eq!(render(Scalar::Date(date)), "\"2026-06-28\"");
        assert_eq!(
            render(Scalar::Time(NaiveTime::from_hms_opt(23, 59, 9).unwrap())),
            "\"23:59:09\""
        );

        let dt: DateTime<Utc> = NaiveDateTime::new(date, NaiveTime::from_hms_opt(12, 0, 0).unwrap()).and_utc();
        assert_eq!(render(Scalar::DateTime(dt)), "\"2026-06-28T12:00:00+00:00\"");
    }

    #[test]
    fn duration_is_decimal_seconds() {
        assert_eq!(render(Scalar::Duration(TimeDelta::seconds(90))), "90");
        assert_eq!(render(Scalar::Duration(TimeDelta::milliseconds(1500))), "1.5");
        assert_eq!(render(Scalar::Duration(TimeDelta::milliseconds(-500))), "-0.5");
        assert_eq!(
            render(Scalar::Duration(
                TimeDelta::seconds(-90) + TimeDelta::milliseconds(-500)
            )),
            "-90.5"
        );
    }
}
