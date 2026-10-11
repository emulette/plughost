//! IPC corpus and fragmentation oracle, using the real framed reader and response decoder.
use super::{MessageReader, Response, encode_message};
use std::io::{self, Read};

type Decoded = Vec<Result<Vec<u8>, io::ErrorKind>>;

struct Chunks<'a> {
    bytes: &'a [u8],
    limit: usize,
}
impl Read for Chunks<'_> {
    fn read(&mut self, output: &mut [u8]) -> io::Result<usize> {
        let count = self.bytes.len().min(output.len()).min(self.limit);
        output[..count].copy_from_slice(&self.bytes[..count]);
        self.bytes = &self.bytes[count..];
        Ok(count)
    }
}

fn decode(bytes: &[u8], limit: usize) -> Decoded {
    let mut reader = MessageReader::new(Chunks { bytes, limit });
    let mut messages = Vec::new();
    loop {
        match reader.read::<Response>() {
            Ok(message) => {
                let mut encoded = Vec::new();
                encode_message(&mut encoded, &message).unwrap();
                messages.push(Ok(encoded));
            }
            Err(error) => {
                messages.push(Err(error.kind()));
                return messages;
            }
        }
    }
}

/// Fragmentation and read-ahead must preserve messages and the terminal error.
pub fn framing(bytes: &[u8]) {
    let small = if bytes.len() < 256 { 1 } else { 31 };
    assert_eq!(decode(bytes, small), decode(bytes, 4096));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn corpus_framing_retains_messages_and_rejects_bad_or_incomplete_frames() {
        let valid = include_bytes!("corpus/framing/two-done");
        framing(valid);
        let messages = decode(valid, 1);
        // Independently encoded protocol enum tag 14, twice, each in a one-byte frame.
        assert_eq!(
            messages,
            [
                Ok(vec![1, 0, 0, 0, 14]),
                Ok(vec![1, 0, 0, 0, 14]),
                Err(io::ErrorKind::UnexpectedEof)
            ]
        );
        for (bytes, kind) in [
            (
                &include_bytes!("corpus/framing/truncated-prefix")[..],
                io::ErrorKind::UnexpectedEof,
            ),
            (
                &include_bytes!("corpus/framing/truncated-body")[..],
                io::ErrorKind::UnexpectedEof,
            ),
            (
                &include_bytes!("corpus/framing/oversized")[..],
                io::ErrorKind::InvalidData,
            ),
            (
                &include_bytes!("corpus/framing/invalid-tag")[..],
                io::ErrorKind::InvalidData,
            ),
            (
                &include_bytes!("corpus/framing/trailing-body")[..],
                io::ErrorKind::InvalidData,
            ),
        ] {
            framing(bytes);
            assert_eq!(decode(bytes, 1), [Err(kind)]);
        }
    }
}
