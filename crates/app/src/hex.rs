//! Hexadecimal, as checksums are published and keys written in the source.

/// Lower-case hexadecimal, two digits a byte.
pub fn encode(bytes: &[u8]) -> String {
    use std::fmt::Write;
    bytes.iter().fold(String::with_capacity(bytes.len() * 2), |mut out, b| {
        let _ = write!(out, "{b:02x}");
        out
    })
}

/// `N` bytes from `2 × N` lower-case hexadecimal digits: for constants, where a malformed value
/// stops the build.
pub const fn decode<const N: usize>(s: &str) -> [u8; N] {
    const fn nibble(c: u8) -> u8 {
        match c {
            b'0'..=b'9' => c - b'0',
            b'a'..=b'f' => c - b'a' + 10,
            _ => panic!("bad hex"),
        }
    }
    let b = s.as_bytes();
    assert!(b.len() == 2 * N);
    let mut out = [0u8; N];
    let mut i = 0;
    while i < N {
        out[i] = nibble(b[2 * i]) << 4 | nibble(b[2 * i + 1]);
        i += 1;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encodes_lower_case_two_digits_a_byte() {
        assert_eq!(encode(&[0x00, 0x0f, 0xab, 0xff]), "000fabff");
        assert_eq!(encode(&[]), "");
    }

    #[test]
    fn decodes_what_it_encodes() {
        const KEY: [u8; 4] = decode("00a1ff7e");

        assert_eq!(KEY, [0x00, 0xa1, 0xff, 0x7e]);
        assert_eq!(encode(&KEY), "00a1ff7e");
    }
}
