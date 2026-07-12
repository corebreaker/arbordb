//! Order-preserving encoding of scalar values for index keys.
//!
//! A `Scalar` is encoded so that the bytewise (`memcmp`) order of two encodings
//! matches the values' semantic order — the property index keys rely on, since
//! the engine compares index keys bytewise. Each encoding is a 1-byte type tag
//! followed by an order-preserving, self-delimiting body, so per-column
//! encodings concatenate into a composite key and still compare correctly. A
//! descending column is the bitwise complement of its ascending encoding.
//!
//! This is **encode-only**: index queries build key prefixes to scan; they never
//! decode column values back (a lookup returns the stored entity key).

use crate::data::Scalar;
use chrono::{Datelike, Timelike};

#[cfg(any(feature = "bigint-as-scalar", feature = "rational-as-scalar"))]
use num_bigint::{BigInt, Sign};

#[cfg(feature = "bigfloat-as-scalar")]
use num_bigfloat::BigFloat;

#[cfg(feature = "rational-as-scalar")]
use num_rational::BigRational;

/// Type tags. Their relative order fixes the (arbitrary but deterministic)
/// ordering between distinct scalar types; within a type the body decides.
/// `Null` is the smallest so a missing/`Null` column sorts first (ascending).
mod tag {
    pub(super) const NULL: u8 = 0;
    pub(super) const BOOL: u8 = 1;
    pub(super) const I8: u8 = 2;
    pub(super) const I16: u8 = 3;
    pub(super) const I32: u8 = 4;
    pub(super) const I64: u8 = 5;
    pub(super) const I128: u8 = 6;
    pub(super) const U8: u8 = 7;
    pub(super) const U16: u8 = 8;
    pub(super) const U32: u8 = 9;
    pub(super) const U64: u8 = 10;
    pub(super) const U128: u8 = 11;
    pub(super) const F32: u8 = 12;
    pub(super) const F64: u8 = 13;
    pub(super) const STR: u8 = 14;
    pub(super) const BYTES: u8 = 15;
    pub(super) const UUID: u8 = 16;
    pub(super) const DATE: u8 = 17;
    pub(super) const TIME: u8 = 18;
    pub(super) const DATETIME: u8 = 19;
    pub(super) const DURATION: u8 = 20;
    #[cfg(feature = "bigint-as-scalar")]
    pub(super) const BIG_INT: u8 = 21;
    // The `BigFloat` classes each get their own tag, and unlike the exact storage tags these are laid out
    // in ascending value order so the leading byte alone sorts the classes:
    // −∞ < negatives < 0 < positives < +∞. NaN is unordered, so it is parked at the top of the block.
    #[cfg(feature = "bigfloat-as-scalar")]
    pub(super) const BIG_FLOAT_NEG_INF: u8 = 22;
    #[cfg(feature = "bigfloat-as-scalar")]
    pub(super) const BIG_FLOAT_NEG: u8 = 23;
    #[cfg(feature = "bigfloat-as-scalar")]
    pub(super) const BIG_FLOAT_ZERO: u8 = 24;
    #[cfg(feature = "bigfloat-as-scalar")]
    pub(super) const BIG_FLOAT_POS: u8 = 25;
    #[cfg(feature = "bigfloat-as-scalar")]
    pub(super) const BIG_FLOAT_POS_INF: u8 = 26;
    #[cfg(feature = "bigfloat-as-scalar")]
    pub(super) const BIG_FLOAT_NAN: u8 = 27;
    #[cfg(feature = "rational-as-scalar")]
    pub(super) const BIG_RATIONAL: u8 = 28;
}

/// Appends `scalar`'s order-preserving encoding to `out`. When `descending`, the
/// freshly written bytes are bit-inverted so the column sorts in reverse.
pub(crate) fn encode_scalar(out: &mut Vec<u8>, scalar: &Scalar, descending: bool) {
    let start = out.len();
    encode_ascending(out, scalar);

    if descending {
        for byte in &mut out[start..] {
            *byte = !*byte;
        }
    }
}

fn encode_ascending(out: &mut Vec<u8>, scalar: &Scalar) {
    match scalar {
        Scalar::Null => out.push(tag::NULL),
        Scalar::Bool(v) => {
            out.push(tag::BOOL);
            out.push(u8::from(*v));
        }
        Scalar::I8(v) => signed(out, tag::I8, &v.to_be_bytes()),
        Scalar::I16(v) => signed(out, tag::I16, &v.to_be_bytes()),
        Scalar::I32(v) => signed(out, tag::I32, &v.to_be_bytes()),
        Scalar::I64(v) => signed(out, tag::I64, &v.to_be_bytes()),
        Scalar::I128(v) => signed(out, tag::I128, &v.to_be_bytes()),
        Scalar::U8(v) => {
            out.push(tag::U8);
            out.push(*v);
        }
        Scalar::U16(v) => unsigned(out, tag::U16, &v.to_be_bytes()),
        Scalar::U32(v) => unsigned(out, tag::U32, &v.to_be_bytes()),
        Scalar::U64(v) => unsigned(out, tag::U64, &v.to_be_bytes()),
        Scalar::U128(v) => unsigned(out, tag::U128, &v.to_be_bytes()),
        Scalar::F32(v) => unsigned(out, tag::F32, &float32(*v).to_be_bytes()),
        Scalar::F64(v) => unsigned(out, tag::F64, &float64(*v).to_be_bytes()),
        Scalar::Str(v) => {
            out.push(tag::STR);
            ordered_bytes(out, v.as_bytes());
        }
        Scalar::Bytes(v) => {
            out.push(tag::BYTES);
            ordered_bytes(out, v);
        }
        Scalar::Uuid(v) => {
            out.push(tag::UUID);
            out.extend_from_slice(v.as_bytes());
        }
        Scalar::Date(v) => signed(out, tag::DATE, &v.num_days_from_ce().to_be_bytes()),
        Scalar::Time(v) => {
            out.push(tag::TIME);
            out.extend_from_slice(&v.num_seconds_from_midnight().to_be_bytes());
            out.extend_from_slice(&v.nanosecond().to_be_bytes());
        }
        Scalar::DateTime(v) => {
            out.push(tag::DATETIME);
            let naive = v.naive_utc();
            push_signed(out, &naive.num_days_from_ce().to_be_bytes());
            out.extend_from_slice(&naive.num_seconds_from_midnight().to_be_bytes());
            out.extend_from_slice(&naive.nanosecond().to_be_bytes());
        }
        Scalar::Duration(v) => {
            // Total nanoseconds as a signed 128-bit value captures the full order
            // (whole TimeDelta range × 1e9 fits in i128).
            let nanos = i128::from(v.num_seconds()) * 1_000_000_000 + i128::from(v.subsec_nanos());
            signed(out, tag::DURATION, &nanos.to_be_bytes());
        }
        #[cfg(feature = "bigint-as-scalar")]
        Scalar::BigInt(v) => {
            out.push(tag::BIG_INT);
            encode_signed_int(out, v);
        }
        #[cfg(feature = "bigfloat-as-scalar")]
        Scalar::BigFloat(v) => encode_bigfloat(out, v),
        #[cfg(feature = "rational-as-scalar")]
        Scalar::Rational(v) => encode_rational(out, v),
    }
}

/// Writes `tag` then a sign-flipped big-endian integer (two's-complement order →
/// unsigned byte order: flipping the sign bit maps the minimum to all-zero).
fn signed(out: &mut Vec<u8>, tag: u8, be: &[u8]) {
    out.push(tag);
    push_signed(out, be);
}

fn push_signed(out: &mut Vec<u8>, be: &[u8]) {
    out.push(be[0] ^ 0x80);
    out.extend_from_slice(&be[1..]);
}

/// Writes `tag` then big-endian bytes verbatim (already order-preserving for
/// unsigned integers and the float total-order transform).
fn unsigned(out: &mut Vec<u8>, tag: u8, be: &[u8]) {
    out.push(tag);
    out.extend_from_slice(be);
}

/// IEEE-754 total order: negatives (sign bit set) flip every bit; non-negatives
/// flip only the sign bit. The result compares as unsigned in value order.
fn float64(v: f64) -> u64 {
    let bits = v.to_bits();
    if bits & (1 << 63) != 0 { !bits } else { bits | (1 << 63) }
}

fn float32(v: f32) -> u32 {
    let bits = v.to_bits();
    if bits & (1 << 31) != 0 { !bits } else { bits | (1 << 31) }
}

/// Order-preserving, self-delimiting byte string: each `0x00` is escaped as
/// `0x00 0xFF`, and the whole is terminated by `0x00 0x01`.
///
/// The two-byte terminator (rather than a bare `0x00`) is what makes encodings
/// **prefix-free**: after any `0x00` the next byte is `0xFF` (escaped content) or
/// `0x01` (terminator), and `0x01 < 0xFF`, so no encoding is a prefix of
/// another. That matters for descending columns — `encode_scalar` reverses a
/// column by bit-complementing it, and bit-complement only reverses order when
/// neither operand is a prefix of the other (a bare `0x00` terminator would make
/// the empty string a prefix of `"\0"`, breaking descending order).
fn ordered_bytes(out: &mut Vec<u8>, bytes: &[u8]) {
    for &byte in bytes {
        out.push(byte);
        if byte == 0x00 {
            out.push(0xFF);
        }
    }

    out.push(0x00);
    out.push(0x01);
}

/// Appends an order-preserving, self-delimiting encoding of a non-negative
/// integer given as its minimal big-endian magnitude `mag` (no leading zero
/// bytes). The byte length is written first — itself length-prefixed, so the
/// whole encoding stays self-delimiting — meaning a longer magnitude (a larger
/// value) always sorts above a shorter one, and equal-length magnitudes then
/// compare bytewise in value order. A bare big-endian concatenation would not:
/// it sorts `[0x02]` ("2") above `[0x01, 0x00]` ("256").
#[cfg(any(feature = "bigint-as-scalar", feature = "rational-as-scalar"))]
fn put_uint_be(out: &mut Vec<u8>, mag: &[u8]) {
    let len_be = (mag.len() as u64).to_be_bytes();
    let start = len_be.iter().position(|&b| b != 0).unwrap_or(len_be.len());

    out.push((len_be.len() - start) as u8);
    out.extend_from_slice(&len_be[start..]);
    out.extend_from_slice(mag);
}

/// Appends an order-preserving encoding of a signed big integer: a sign-class
/// byte (negatives < zero < positives) then the magnitude via [`put_uint_be`].
/// For negatives the magnitude bytes are bit-inverted, so a larger magnitude (a
/// smaller value) sorts lower.
#[cfg(any(feature = "bigint-as-scalar", feature = "rational-as-scalar"))]
fn encode_signed_int(out: &mut Vec<u8>, v: &BigInt) {
    const NEG: u8 = 0;
    const ZERO: u8 = 1;
    const POS: u8 = 2;

    match v.sign() {
        Sign::NoSign => out.push(ZERO),
        Sign::Plus => {
            out.push(POS);
            put_uint_be(out, &v.magnitude().to_bytes_be());
        }
        Sign::Minus => {
            out.push(NEG);

            let start = out.len();
            put_uint_be(out, &v.magnitude().to_bytes_be());
            for b in &mut out[start..] {
                *b = !*b;
            }
        }
    }
}

/// Appends the value-ordered encoding of a `BigFloat`. The class tag orders the
/// special values and the sign; a finite, non-zero number is keyed first by `d`,
/// the base-10 exponent of its leading digit (so magnitude dominates), then by
/// its significant digits. Negatives invert the body so a larger magnitude sorts
/// lower.
#[cfg(feature = "bigfloat-as-scalar")]
fn encode_bigfloat(out: &mut Vec<u8>, v: &BigFloat) {
    if v.is_nan() {
        return out.push(tag::BIG_FLOAT_NAN);
    }
    if v.is_inf_pos() {
        return out.push(tag::BIG_FLOAT_POS_INF);
    }
    if v.is_inf_neg() {
        return out.push(tag::BIG_FLOAT_NEG_INF);
    }
    if v.is_zero() {
        return out.push(tag::BIG_FLOAT_ZERO);
    }

    let n = v.get_mantissa_len();
    let mut digits = vec![0u8; n];
    v.get_mantissa_bytes(&mut digits);

    // Trailing zeros do not change the value; dropping them makes the digit
    // string canonical, so values that are equal encode identically.
    while digits.len() > 1 && *digits.last().unwrap() == 0 {
        digits.pop();
    }

    // value = (n significant digits) × 10^e with the point after the last digit,
    // so the base-10 exponent of the leading digit is (n - 1) + e. It fits i16.
    let d = (n as i32 - 1 + i32::from(v.get_exponent())) as i16;

    let mut body = Vec::new();
    push_signed(&mut body, &d.to_be_bytes());
    ordered_bytes(&mut body, &digits);

    if v.is_negative() {
        out.push(tag::BIG_FLOAT_NEG);
        for b in &body {
            out.push(!b);
        }
    } else {
        out.push(tag::BIG_FLOAT_POS);
        out.extend_from_slice(&body);
    }
}

/// Computes the canonical continued-fraction terms `[a0, a1, …]` of `numer/denom`
/// (with `denom > 0`, as a `BigRational` guarantees). `a0` is the floor of the
/// value; every later term is ≥ 1 and the last is ≥ 2, so the expansion — and the
/// encoding built from it — is unique per value.
#[cfg(feature = "rational-as-scalar")]
fn continued_fraction(numer: &BigInt, denom: &BigInt) -> Vec<BigInt> {
    let mut terms = Vec::new();
    let mut p = numer.clone();
    let mut q = denom.clone();

    loop {
        // Floor division: `BigInt`'s `/` truncates toward zero, and `q` is always
        // positive, so only a negative remainder needs the downward correction.
        let trunc = &p / &q;
        let rem = &p - &(&trunc * &q);
        let (a, r) = if rem.sign() == Sign::Minus {
            (trunc - BigInt::from(1), rem + &q)
        } else {
            (trunc, rem)
        };

        terms.push(a);
        if r.sign() == Sign::NoSign {
            break;
        }

        p = q;
        q = r;
    }

    terms
}

/// Appends the value-ordered encoding of a rational via its continued fraction.
/// A continued fraction increases in its even-indexed terms and decreases in its
/// odd-indexed ones, so odd-index terms are bit-inverted. A per-position marker
/// separates "another term follows" from "the expansion stops here"; the stop
/// marker's value is chosen by parity so termination sorts on the correct side of
/// a continuation at that position.
#[cfg(feature = "rational-as-scalar")]
fn encode_rational(out: &mut Vec<u8>, v: &BigRational) {
    const CONTINUE: u8 = 1;
    const STOP_ODD: u8 = 0;
    const STOP_EVEN: u8 = 2;

    out.push(tag::BIG_RATIONAL);

    let terms = continued_fraction(v.numer(), v.denom());
    encode_signed_int(out, &terms[0]);

    for (i, term) in terms.iter().enumerate().skip(1) {
        out.push(CONTINUE);

        let start = out.len();
        put_uint_be(out, &term.magnitude().to_bytes_be());
        if i % 2 == 1 {
            for b in &mut out[start..] {
                *b = !*b;
            }
        }
    }

    // The stop sits at the position just past the last term; its parity decides
    // whether "stop" must sort below (odd) or above (even) a continuation.
    let stop = if terms.len().is_multiple_of(2) {
        STOP_EVEN
    } else {
        STOP_ODD
    };

    out.push(stop);
}

#[cfg(test)]
mod tests {
    use super::*;

    use chrono::{DateTime, NaiveDate, NaiveTime, TimeDelta, Utc};
    use uuid::Uuid;

    fn asc(scalar: &Scalar) -> Vec<u8> {
        let mut out = Vec::new();
        encode_scalar(&mut out, scalar, false);
        out
    }

    fn desc(scalar: &Scalar) -> Vec<u8> {
        let mut out = Vec::new();
        encode_scalar(&mut out, scalar, true);
        out
    }

    /// Asserts the scalars are given in strictly ascending value order, and that
    /// the encoding preserves it (ascending) and reverses it (descending).
    fn assert_ordered(sorted: &[Scalar]) {
        for pair in sorted.windows(2) {
            let (lo, hi) = (&pair[0], &pair[1]);
            assert!(asc(lo) < asc(hi), "ascending: {lo:?} should encode below {hi:?}");
            assert!(desc(lo) > desc(hi), "descending: {lo:?} should encode above {hi:?}");
        }
    }

    fn date(y: i32, m: u32, d: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(y, m, d).unwrap()
    }

    fn datetime(y: i32, m: u32, d: u32, h: u32, min: u32, s: u32) -> DateTime<Utc> {
        date(y, m, d).and_hms_opt(h, min, s).unwrap().and_utc()
    }

    #[test]
    fn signed_integers_order() {
        assert_ordered(&[
            Scalar::I32(i32::MIN),
            Scalar::I32(-2),
            Scalar::I32(-1),
            Scalar::I32(0),
            Scalar::I32(1),
            Scalar::I32(i32::MAX),
        ]);
        assert_ordered(&[Scalar::I128(i128::MIN), Scalar::I128(0), Scalar::I128(i128::MAX)]);
        assert_ordered(&[Scalar::I8(i8::MIN), Scalar::I8(0), Scalar::I8(i8::MAX)]);
        assert_ordered(&[Scalar::I16(i16::MIN), Scalar::I16(0), Scalar::I16(i16::MAX)]);
    }

    #[test]
    fn unsigned_integers_order() {
        assert_ordered(&[Scalar::U64(0), Scalar::U64(1), Scalar::U64(u64::MAX)]);
        assert_ordered(&[Scalar::U8(0), Scalar::U8(128), Scalar::U8(u8::MAX)]);
        assert_ordered(&[Scalar::U16(0), Scalar::U16(1), Scalar::U16(u16::MAX)]);
        assert_ordered(&[Scalar::U32(0), Scalar::U32(1), Scalar::U32(u32::MAX)]);
        assert_ordered(&[Scalar::U128(0), Scalar::U128(1), Scalar::U128(u128::MAX)]);
    }

    #[test]
    fn booleans_order() {
        assert_ordered(&[Scalar::Bool(false), Scalar::Bool(true)]);
    }

    #[test]
    fn floats_order() {
        assert_ordered(&[
            Scalar::F64(f64::NEG_INFINITY),
            Scalar::F64(-100.5),
            Scalar::F64(-1.0),
            Scalar::F64(0.0),
            Scalar::F64(1.0),
            Scalar::F64(100.5),
            Scalar::F64(f64::INFINITY),
        ]);
        assert_ordered(&[Scalar::F32(-1.0), Scalar::F32(0.0), Scalar::F32(1.0)]);
    }

    #[test]
    fn strings_order() {
        assert_ordered(&[
            Scalar::Str(String::new()),
            Scalar::Str("a".into()),
            Scalar::Str("ab".into()),
            Scalar::Str("b".into()),
        ]);
    }

    #[test]
    fn strings_with_interior_null_byte_stay_ordered() {
        // The escape (0x00 -> 0x00 0xFF) + 0x00 terminator must keep prefixes
        // below extensions and "\0" below ordinary bytes.
        assert_ordered(&[
            Scalar::Str("a".into()),
            Scalar::Str("a\u{0}".into()),
            Scalar::Str("a\u{0}b".into()),
            Scalar::Str("ab".into()),
        ]);
    }

    #[test]
    fn bytes_order() {
        assert_ordered(&[
            Scalar::Bytes(vec![]),
            Scalar::Bytes(vec![0]),
            Scalar::Bytes(vec![0, 0]),
            Scalar::Bytes(vec![1]),
            Scalar::Bytes(vec![255]),
        ]);
    }

    #[test]
    fn uuids_order() {
        assert_ordered(&[
            Scalar::Uuid(Uuid::from_u128(0)),
            Scalar::Uuid(Uuid::from_u128(1)),
            Scalar::Uuid(Uuid::from_u128(u128::MAX)),
        ]);
    }

    #[test]
    fn temporal_order() {
        assert_ordered(&[
            Scalar::Date(date(1, 1, 1)),
            Scalar::Date(date(1969, 12, 31)),
            Scalar::Date(date(2026, 6, 22)),
        ]);
        assert_ordered(&[
            Scalar::Time(NaiveTime::from_hms_opt(0, 0, 0).unwrap()),
            Scalar::Time(NaiveTime::from_hms_milli_opt(12, 0, 0, 1).unwrap()),
            Scalar::Time(NaiveTime::from_hms_opt(23, 59, 59).unwrap()),
        ]);
        assert_ordered(&[
            Scalar::DateTime(datetime(1970, 1, 1, 0, 0, 0)),
            Scalar::DateTime(datetime(1970, 1, 1, 0, 0, 1)),
            Scalar::DateTime(datetime(2026, 6, 22, 12, 0, 0)),
        ]);
        assert_ordered(&[
            Scalar::Duration(TimeDelta::seconds(-10)),
            Scalar::Duration(TimeDelta::seconds(-1) + TimeDelta::nanoseconds(-1)),
            Scalar::Duration(TimeDelta::zero()),
            Scalar::Duration(TimeDelta::nanoseconds(1)),
            Scalar::Duration(TimeDelta::seconds(10)),
        ]);
    }

    #[test]
    fn null_sorts_before_every_other_type() {
        let null = asc(&Scalar::Null);
        for other in [
            Scalar::Bool(false),
            Scalar::I32(i32::MIN),
            Scalar::U8(0),
            Scalar::F64(f64::NEG_INFINITY),
            Scalar::Str(String::new()),
            Scalar::Bytes(vec![]),
        ] {
            assert!(null < asc(&other), "Null should sort before {other:?}");
        }
    }

    #[test]
    fn composite_keys_compare_column_by_column() {
        // Build a 2-column key (string, i32) by concatenating per-column encodings.
        fn composite(name: &str, n: i32) -> Vec<u8> {
            let mut out = Vec::new();
            encode_scalar(&mut out, &Scalar::Str(name.into()), false);
            encode_scalar(&mut out, &Scalar::I32(n), false);
            out
        }

        // The first column dominates; the second breaks ties — even when the
        // second would order the other way on its own.
        assert!(composite("a", 100) < composite("ab", 1));
        assert!(composite("a", 1) < composite("a", 2));
        assert!(composite("a", 2) < composite("b", 1));
    }

    #[cfg(feature = "bigint-as-scalar")]
    #[test]
    fn bigints_order() {
        // The 127/128 and 255/256 pairs cross a byte-length boundary — exactly
        // where a fixed-width sign flip (the previous encoding) inverted the order.
        let huge = BigInt::parse_bytes(b"123456789012345678901234567890", 10).unwrap();

        assert_ordered(&[
            Scalar::BigInt(-huge.clone()),
            Scalar::BigInt(BigInt::from(-257)),
            Scalar::BigInt(BigInt::from(-256)),
            Scalar::BigInt(BigInt::from(-129)),
            Scalar::BigInt(BigInt::from(-128)),
            Scalar::BigInt(BigInt::from(-1)),
            Scalar::BigInt(BigInt::from(0)),
            Scalar::BigInt(BigInt::from(1)),
            Scalar::BigInt(BigInt::from(127)),
            Scalar::BigInt(BigInt::from(128)),
            Scalar::BigInt(BigInt::from(255)),
            Scalar::BigInt(BigInt::from(256)),
            Scalar::BigInt(huge),
        ]);
    }

    #[cfg(feature = "bigfloat-as-scalar")]
    #[test]
    fn bigfloats_order() {
        use num_bigfloat::{INF_NEG, INF_POS};

        let bf = |s: &str| BigFloat::parse(s).unwrap();

        assert_ordered(&[
            Scalar::BigFloat(INF_NEG),
            Scalar::BigFloat(bf("-1000")),
            Scalar::BigFloat(bf("-100.5")),
            Scalar::BigFloat(bf("-1")),
            Scalar::BigFloat(bf("-0.01")),
            Scalar::BigFloat(bf("-0.001")),
            Scalar::BigFloat(bf("0")),
            Scalar::BigFloat(bf("0.001")),
            Scalar::BigFloat(bf("0.01")),
            Scalar::BigFloat(bf("0.1")),
            Scalar::BigFloat(bf("1")),
            Scalar::BigFloat(bf("1.5")),
            Scalar::BigFloat(bf("2")),
            Scalar::BigFloat(bf("10")),
            Scalar::BigFloat(bf("100.5")),
            Scalar::BigFloat(bf("1000")),
            Scalar::BigFloat(INF_POS),
        ]);

        // NaN is parked out of the value order under its own class tag.
        assert_eq!(asc(&Scalar::BigFloat(num_bigfloat::NAN)), vec![tag::BIG_FLOAT_NAN]);
    }

    #[cfg(feature = "rational-as-scalar")]
    #[test]
    fn rationals_order() {
        let r = |n: i64, d: i64| Scalar::Rational(BigRational::new(BigInt::from(n), BigInt::from(d)));

        // Spans negatives, the dense (0, 1) interval where continued fractions
        // earn their keep (1/100 < 1/3 < 1/2 < 2/3), and integers.
        assert_ordered(&[
            r(-2, 1),
            r(-3, 2),
            r(-1, 1),
            r(-1, 2),
            r(-1, 3),
            r(0, 1),
            r(1, 100),
            r(1, 3),
            r(1, 2),
            r(2, 3),
            r(1, 1),
            r(3, 2),
            r(2, 1),
            r(100, 1),
        ]);
    }
}
