//! Bounded JSON frames for the one-request history worker protocol.
//! Callers own deadlines, descriptor transfer, schema validation and child reaping.

use serde::{de::DeserializeOwned, Serialize};
use std::io::{self, Read, Write};

#[derive(Clone, Copy)]
pub enum HistoryFrameKind {
    Request,
    Reply,
}
impl HistoryFrameKind {
    pub const fn max_bytes(self) -> usize {
        match self {
            Self::Request => 32 * 1024,
            Self::Reply => 8 * 1024 * 1024,
        }
    }
}

#[derive(Debug)]
pub enum HistoryFrameError {
    TooLarge,
    InvalidLength,
    InvalidJson,
    TrailingData,
    Io(io::Error),
}
impl From<io::Error> for HistoryFrameError {
    fn from(error: io::Error) -> Self {
        Self::Io(error)
    }
}

struct BoundedJson {
    bytes: Vec<u8>,
    limit: usize,
    exceeded: bool,
}
impl Write for BoundedJson {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        if bytes.len() > self.limit.saturating_sub(self.bytes.len()) {
            self.exceeded = true;
            return Err(io::Error::other("history frame limit"));
        }
        self.bytes.extend_from_slice(bytes);
        Ok(bytes.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

/// Encode completely within the byte limit before writing any frame bytes.
/// An IO failure while sending still invalidates the entire exchange.
pub fn write_history_frame(
    output: &mut impl Write,
    kind: HistoryFrameKind,
    value: &impl Serialize,
) -> Result<(), HistoryFrameError> {
    write_json_frame(output, kind.max_bytes(), value)
}

pub(crate) fn write_json_frame(
    output: &mut impl Write,
    limit: usize,
    value: &impl Serialize,
) -> Result<(), HistoryFrameError> {
    if limit > u32::MAX as usize {
        return Err(HistoryFrameError::InvalidLength);
    }
    let mut buffer = BoundedJson {
        bytes: Vec::new(),
        limit,
        exceeded: false,
    };
    let encoded = serde_json::to_writer(&mut buffer, value);
    if buffer.exceeded {
        return Err(HistoryFrameError::TooLarge);
    }
    encoded.map_err(|_| HistoryFrameError::InvalidJson)?;
    output.write_all(&(buffer.bytes.len() as u32).to_be_bytes())?;
    output.write_all(&buffer.bytes)?;
    Ok(())
}

/// Read one frame. The supervisor must also check EOF, child exit and publication
/// gates. Deserialized values grant no filesystem authority.
pub fn read_history_frame<T: DeserializeOwned>(
    input: &mut impl Read,
    kind: HistoryFrameKind,
) -> Result<T, HistoryFrameError> {
    read_json_frame(input, kind.max_bytes())
}

pub(crate) fn read_json_frame<T: DeserializeOwned>(
    input: &mut impl Read,
    limit: usize,
) -> Result<T, HistoryFrameError> {
    let mut prefix = [0; 4];
    input.read_exact(&mut prefix)?;
    let length = u32::from_be_bytes(prefix) as usize;
    if length == 0 {
        return Err(HistoryFrameError::InvalidLength);
    }
    if length > limit {
        return Err(HistoryFrameError::TooLarge);
    }
    let mut body = vec![0; length];
    input.read_exact(&mut body)?;
    serde_json::from_slice(&body).map_err(|_| HistoryFrameError::InvalidJson)
}

/// This can block until the peer closes. Use a supervised or deadline-bound reader.
pub fn finish_history_frames(input: &mut impl Read) -> Result<(), HistoryFrameError> {
    let mut byte = [0];
    loop {
        match input.read(&mut byte) {
            Ok(0) => return Ok(()),
            Ok(_) => return Err(HistoryFrameError::TrailingData),
            Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
            Err(error) => return Err(error.into()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde::Deserialize;
    use std::io::Cursor;

    #[derive(Debug, Serialize, Deserialize, PartialEq)]
    #[serde(deny_unknown_fields)]
    struct Message {
        id: String,
    }

    #[test]
    fn roundtrip_and_exact_encoded_limits() {
        for kind in [HistoryFrameKind::Request, HistoryFrameKind::Reply] {
            let text = "x".repeat(kind.max_bytes() - 2);
            let mut wire = Vec::new();
            write_history_frame(&mut wire, kind, &text).unwrap();
            assert_eq!(wire.len(), kind.max_bytes() + 4);
            let mut input = Cursor::new(wire);
            assert_eq!(
                read_history_frame::<String>(&mut input, kind).unwrap(),
                text
            );
            finish_history_frames(&mut input).unwrap();
            for oversized in [format!("{text}x"), "\n".repeat(kind.max_bytes() / 2)] {
                let mut output = Vec::new();
                assert!(matches!(
                    write_history_frame(&mut output, kind, &oversized),
                    Err(HistoryFrameError::TooLarge)
                ));
                assert!(output.is_empty());
            }
        }
    }

    #[test]
    fn invalid_prefixes_do_not_read_body() {
        for (length, too_large) in [(0, false), (u32::MAX, true)] {
            let mut input = Cursor::new(length.to_be_bytes());
            let result = read_history_frame::<Message>(&mut input, HistoryFrameKind::Request);
            assert!(matches!(
                (&result, too_large),
                (Err(HistoryFrameError::TooLarge), true)
                    | (Err(HistoryFrameError::InvalidLength), false)
            ));
            assert_eq!(input.position(), 4);
        }
    }

    #[test]
    fn truncated_invalid_and_extra_messages_are_refused() {
        for bytes in [vec![0, 0], vec![0, 0, 0, 3, b'{']] {
            assert!(
                matches!(read_history_frame::<Message>(&mut Cursor::new(bytes), HistoryFrameKind::Reply), Err(HistoryFrameError::Io(error)) if error.kind() == io::ErrorKind::UnexpectedEof)
            );
        }
        for body in [
            b"{\"id\":\"a\",\"extra\":1}".as_slice(),
            b"{\"id\":\"a\",\"id\":\"b\"}",
            b"{} {}",
            &[255],
        ] {
            let mut wire = (body.len() as u32).to_be_bytes().to_vec();
            wire.extend_from_slice(body);
            assert!(matches!(
                read_history_frame::<Message>(&mut Cursor::new(wire), HistoryFrameKind::Reply),
                Err(HistoryFrameError::InvalidJson)
            ));
        }
        let mut wire = Vec::new();
        for _ in 0..2 {
            write_history_frame(
                &mut wire,
                HistoryFrameKind::Reply,
                &Message { id: "one".into() },
            )
            .unwrap();
        }
        let mut input = Cursor::new(wire);
        read_history_frame::<Message>(&mut input, HistoryFrameKind::Reply).unwrap();
        assert!(matches!(
            finish_history_frames(&mut input),
            Err(HistoryFrameError::TrailingData)
        ));
    }

    #[test]
    fn short_reads_and_writes_preserve_framing() {
        struct ShortIo(Cursor<Vec<u8>>);
        impl Read for ShortIo {
            fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
                let length = buffer.len().min(1);
                self.0.read(&mut buffer[..length])
            }
        }
        impl Write for ShortIo {
            fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
                self.0.write(&bytes[..bytes.len().min(1)])
            }
            fn flush(&mut self) -> io::Result<()> {
                Ok(())
            }
        }
        let message = Message {
            id: "fragmented".into(),
        };
        let mut io = ShortIo(Cursor::new(Vec::new()));
        write_history_frame(&mut io, HistoryFrameKind::Request, &message).unwrap();
        io.0.set_position(0);
        assert_eq!(
            read_history_frame::<Message>(&mut io, HistoryFrameKind::Request).unwrap(),
            message
        );
        finish_history_frames(&mut io).unwrap();
    }
}
