use serde::Serialize;
use serde::de::DeserializeOwned;

use crate::abi::{RcCallResult, StatusCode};
use crate::protocol::{PROTOCOL_VERSION, ProtocolEnvelope};

#[derive(Debug)]
pub enum CodecError {
    Encode(bincode::error::EncodeError),
    Decode(bincode::error::DecodeError),
    ProtocolVersionMismatch { expected: u16, got: u16 },
}

impl core::fmt::Display for CodecError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            CodecError::Encode(err) => write!(f, "failed to encode plugin message: {err}"),
            CodecError::Decode(err) => write!(f, "failed to decode plugin message: {err}"),
            CodecError::ProtocolVersionMismatch { expected, got } => write!(
                f,
                "plugin protocol version mismatch: expected {expected}, got {got}"
            ),
        }
    }
}

impl std::error::Error for CodecError {}

impl CodecError {
    /// The status a plugin reports for this error.
    #[must_use]
    pub fn status(&self) -> StatusCode {
        match self {
            CodecError::Encode(_) => StatusCode::EncodeError,
            CodecError::Decode(_) => StatusCode::DecodeError,
            CodecError::ProtocolVersionMismatch { .. } => StatusCode::Unsupported,
        }
    }

    /// The call result a plugin returns for this error, carrying the
    /// readable text -- a protocol skew then reaches the user as "plugin
    /// protocol version mismatch: expected 10, got 11", not a bare code.
    #[must_use]
    pub fn into_call_result(&self) -> RcCallResult {
        RcCallResult::error_with_message(self.status(), &self.to_string())
    }
}

/// Upper bound on how many bytes decoding one message may consume or
/// allocate. bincode trusts length prefixes, so without a limit a message
/// decoded against the wrong schema (a skewed or corrupt plugin) can ask for
/// an absurd allocation and abort the process; real messages (frames, disk
/// images, save states) are a few megabytes.
// The pre-payload version check reads the envelope's first byte(s) as a
// varint-encoded `u16`; versions below 251 are exactly one byte.
const _: () = assert!(
    PROTOCOL_VERSION < 251,
    "a larger version changes the prefix"
);

pub const MAX_MESSAGE_BYTES: usize = 256 * 1024 * 1024;

fn config() -> impl bincode::config::Config {
    bincode::config::standard().with_limit::<MAX_MESSAGE_BYTES>()
}

pub fn encode<T: Serialize>(value: &T) -> Result<Vec<u8>, CodecError> {
    bincode::serde::encode_to_vec(value, config()).map_err(CodecError::Encode)
}

pub fn decode<T: DeserializeOwned>(bytes: &[u8]) -> Result<T, CodecError> {
    let (value, _): (T, usize) =
        bincode::serde::decode_from_slice(bytes, config()).map_err(CodecError::Decode)?;
    Ok(value)
}

pub fn encode_message<T: Serialize>(value: &T) -> Result<Vec<u8>, CodecError> {
    let wrapped = ProtocolEnvelope {
        protocol_version: PROTOCOL_VERSION,
        payload: value,
    };
    encode(&wrapped)
}

/// Decodes a message, checking the protocol version **before** touching the
/// payload: the envelope starts with the version as a variable-length `u16`
/// (frozen for the life of the project), so a plugin built against another
/// protocol is reported as such instead of its payload being decoded
/// against the wrong schema.
pub fn decode_message<T: DeserializeOwned>(bytes: &[u8]) -> Result<T, CodecError> {
    let (version, _): (u16, usize) =
        bincode::serde::decode_from_slice(bytes, config()).map_err(CodecError::Decode)?;
    if version != PROTOCOL_VERSION {
        return Err(CodecError::ProtocolVersionMismatch {
            expected: PROTOCOL_VERSION,
            got: version,
        });
    }
    let wrapped: ProtocolEnvelope<T> = decode(bytes)?;
    Ok(wrapped.payload)
}

#[cfg(test)]
mod tests {
    use crate::protocol::MachineCommand;

    use super::*;

    #[test]
    fn machine_command_round_trips_through_versioned_codec() {
        let command = MachineCommand::GetPc;
        let bytes = encode_message(&command).expect("encode");
        let decoded: MachineCommand = decode_message(&bytes).expect("decode");
        assert_eq!(decoded, command);
    }

    #[test]
    fn decode_message_rejects_version_mismatch() {
        let bytes = encode(&ProtocolEnvelope {
            protocol_version: PROTOCOL_VERSION + 1,
            payload: MachineCommand::GetPc,
        })
        .expect("encode");

        let err = decode_message::<MachineCommand>(&bytes).expect_err("must fail");
        assert!(matches!(
            err,
            CodecError::ProtocolVersionMismatch {
                expected: PROTOCOL_VERSION,
                got
            } if got == PROTOCOL_VERSION + 1
        ));
    }

    #[test]
    fn the_version_is_reported_even_when_the_payload_has_another_shape() {
        // A skewed plugin: right envelope, a payload this schema cannot read.
        let bytes = encode(&ProtocolEnvelope {
            protocol_version: PROTOCOL_VERSION + 3,
            payload: ("a different", "schema entirely", 99_u64),
        })
        .expect("encode");
        let err = decode_message::<MachineCommand>(&bytes).expect_err("must fail");
        assert!(
            matches!(err, CodecError::ProtocolVersionMismatch { got, .. } if got == PROTOCOL_VERSION + 3),
            "{err}"
        );
    }

    #[test]
    fn the_envelope_starts_with_the_version_as_a_varint() {
        // Frozen wire fact the pre-payload version check relies on.
        let bytes = encode_message(&MachineCommand::GetPc).expect("encode");
        assert_eq!(bytes[0] as u16, PROTOCOL_VERSION);
    }

    #[test]
    fn a_length_prefix_beyond_the_limit_is_an_error_not_an_allocation() {
        // Current version, then a `Vec<u8>` claiming 2^40 elements.
        let mut bytes = vec![PROTOCOL_VERSION as u8, 253];
        bytes.extend_from_slice(&(1u64 << 40).to_le_bytes());
        let err = decode_message::<Vec<u8>>(&bytes).expect_err("must fail");
        assert!(matches!(err, CodecError::Decode(_)), "{err}");
    }
}
