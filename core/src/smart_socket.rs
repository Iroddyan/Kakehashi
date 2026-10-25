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

/// The status bytes at the start of a smart-socket response.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SmartSocketStatus {
    Okay,
    Fail,
}

/// Errors raised while encoding or decoding smart-socket frames.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SmartSocketError {
    RequestTooLong { length: usize },
    TruncatedInput { expected: usize, actual: usize },
    InvalidHexLength,
    InvalidStatus,
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
        }
    }
}

impl Error for SmartSocketError {}

/// Encodes a request as a four-digit ASCII hexadecimal byte length followed by its payload.
///
/// AOSP documents this framing in `platform/packages/modules/adb/docs/dev/overview.md` at
/// commit `1cf2f017d312f73b3dc53bda85ef2610e35a80e9`.
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

/// Decodes the four-byte response status, leaving subsequent bytes for the caller to interpret.
///
/// AOSP documents status-only success, length-prefixed failure reasons, and connection state
/// changes after `OKAY` in `platform/packages/modules/adb/docs/dev/overview.md` at commit
/// `1cf2f017d312f73b3dc53bda85ef2610e35a80e9`. This function reads only those four status bytes.
pub fn decode_status(input: &[u8]) -> Result<SmartSocketStatus, SmartSocketError> {
    if input.len() < 4 {
        return Err(SmartSocketError::TruncatedInput {
            expected: 4,
            actual: input.len(),
        });
    }

    let status = &input[..4];
    match status {
        b"OKAY" => Ok(SmartSocketStatus::Okay),
        b"FAIL" => Ok(SmartSocketStatus::Fail),
        _ => Err(SmartSocketError::InvalidStatus),
    }
}

/// Decodes one four-digit hexadecimal length-prefixed payload from the start of `input`.
///
/// Returns the payload and the number of bytes consumed (the four-byte prefix plus payload).
/// Any later bytes belong to the caller's stream and are left unconsumed. Host service replies
/// that carry data after `OKAY` use this framing according to
/// `platform/packages/modules/adb/docs/dev/services.md` at commit
/// `1cf2f017d312f73b3dc53bda85ef2610e35a80e9`.
pub fn decode_length_prefixed_payload(input: &[u8]) -> Result<(Vec<u8>, usize), SmartSocketError> {
    if input.len() < 4 {
        return Err(SmartSocketError::TruncatedInput {
            expected: 4,
            actual: input.len(),
        });
    }
    let payload_len = parse_length(&input[..4])?;
    let expected = 4 + payload_len;
    if input.len() < expected {
        return Err(SmartSocketError::TruncatedInput {
            expected,
            actual: input.len(),
        });
    }
    Ok((input[4..expected].to_vec(), expected))
}

/// Decodes one response, returning the number of bytes consumed from the stream.
///
/// Pass `okay_has_length_prefixed_payload` according to the request being answered: an `OKAY`
/// can be status-only or followed by a length-prefixed payload. This avoids guessing when a TCP
/// read ends exactly after the status. A `FAIL` response always carries its framed reason.
pub fn decode_response(
    input: &[u8],
    okay_has_length_prefixed_payload: bool,
) -> Result<(SmartSocketResponse, usize), SmartSocketError> {
    let status = decode_status(input)?;
    if status == SmartSocketStatus::Okay && !okay_has_length_prefixed_payload {
        return Ok((
            SmartSocketResponse::Okay {
                payload: Vec::new(),
            },
            4,
        ));
    }

    let (payload, payload_consumed) = decode_length_prefixed_payload(&input[4..])?;
    let response = match status {
        SmartSocketStatus::Okay => SmartSocketResponse::Okay { payload },
        SmartSocketStatus::Fail => SmartSocketResponse::Fail { message: payload },
    };
    Ok((response, 4 + payload_consumed))
}

fn parse_length(input: &[u8]) -> Result<usize, SmartSocketError> {
    if input.len() != 4 {
        return Err(SmartSocketError::TruncatedInput {
            expected: 4,
            actual: input.len(),
        });
    }
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
    use super::{
        decode_length_prefixed_payload, decode_response, decode_status, encode_request,
        SmartSocketError, SmartSocketResponse, SmartSocketStatus,
    };

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
            decode_response(b"OKAY0005helloNEXT", true).unwrap(),
            (
                SmartSocketResponse::Okay {
                    payload: b"hello".to_vec()
                },
                13
            )
        );
    }

    #[test]
    fn decodes_status_only_okay() {
        assert_eq!(decode_status(b"OKAY"), Ok(SmartSocketStatus::Okay));
        assert_eq!(
            decode_response(b"OKAY", false).unwrap(),
            (
                SmartSocketResponse::Okay {
                    payload: Vec::new()
                },
                4
            )
        );
    }

    #[test]
    fn decodes_fail_response_message() {
        assert_eq!(
            decode_response(b"FAIL0009bad thingNEXT", false).unwrap(),
            (
                SmartSocketResponse::Fail {
                    message: b"bad thing".to_vec()
                },
                17
            )
        );
    }

    #[test]
    fn length_prefixed_decoder_reports_consumed_bytes() {
        assert_eq!(
            decode_length_prefixed_payload(b"0005helloNEXT").unwrap(),
            (b"hello".to_vec(), 9)
        );
    }

    #[test]
    fn rejects_truncated_status_and_length_prefix() {
        assert_eq!(
            decode_status(b"OK"),
            Err(SmartSocketError::TruncatedInput {
                expected: 4,
                actual: 2
            })
        );
        assert_eq!(
            decode_length_prefixed_payload(b"00"),
            Err(SmartSocketError::TruncatedInput {
                expected: 4,
                actual: 2
            })
        );
    }

    #[test]
    fn rejects_payload_shorter_than_declared_length() {
        assert_eq!(
            decode_length_prefixed_payload(b"0005hey"),
            Err(SmartSocketError::TruncatedInput {
                expected: 9,
                actual: 7
            })
        );
    }

    #[test]
    fn rejects_non_hex_length() {
        assert_eq!(
            decode_length_prefixed_payload(b"00x1a"),
            Err(SmartSocketError::InvalidHexLength)
        );
    }

    #[test]
    fn rejects_invalid_status() {
        assert_eq!(decode_status(b"NOPE"), Err(SmartSocketError::InvalidStatus));
    }

    #[test]
    fn decodes_captured_host_version_response_fixture() {
        let response = include_bytes!("../../tests/fixtures/host-version-response.txt");
        assert_eq!(
            decode_response(response, true).unwrap(),
            (
                SmartSocketResponse::Okay {
                    payload: b"0029".to_vec()
                },
                12
            )
        );
    }

    #[test]
    fn rejects_oversized_requests() {
        assert!(matches!(
            encode_request(&vec![0; 0x1_0000]),
            Err(SmartSocketError::RequestTooLong { length: 0x1_0000 })
        ));
    }
}
