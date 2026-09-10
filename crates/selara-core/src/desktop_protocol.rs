//! Versioned, line-oriented protocol between the Settings app and `serve`.
//!
//! The parser is deliberately bounded: a malformed or unterminated line cannot
//! grow without limit while the desktop app is waiting for a response.

use serde::{Deserialize, Serialize};

pub const PROTOCOL_VERSION: u32 = 1;
pub const MAX_LINE_BYTES: usize = 64 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AxTrust {
    Unknown,
    Granted,
    Missing,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ServeReadiness {
    Starting,
    Ready,
    Busy,
    Quiescing,
    Quiesced,
    Stopped,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ServeStatus {
    pub version: u32,
    pub id: Option<String>,
    pub readiness: ServeReadiness,
    pub ax_trust: AxTrust,
    pub generation: u64,
    pub external: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "command", rename_all = "snake_case")]
pub enum ProtocolCommand {
    Status,
    RequestPermission,
    Quiesce,
    Resume,
    ReloadAuth,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProtocolRequest {
    pub version: u32,
    pub id: String,
    #[serde(flatten)]
    pub command: ProtocolCommand,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProtocolResponse {
    pub version: u32,
    pub id: Option<String>,
    pub ok: bool,
    pub status: ServeStatus,
    pub error: Option<String>,
}

impl ProtocolResponse {
    pub fn ok(id: Option<String>, status: ServeStatus) -> Self {
        Self {
            version: PROTOCOL_VERSION,
            id,
            ok: true,
            status,
            error: None,
        }
    }

    pub fn error(id: Option<String>, status: ServeStatus, error: impl Into<String>) -> Self {
        Self {
            version: PROTOCOL_VERSION,
            id,
            ok: false,
            status,
            error: Some(error.into()),
        }
    }
}

/// Fixed-size callers feed this decoder. Errors belong to individual frames,
/// so malformed data never discards adjacent valid messages in the same read.
pub struct JsonLines<T> {
    buf: Vec<u8>,
    dropping: bool,
    marker: std::marker::PhantomData<T>,
}
impl<T> Default for JsonLines<T> {
    fn default() -> Self {
        Self {
            buf: Vec::new(),
            dropping: false,
            marker: std::marker::PhantomData,
        }
    }
}
impl<T: serde::de::DeserializeOwned> JsonLines<T> {
    pub fn push(&mut self, bytes: &[u8]) -> Vec<Result<T, String>> {
        let mut out = Vec::new();
        for &byte in bytes {
            if byte == b'\n' {
                if !self.dropping && !self.buf.is_empty() {
                    out.push(
                        serde_json::from_slice(&self.buf)
                            .map_err(|e| format!("invalid protocol frame: {e}")),
                    );
                }
                self.buf.clear();
                self.dropping = false;
            } else if !self.dropping {
                if self.buf.len() == MAX_LINE_BYTES {
                    self.buf.clear();
                    self.dropping = true;
                    out.push(Err(format!("protocol line exceeds {MAX_LINE_BYTES} bytes")));
                } else {
                    self.buf.push(byte);
                }
            }
        }
        out
    }
}
pub type ProtocolDecoder = JsonLines<ProtocolRequest>;
pub type ResponseDecoder = JsonLines<ProtocolResponse>;

#[cfg(test)]
mod tests {
    use super::*;
    fn request(id: &str) -> Vec<u8> {
        let mut bytes = serde_json::to_vec(&ProtocolRequest {
            version: 1,
            id: id.into(),
            command: ProtocolCommand::Status,
        })
        .unwrap();
        bytes.push(b'\n');
        bytes
    }
    #[test]
    fn partial_chunks_are_reassembled() {
        let raw = request("a");
        let mut decoder = ProtocolDecoder::default();
        assert!(decoder.push(&raw[..4]).is_empty());
        assert_eq!(decoder.push(&raw[4..]).remove(0).unwrap().id, "a");
    }
    #[test]
    fn malformed_and_oversized_frames_preserve_both_neighbors() {
        for invalid in [
            b"invalid\n".to_vec(),
            [vec![b'x'; MAX_LINE_BYTES + 1], vec![b'\n']].concat(),
        ] {
            let mut decoder = ProtocolDecoder::default();
            let frames = decoder.push(&[request("before"), invalid, request("after")].concat());
            assert_eq!(frames.len(), 3);
            assert_eq!(frames[0].as_ref().unwrap().id, "before");
            assert!(frames[1].is_err());
            assert_eq!(frames[2].as_ref().unwrap().id, "after");
        }
    }
    #[test]
    fn unterminated_input_is_bounded_and_recovers() {
        let mut decoder = ProtocolDecoder::default();
        let chunk = vec![b'x'; 8192];
        for _ in 0..100 {
            decoder.push(&chunk);
            assert!(decoder.buf.len() <= MAX_LINE_BYTES);
        }
        assert!(decoder.push(b"\n").is_empty());
        assert_eq!(decoder.push(&request("ok")).remove(0).unwrap().id, "ok");
    }
}
