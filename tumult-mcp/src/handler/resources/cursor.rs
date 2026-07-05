//! Opaque base64 pagination cursor for `resources/list`.

use rust_mcp_sdk::schema::RpcError;

const BASE64_ALPHABET: &[u8; 64] =
    b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

/// Encode a `resources/list` offset as an opaque base64 cursor.
pub(super) fn encode_cursor(offset: usize) -> String {
    let digits = offset.to_string().into_bytes();
    let mut out = String::with_capacity(digits.len().div_ceil(3) * 4);
    for chunk in digits.chunks(3) {
        let b1 = chunk.get(1).copied().map(usize::from);
        let b2 = chunk.get(2).copied().map(usize::from);
        let group = (usize::from(chunk[0]) << 16) | (b1.unwrap_or(0) << 8) | b2.unwrap_or(0);
        out.push(char::from(BASE64_ALPHABET[(group >> 18) & 63]));
        out.push(char::from(BASE64_ALPHABET[(group >> 12) & 63]));
        out.push(if b1.is_some() {
            char::from(BASE64_ALPHABET[(group >> 6) & 63])
        } else {
            '='
        });
        out.push(if b2.is_some() {
            char::from(BASE64_ALPHABET[group & 63])
        } else {
            '='
        });
    }
    out
}

/// Decode an opaque cursor back to an offset.
///
/// # Errors
///
/// Returns an invalid-params [`RpcError`] for anything that is not the
/// base64 encoding of a decimal offset (per the MCP spec, invalid cursors
/// are protocol errors).
pub(super) fn decode_cursor(cursor: &str) -> Result<usize, RpcError> {
    let invalid =
        || RpcError::invalid_params().with_message(format!("invalid pagination cursor: {cursor}"));
    if cursor.is_empty() || !cursor.len().is_multiple_of(4) {
        return Err(invalid());
    }
    let trimmed = cursor.trim_end_matches('=');
    if cursor.len() - trimmed.len() > 2 {
        return Err(invalid());
    }
    let mut bits: usize = 0;
    let mut bit_count: u32 = 0;
    let mut digits: Vec<u8> = Vec::with_capacity(cursor.len() / 4 * 3);
    for byte in trimmed.bytes() {
        let value = BASE64_ALPHABET
            .iter()
            .position(|&b| b == byte)
            .ok_or_else(invalid)?;
        bits = (bits << 6) | value;
        bit_count += 6;
        if bit_count >= 8 {
            bit_count -= 8;
            let out = u8::try_from((bits >> bit_count) & 0xFF).map_err(|_| invalid())?;
            digits.push(out);
            bits &= (1 << bit_count) - 1;
        }
    }
    if bits != 0 {
        // Non-canonical padding bits.
        return Err(invalid());
    }
    let text = String::from_utf8(digits).map_err(|_| invalid())?;
    text.parse::<usize>().map_err(|_| invalid())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cursor_round_trips_offsets() {
        for offset in [0usize, 1, 99, 100, 12345, usize::MAX / 2] {
            let cursor = encode_cursor(offset);
            assert_eq!(decode_cursor(&cursor).unwrap(), offset, "cursor {cursor}");
        }
    }

    #[test]
    fn cursor_rejects_invalid_input() {
        for bad in ["", "!!!!", "AAAA", "abc", "MTAw=", "====", "not base64"] {
            assert!(
                decode_cursor(bad).is_err(),
                "cursor {bad:?} must be rejected"
            );
        }
    }
}
