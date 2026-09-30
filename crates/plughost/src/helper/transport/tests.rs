use std::io::{Read, Write};
use std::time::{Duration, Instant};

use interprocess::local_socket::prelude::*;
use interprocess::local_socket::{GenericNamespaced, ListenerOptions, Stream};
use plughost_core::ipc::{
    Hello, MessageReader, PROTOCOL_VERSION, Request, Response, write_message,
};

use super::*;

fn connection() -> (Transport, Stream) {
    let name = format!("plughost-transport-test-{:016x}", super::super::random());
    let listener = ListenerOptions::new()
        .name(name.as_str().to_ns_name::<GenericNamespaced>().unwrap())
        .create_sync()
        .unwrap();
    let peer = Stream::connect(name.as_str().to_ns_name::<GenericNamespaced>().unwrap()).unwrap();
    let transport = Transport::new(listener.accept().unwrap()).unwrap();
    (transport, peer)
}

fn identify(transport: &mut Transport, peer: &mut Stream) {
    write_message(
        peer,
        &Hello {
            protocol: PROTOCOL_VERSION,
            token: 42,
        },
    )
    .unwrap();
    assert!(matches!(
        transport.receive(Duration::from_secs(2)),
        Ok(Ok(Incoming::Hello(Hello { token: 42, .. })))
    ));
}

#[test]
fn an_incomplete_frame_does_not_block_the_supervisors_deadline() {
    let (mut transport, mut peer) = connection();
    identify(&mut transport, &mut peer);
    let mut frame = Vec::new();
    write_message(&mut frame, &Response::EditorOpen(true)).unwrap();
    peer.write_all(&frame[..2]).unwrap();
    let started = Instant::now();
    assert!(matches!(
        transport.receive(Duration::from_millis(50)),
        Err(Stopped::TimedOut)
    ));
    assert!(started.elapsed() < Duration::from_secs(2));
    peer.write_all(&frame[2..]).unwrap();
    assert!(matches!(
        transport.receive(Duration::from_secs(2)),
        Ok(Ok(Incoming::Response(Response::EditorOpen(true))))
    ));
    drop(peer);
    transport.close();
}

#[test]
fn an_unread_request_does_not_block_the_supervisor_or_cleanup() {
    let (mut transport, mut peer) = connection();
    identify(&mut transport, &mut peer);
    let request = Request::Load {
        host: plughost_core::HostIdentity::default(),
        plugins: vec![plughost_core::PluginRef {
            format: plughost_core::PluginFormat::Clap,
            bundle: None,
            class_id: "x".repeat(8 * 1024 * 1024),
        }],
        activity: 0,
    };
    let started = Instant::now();
    transport.queue(&request).unwrap();
    assert!(matches!(
        transport.flush(Duration::from_millis(50)),
        Err(Stopped::TimedOut)
    ));
    assert!(matches!(
        transport.receive(Duration::from_millis(50)),
        Err(Stopped::TimedOut)
    ));
    drop(peer);
    transport.close();
    assert!(started.elapsed() < Duration::from_secs(2));
}

/// Reads in small pieces with pauses, like a helper busy between reads.
struct Slow(Stream);

impl Read for Slow {
    fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
        std::thread::sleep(Duration::from_micros(200));
        let length = buffer.len().min(16 * 1024);
        self.0.read(&mut buffer[..length])
    }
}

#[test]
fn a_large_request_flushed_in_short_slices_arrives_intact() {
    let (mut transport, mut peer) = connection();
    identify(&mut transport, &mut peer);
    let request = Request::Load {
        host: plughost_core::HostIdentity::default(),
        plugins: vec![plughost_core::PluginRef {
            format: plughost_core::PluginFormat::Clap,
            bundle: None,
            class_id: (0..8 * 1024 * 1024)
                .map(|index| char::from(b'a' + (index % 26) as u8))
                .collect(),
        }],
        activity: 0,
    };
    let reader = std::thread::spawn(move || MessageReader::new(Slow(peer)).read::<Request>());
    transport.queue(&request).unwrap();
    // Each slice ends mid-write, as the supervisor's polling flushes do.
    loop {
        match transport.flush(Duration::from_millis(1)) {
            Ok(()) => break,
            Err(Stopped::TimedOut) => continue,
            Err(Stopped::Closed) => panic!("the connection closed"),
        }
    }
    assert_eq!(reader.join().unwrap().unwrap(), request);
    transport.close();
}

#[test]
fn a_request_too_large_for_a_message_is_not_mistaken_for_a_closed_connection() {
    let (mut transport, mut peer) = connection();
    identify(&mut transport, &mut peer);
    let oversized = Request::LoadState {
        slot: 0,
        state: plughost_core::PluginState {
            format: plughost_core::PluginFormat::Clap,
            class_id: String::new(),
            name: String::new(),
            vendor: String::new(),
            version: String::new(),
            component: vec![0; plughost_core::ipc::MAX_MESSAGE_BYTES],
            controller: Vec::new(),
        },
    };
    assert!(matches!(
        transport.queue(&oversized),
        Err(Unqueued::Unencodable)
    ));
    // Nothing was written; the connection carries the next request.
    let reader = std::thread::spawn(move || MessageReader::new(peer).read::<Request>());
    transport.queue(&Request::Shutdown).unwrap();
    transport.flush(Duration::from_secs(2)).unwrap();
    assert_eq!(reader.join().unwrap().unwrap(), Request::Shutdown);
    transport.close();
}
