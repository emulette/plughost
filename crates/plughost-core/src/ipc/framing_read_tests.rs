use super::*;
use crate::ipc::Request;

struct Counted<R> {
    reader: R,
    calls: usize,
}

impl<R: Read> Read for Counted<R> {
    fn read(&mut self, bytes: &mut [u8]) -> io::Result<usize> {
        self.calls += 1;
        self.reader.read(bytes)
    }
}

#[test]
fn one_read_can_supply_several_frames() {
    let mut wire = Vec::new();
    write_message(&mut wire, &Request::Reset).unwrap();
    write_message(&mut wire, &Request::Shutdown).unwrap();
    let mut reader = MessageReader::new(Counted {
        reader: wire.as_slice(),
        calls: 0,
    });
    assert_eq!(reader.read::<Request>().unwrap(), Request::Reset);
    assert_eq!(reader.read::<Request>().unwrap(), Request::Shutdown);
    assert_eq!(reader.reader.calls, 1);
}

#[test]
fn fragmented_and_coalesced_frames_preserve_large_payloads_and_following_messages() {
    let requests = [
        Request::Reset,
        Request::ImportPreset {
            slot: 2,
            bytes: vec![43; RETAINED_BYTES - 10].into(),
        },
        Request::ImportPreset {
            slot: 7,
            bytes: vec![79; 2 * RETAINED_BYTES + 31].into(),
        },
        Request::Shutdown,
    ];
    let mut wire = Vec::new();
    for request in &requests {
        write_message(&mut wire, request).unwrap();
    }
    for chunk in [1, 3, 31, RETAINED_BYTES - 3, RETAINED_BYTES, wire.len()] {
        let mut remaining = wire.as_slice();
        let mut decoder = FrameDecoder::default();
        let mut received = Vec::new();
        while !remaining.is_empty() {
            let space = decoder.unfilled().unwrap();
            let count = chunk.min(space.len()).min(remaining.len());
            assert!(count > 0);
            space[..count].copy_from_slice(&remaining[..count]);
            remaining = &remaining[count..];
            decoder.advance(count);
            while let Some(message) = decoder.message::<Request>() {
                received.push(message.unwrap());
            }
        }
        assert_eq!(received, requests, "chunk={chunk}");
        assert!(decoder.message::<Request>().is_none());
    }
}

#[test]
fn read_ahead_preserves_errors_after_a_valid_frame() {
    for suffix in [vec![0xFF], vec![0; RETAINED_BYTES + 1]] {
        let mut wire = Vec::new();
        write_message(&mut wire, &Request::Reset).unwrap();
        wire.extend_from_slice(&(suffix.len() as u32).to_le_bytes());
        wire.extend_from_slice(&suffix);
        write_message(&mut wire, &Request::Shutdown).unwrap();
        let mut reader = MessageReader::new(wire.as_slice());
        assert_eq!(reader.read::<Request>().unwrap(), Request::Reset);
        assert_eq!(
            reader.read::<Request>().unwrap_err().kind(),
            io::ErrorKind::InvalidData
        );
        assert_eq!(reader.read::<Request>().unwrap(), Request::Shutdown);
    }
    let mut wire = Vec::new();
    write_message(&mut wire, &Request::Reset).unwrap();
    wire.extend_from_slice(&((MAX_MESSAGE_BYTES + 1) as u32).to_le_bytes());
    let mut reader = MessageReader::new(wire.as_slice());
    assert_eq!(reader.read::<Request>().unwrap(), Request::Reset);
    assert_eq!(
        reader.read::<Request>().unwrap_err().kind(),
        io::ErrorKind::InvalidData
    );
}

#[test]
#[cfg(unix)]
#[ignore = "socket framing benchmark; run in release mode with --nocapture"]
fn socket_framing_benchmark() {
    use std::os::unix::net::UnixStream;
    use std::time::Instant;
    const ROUNDS: usize = 20_000;
    let (client, server) = UnixStream::pair().unwrap();
    let worker = std::thread::spawn(move || {
        let mut reader = MessageReader::new(Counted {
            reader: server.try_clone().unwrap(),
            calls: 0,
        });
        let mut writer = MessageWriter::new(server);
        for _ in 0..ROUNDS {
            assert_eq!(reader.read::<Request>().unwrap(), Request::Reset);
            writer.write(&Request::Shutdown).unwrap();
        }
        reader.reader.calls
    });
    let mut reader = MessageReader::new(Counted {
        reader: client.try_clone().unwrap(),
        calls: 0,
    });
    let mut writer = MessageWriter::new(client);
    let started = Instant::now();
    for _ in 0..ROUNDS {
        writer.write(&Request::Reset).unwrap();
        assert_eq!(reader.read::<Request>().unwrap(), Request::Shutdown);
    }
    let elapsed = started.elapsed();
    let server_reads = worker.join().unwrap();
    writeln!(
        std::io::stdout(),
        "rounds={ROUNDS} client_reads={} server_reads={server_reads} ms={:.3}",
        reader.reader.calls,
        elapsed.as_secs_f64() * 1000.0
    )
    .unwrap();
}
