//! ADB smart-socket request and response framing.

use std::error::Error;
use std::fmt;

/// The maximum payload size representable by the four-digit hexadecimal prefix.
pub const MAX_PAYLOAD_LEN: usize = 0xffff;

/// A decoded smart-socket response with its status and raw payload bytes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SmartSocketResponse {
    Okay { payload: Vec<u8> },
    Fail { message: Vec<u8> },
}

/// Errors raised while encoding or decoding smart-socket frames.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SmartSocketError {
    RequestTooLong { length: usize },
    TruncatedInput { expected: usize, actual: usize },
    InvalidHexLength,
    InvalidStatus,
    TrailingData { expected: usize, actual: usize },
}

impl fmt::Display for SmartSocketError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::RequestTooLong { length } => write!(
                f,
                "smart-socket payload length {length} exceeds {MAX_PAYLOAD_LEN} bytes"
            ),
            Self::TruncatedInput { expected, actual } => write!(
                f,
                "truncated smart-socket frame: expected {expected} bytes, received {actual}"
            ),
            Self::InvalidHexLength => f.write_str("smart-socket length is not four hex digits"),
            Self::InvalidStatus => f.write_str("smart-socket response status is not OKAY or FAIL"),
            Self::TrailingData { expected, actual } => write!(
                f,
                "smart-socket frame has trailing data: expected {expected} bytes, received {actual}"
            ),
        }
    }
}

impl Error for SmartSocketError {}

/// Encodes a request as a four-digit ASCII hexadecimal byte length followed by its payload.
///
/// AOSP documents this framing in `platform/system/core/adb/OVERVIEW.TXT` at
/// commit `65e461c478b000246ef81c3ca5fdd2dbb819d7ad` and in
/// `platform/packages/modules/adb/protocol.txt` at commit
/// `f35fe9f743e862149942c8713e9e5086ecbf903f`.
pub fn encode_request(payload: &[u8]) -> Result<Vec<u8>, SmartSocketError> {
    if payload.len() > MAX_PAYLOAD_LEN {
        return Err(SmartSocketError::RequestTooLong {
            length: payload.len(),
        });
    }

    let prefix = format!("{:04x}", payload.len());
    let mut frame = Vec::with_capacity(4 + payload.len());
    frame.extend_from_slice(prefix.as_bytes());
    frame.extend_from_slice(payload);
    Ok(frame)
}

/// Decodes an `OKAY` or `FAIL` response followed by a four-digit hexadecimal payload length.
///
/// AOSP describes `OKAY`/`FAIL`, the prefixed payload, and the failure message in
/// `platform/system/core/adb/OVERVIEW.TXT` at commit
/// `65e461c478b000246ef81c3ca5fdd2dbb819d7ad`; host service replies use the
/// same length-prefixed payload form in `platform/packages/modules/adb/docs/dev/services.md`
/// at `HEAD` (verified 2026-10-25). The special `host:version` raw version response
/// described by OVERVIEW.TXT is intentionally outside this framed response helper.
pub fn decode_response(input: &[u8]) -> Result<SmartSocketResponse, SmartSocketError> {
    if input.len() < 4 {
        return Err(SmartSocketError::TruncatedInput {
            expected: 4,
            actual: input.len(),
        });
    }

    let status = &input[..4];
    let is_okay = status == b"OKAY";
    let is_fail = status == b"FAIL";
    if !is_okay && !is_fail {
        return Err(SmartSocketError::InvalidStatus);
    }

    if input.len() < 8 {
        return Err(SmartSocketError::TruncatedInput {
            expected: 8,
            actual: input.len(),
        });
    }
    let payload_len = parse_length(&input[4..8])?;
    let expected = 8 + payload_len;
    if input.len() < expected {
        return Err(SmartSocketError::TruncatedInput {
            expected,
            actual: input.len(),
        });
    }
    if input.len() > expected {
        return Err(SmartSocketError::TrailingData {
            expected,
            actual: input.len(),
        });
    }

    let payload = input[8..].to_vec();
    if is_okay {
        Ok(SmartSocketResponse::Okay { payload })
    } else {
        Ok(SmartSocketResponse::Fail { message: payload })
    }
}

fn parse_length(input: &[u8]) -> Result<usize, SmartSocketError> {
    debug_assert_eq!(input.len(), 4);
    input.iter().try_fold(0usize, |value, digit| {
        let nibble = match digit {
            b'0'..=b'9' => usize::from(digit - b'0'),
            b'a'..=b'f' => usize::from(digit - b'a' + 10),
            b'A'..=b'F' => usize::from(digit - b'A' + 10),
            _ => return Err(SmartSocketError::InvalidHexLength),
        };
        Ok((value << 4) | nibble)
    })
}

#[cfg(test)]
mod tests {
    use super::{decode_response, encode_request, SmartSocketError, SmartSocketResponse};

    #[test]
    fn encodes_request_length_and_payload() {
        assert_eq!(
            encode_request(b"host:version").unwrap(),
            b"000chost:version"
        );
    }

    #[test]
    fn decodes_okay_response_payload() {
        assert_eq!(
            decode_response(b"OKAY0005hello").unwrap(),
            SmartSocketResponse::Okay {
                payload: b"hello".to_vec()
            }
        );
    }

    #[test]
    fn decodes_fail_response_message() {
        assert_eq!(
            decode_response(b"FAIL0009bad thing").unwrap(),
            SmartSocketResponse::Fail {
                message: b"bad thing".to_vec()
            }
        );
    }

    #[test]
    fn rejects_truncated_status_and_length_prefix() {
        assert_eq!(
            decode_response(b"OK"),
            Err(SmartSocketError::TruncatedInput {
                expected: 4,
                actual: 2
            })
        );
        assert_eq!(
            decode_response(b"OKAY00"),
            Err(SmartSocketError::TruncatedInput {
                expected: 8,
                actual: 6
            })
        );
    }

    #[test]
    fn rejects_payload_shorter_than_declared_length() {
        assert_eq!(
            decode_response(b"OKAY0005hey"),
            Err(SmartSocketError::TruncatedInput {
                expected: 13,
                actual: 11
            })
        );
    }

    #[test]
    fn rejects_non_hex_length() {
        assert_eq!(
            decode_response(b"OKAY00x1a"),
            Err(SmartSocketError::InvalidHexLength)
        );
    }

    #[test]
    fn rejects_invalid_status() {
        assert_eq!(
            decode_response(b"NOPE0000"),
            Err(SmartSocketError::InvalidStatus)
        );
    }

    #[test]
    fn rejects_extra_bytes_and_oversized_requests() {
        assert_eq!(
            decode_response(b"OKAY0000x"),
            Err(SmartSocketError::TrailingData {
                expected: 8,
                actual: 9
            })
        );
        assert!(matches!(
            encode_request(&vec![0; 0x1_0000]),
            Err(SmartSocketError::RequestTooLong { length: 0x1_0000 })
        ));
    }
}
