//! A minimal standard-alphabet ([RFC 4648 §4]) Base64 encoder, used to render
//! the `Bytes` scalar as text. Encoding only — ArborDb never parses an export
//! back in.
//!
//! [RFC 4648 §4]: https://www.rfc-editor.org/rfc/rfc4648#section-4

const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

/// Encodes `input` as a padded Base64 string.
pub(super) fn encode(input: &[u8]) -> String {
    let mut out = String::with_capacity(input.len().div_ceil(3) * 4);

    for chunk in input.chunks(3) {
        let b1 = chunk.get(1).copied();
        let b2 = chunk.get(2).copied();

        let triple = (u32::from(chunk[0]) << 16) | (u32::from(b1.unwrap_or(0)) << 8) | u32::from(b2.unwrap_or(0));

        out.push(char::from(ALPHABET[((triple >> 18) & 0x3F) as usize]));
        out.push(char::from(ALPHABET[((triple >> 12) & 0x3F) as usize]));
        out.push(match b1 {
            Some(_) => char::from(ALPHABET[((triple >> 6) & 0x3F) as usize]),
            None => '=',
        });

        out.push(match b2 {
            Some(_) => char::from(ALPHABET[(triple & 0x3F) as usize]),
            None => '=',
        });
    }

    out
}

#[cfg(test)]
mod tests {
    use super::encode;

    #[test]
    fn rfc4648_test_vectors() {
        assert_eq!(encode(b""), "");
        assert_eq!(encode(b"f"), "Zg==");
        assert_eq!(encode(b"fo"), "Zm8=");
        assert_eq!(encode(b"foo"), "Zm9v");
        assert_eq!(encode(b"foob"), "Zm9vYg==");
        assert_eq!(encode(b"fooba"), "Zm9vYmE=");
        assert_eq!(encode(b"foobar"), "Zm9vYmFy");
    }

    #[test]
    fn encodes_high_bytes() {
        assert_eq!(encode(&[0, 1, 2, 255]), "AAEC/w==");
    }
}
