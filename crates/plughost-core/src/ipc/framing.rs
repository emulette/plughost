//! Per-connection framing storage; owned decoded messages remain independent of the scratch.
#[cfg(test)]
#[path = "framing_read_tests.rs"]
mod read_tests;
use super::MAX_MESSAGE_BYTES;
use crate::errors::{MALFORMED_MESSAGE, MESSAGE_TOO_LARGE};
use serde::{Serialize, de::DeserializeOwned};
use std::io::{self, Read, Write};

/// Scratch retained between messages. Larger frames allocate for their own duration only.
const RETAINED_BYTES: usize = 64 << 10;
const PREFIX_BYTES: usize = size_of::<u32>();

/// A framed reader that retains bounded scratch between messages.
/// Decoding owned strings/vectors may still allocate. This is blocking control/offline I/O.
/// Keep the same reader for the connection: it reads ahead into subsequent frames.
pub struct MessageReader<R> {
    reader: R,
    decoder: FrameDecoder,
}
impl<R: Read> MessageReader<R> {
    pub fn new(reader: R) -> Self {
        Self {
            reader,
            decoder: FrameDecoder::default(),
        }
    }
    pub fn read<T: DeserializeOwned>(&mut self) -> io::Result<T> {
        loop {
            if let Some(message) = self.decoder.message() {
                return message;
            }
            match self.reader.read(self.decoder.unfilled()?) {
                Ok(0) => return Err(io::ErrorKind::UnexpectedEof.into()),
                Ok(count) => self.decoder.advance(count),
                Err(error) if error.kind() == io::ErrorKind::Interrupted => {}
                Err(error) => return Err(error),
            }
        }
    }
}

/// A framed writer that retains bounded serialization scratch between messages.
/// Serialization completes before the frame is written with a single write call.
pub struct MessageWriter<W> {
    writer: W,
    bytes: Vec<u8>,
}
impl<W: Write> MessageWriter<W> {
    pub fn new(writer: W) -> Self {
        Self {
            writer,
            bytes: Vec::new(),
        }
    }
    pub fn write<T: Serialize>(&mut self, message: &T) -> io::Result<()> {
        self.write_bounded(message, MAX_MESSAGE_BYTES)
    }
    fn write_bounded<T: Serialize>(&mut self, message: &T, limit: usize) -> io::Result<()> {
        let result = self.encode(message, limit).and_then(|()| {
            self.writer.write_all(&self.bytes)?;
            self.writer.flush()
        });
        self.bytes.clear();
        self.bytes.shrink_to(RETAINED_BYTES);
        result
    }

    fn encode<T: Serialize>(&mut self, message: &T, limit: usize) -> io::Result<()> {
        encode_bounded(&mut self.bytes, message, limit)
    }
}

/// Encodes one frame into `bytes`, replacing its contents and keeping its capacity, for a writer
/// that sends it in pieces. A failed serialization leaves no partial frame.
pub fn encode_message<T: Serialize>(bytes: &mut Vec<u8>, message: &T) -> io::Result<()> {
    encode_bounded(bytes, message, MAX_MESSAGE_BYTES)
}

/// Encodes the length prefix and payload. A failed serialization never emits a partial frame.
fn encode_bounded<T: Serialize>(bytes: &mut Vec<u8>, message: &T, limit: usize) -> io::Result<()> {
    bytes.clear();
    // Sized first: growing by doubling could hold up to twice a large frame, depending on the
    // capacity earlier frames left, and an oversized message allocates nothing.
    let size = postcard::serialize_with_flavor(message, postcard::ser_flavors::Size::default())
        .map_err(serialization_error)?;
    if size > limit {
        return Err(serialization_error(postcard::Error::SerializeBufferFull));
    }
    bytes.reserve_exact(PREFIX_BYTES + size);
    bytes.extend_from_slice(&[0; PREFIX_BYTES]);
    let buffer = MessageBuffer {
        bytes: &mut *bytes,
        limit: PREFIX_BYTES + limit,
    };
    let result = postcard::serialize_with_flavor(message, buffer).map_err(serialization_error);
    if let Err(error) = result {
        bytes.clear();
        return Err(error);
    }
    let length = (bytes.len() - PREFIX_BYTES) as u32;
    bytes[..PREFIX_BYTES].copy_from_slice(&length.to_le_bytes());
    Ok(())
}

fn serialization_error(error: postcard::Error) -> io::Error {
    if error == postcard::Error::SerializeBufferFull {
        io::Error::new(io::ErrorKind::InvalidInput, MESSAGE_TOO_LARGE)
    } else {
        io::Error::new(io::ErrorKind::InvalidInput, error)
    }
}

/// Decodes frames from reads of any size and keeps a partial frame between them, so a reader can
/// stop at a deadline and continue later without losing data.
#[derive(Default)]
pub struct FrameDecoder {
    /// Initialized once and reused without clearing between frames.
    bytes: Vec<u8>,
    consumed: usize,
    filled: usize,
    /// Large payloads grow from received slices without zero-initializing unread bytes.
    large: Option<LargePayload>,
}

struct LargePayload {
    length: usize,
    bytes: Vec<u8>,
}

impl FrameDecoder {
    /// Space for the next read, which may contain several frames. Call [`Self::message`] until
    /// it returns `None` before reading more; leftover bytes are kept across calls.
    pub fn unfilled(&mut self) -> io::Result<&mut [u8]> {
        if self.large.is_none()
            && self
                .length()
                .is_some_and(|length| length > MAX_MESSAGE_BYTES)
        {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                MESSAGE_TOO_LARGE,
            ));
        }
        if self.bytes.is_empty() {
            self.bytes.resize(RETAINED_BYTES, 0);
        }
        if self.consumed != 0 {
            self.bytes.copy_within(self.consumed..self.filled, 0);
            self.filled -= self.consumed;
            self.consumed = 0;
        }
        Ok(&mut self.bytes[self.filled..])
    }

    /// Records that `count` bytes were read into [`FrameDecoder::unfilled`].
    pub fn advance(&mut self, count: usize) {
        self.filled += count;
    }

    /// The next frame, once all of its bytes arrived. Following frames stay buffered, and large
    /// payload storage is released after decoding. Partial large payloads consume received bytes.
    pub fn message<T: DeserializeOwned>(&mut self) -> Option<io::Result<T>> {
        if self.large.is_none() {
            let length = self.length()?;
            if length > MAX_MESSAGE_BYTES {
                return None;
            }
            let begin = self.consumed + PREFIX_BYTES;
            if length + PREFIX_BYTES <= RETAINED_BYTES {
                let end = begin + length;
                if self.filled < end {
                    return None;
                }
                let message = decode(&self.bytes[begin..end]);
                self.consumed = end;
                return Some(message);
            }
            self.consumed = begin;
            self.large = Some(LargePayload {
                length,
                bytes: Vec::with_capacity(length),
            });
        }
        let payload = self.large.as_mut().unwrap();
        let take = (payload.length - payload.bytes.len()).min(self.filled - self.consumed);
        payload
            .bytes
            .extend_from_slice(&self.bytes[self.consumed..self.consumed + take]);
        self.consumed += take;
        if payload.bytes.len() != payload.length {
            return None;
        }
        let message = decode(&payload.bytes);
        self.large = None;
        Some(message)
    }

    fn length(&self) -> Option<usize> {
        (self.filled - self.consumed >= PREFIX_BYTES).then(|| {
            u32::from_le_bytes(
                self.bytes[self.consumed..self.consumed + PREFIX_BYTES]
                    .try_into()
                    .unwrap(),
            ) as usize
        })
    }
}

/// Writes a single frame. Use [`MessageWriter`] to reuse scratch across a connection.
pub fn write_message<W: Write, T: Serialize>(writer: &mut W, message: &T) -> io::Result<()> {
    MessageWriter::new(writer).write(message)
}

struct MessageBuffer<'a> {
    bytes: &'a mut Vec<u8>,
    limit: usize,
}

impl postcard::ser_flavors::Flavor for MessageBuffer<'_> {
    type Output = ();

    fn try_push(&mut self, byte: u8) -> postcard::Result<()> {
        self.try_extend(&[byte])
    }

    fn try_extend(&mut self, bytes: &[u8]) -> postcard::Result<()> {
        if bytes.len() > self.limit - self.bytes.len() {
            return Err(postcard::Error::SerializeBufferFull);
        }
        self.bytes.extend_from_slice(bytes);
        Ok(())
    }

    fn finalize(self) -> postcard::Result<Self::Output> {
        Ok(())
    }
}

/// Decodes one payload, which must be exactly one message.
fn decode<T: DeserializeOwned>(bytes: &[u8]) -> io::Result<T> {
    let (message, remaining) = postcard::take_from_bytes(bytes)
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, MALFORMED_MESSAGE))?;
    if !remaining.is_empty() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            MALFORMED_MESSAGE,
        ));
    }
    Ok(message)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ipc::Request;

    #[test]
    fn messages_round_trip_through_the_framing() {
        let request = Request::ImportPreset {
            slot: 0,
            bytes: vec![42; 128].into(),
        };
        let mut wire = Vec::new();
        write_message(&mut wire, &request).unwrap();
        assert_eq!(wire[..4], ((wire.len() - 4) as u32).to_le_bytes());
        let read: Request = MessageReader::new(wire.as_slice()).read().unwrap();
        assert_eq!(read, request);
    }

    #[test]
    fn oversized_and_malformed_messages_are_rejected() {
        let huge = ((MAX_MESSAGE_BYTES + 1) as u32).to_le_bytes();
        let error = MessageReader::new(huge.as_slice())
            .read::<Request>()
            .unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::InvalidData);

        let mut garbage = 3u32.to_le_bytes().to_vec();
        garbage.extend([0xFF, 0xFF, 0xFF]);
        let error = MessageReader::new(garbage.as_slice())
            .read::<Request>()
            .unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::InvalidData);
    }

    #[test]
    fn framing_keeps_consecutive_messages_separate_and_rejects_truncation() {
        let mut wire = vec![];
        write_message(&mut wire, &Request::Reset).unwrap();
        write_message(&mut wire, &Request::Shutdown).unwrap();
        let mut reader = MessageReader::new(wire.as_slice());
        assert_eq!(reader.read::<Request>().unwrap(), Request::Reset);
        assert_eq!(reader.read::<Request>().unwrap(), Request::Shutdown);
        for bytes in [&[1, 0][..], &[2, 0, 0, 0, 0][..]] {
            assert_eq!(
                MessageReader::new(&mut &*bytes)
                    .read::<Request>()
                    .unwrap_err()
                    .kind(),
                io::ErrorKind::UnexpectedEof
            );
        }
    }

    #[test]
    fn reused_writer_recovers_after_an_oversized_message_without_emitting_a_prefix() {
        let mut wire = Vec::new();
        let mut writer = MessageWriter::new(&mut wire);
        writer.write(&Request::Reset).unwrap();
        let oversized = Request::ImportPreset {
            slot: 0,
            bytes: vec![42; 128].into(),
        };
        assert_eq!(
            writer.write_bounded(&oversized, 8).unwrap_err().kind(),
            io::ErrorKind::InvalidInput
        );
        writer.write(&Request::Shutdown).unwrap();
        let mut reader = MessageReader::new(wire.as_slice());
        assert_eq!(reader.read::<Request>().unwrap(), Request::Reset);
        assert_eq!(reader.read::<Request>().unwrap(), Request::Shutdown);
        assert_eq!(
            reader.read::<Request>().unwrap_err().kind(),
            io::ErrorKind::UnexpectedEof
        );
    }

    #[test]
    fn a_large_frame_takes_its_size_whatever_capacity_earlier_frames_left() {
        let request = Request::ImportPreset {
            slot: 0,
            bytes: vec![42; 1 << 20].into(),
        };
        for retained in [0, 256, 1000] {
            let mut bytes = Vec::with_capacity(retained);
            encode_message(&mut bytes, &request).unwrap();
            assert_eq!(bytes.capacity(), bytes.len(), "retained {retained}");
        }
    }

    #[test]
    fn the_decoder_resumes_frames_split_at_any_byte_and_rejects_oversized_prefixes() {
        let mut wire = Vec::new();
        encode_message(&mut wire, &Request::Reset).unwrap();
        let mut second = Vec::new();
        encode_message(&mut second, &Request::Shutdown).unwrap();
        wire.extend_from_slice(&second);
        let mut decoder = FrameDecoder::default();
        let mut received = Vec::new();
        for byte in wire {
            assert!(decoder.message::<Request>().is_none());
            decoder.unfilled().unwrap()[0] = byte;
            decoder.advance(1);
            if let Some(message) = decoder.message::<Request>() {
                received.push(message.unwrap());
            }
        }
        assert_eq!(received, [Request::Reset, Request::Shutdown]);

        let mut oversized = FrameDecoder::default();
        oversized.unfilled().unwrap()[..PREFIX_BYTES]
            .copy_from_slice(&u32::try_from(MAX_MESSAGE_BYTES + 1).unwrap().to_le_bytes());
        oversized.advance(PREFIX_BYTES);
        assert!(oversized.message::<Request>().is_none());
        assert_eq!(
            oversized.unfilled().unwrap_err().kind(),
            io::ErrorKind::InvalidData
        );
    }

    #[test]
    fn reused_reader_separates_owned_payloads_and_rejects_trailing_data_after_a_large_frame() {
        let large = Request::ImportPreset {
            slot: 0,
            bytes: vec![42; 4096].into(),
        };
        let mut wire = Vec::new();
        write_message(&mut wire, &large).unwrap();
        let mut invalid = postcard::to_stdvec(&Request::Reset).unwrap();
        invalid.push(0);
        wire.extend_from_slice(&(invalid.len() as u32).to_le_bytes());
        wire.extend_from_slice(&invalid);
        write_message(&mut wire, &Request::Shutdown).unwrap();
        let mut reader = MessageReader::new(wire.as_slice());
        let received: Request = reader.read().unwrap();
        assert_eq!(
            reader.read::<Request>().unwrap_err().kind(),
            io::ErrorKind::InvalidData
        );
        assert_eq!(reader.read::<Request>().unwrap(), Request::Shutdown);
        assert_eq!(received, large);
    }
}
