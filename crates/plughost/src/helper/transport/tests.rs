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

#[test]
#[cfg(target_os = "macos")]
fn a_peer_closed_before_accept_keeps_its_queued_hello_readable() {
    let name = format!("plughost-transport-test-{:016x}", super::super::random());
    let listener = ListenerOptions::new()
        .name(name.as_str().to_ns_name::<GenericNamespaced>().unwrap())
        .create_sync()
        .unwrap();
    let mut peer =
        Stream::connect(name.as_str().to_ns_name::<GenericNamespaced>().unwrap()).unwrap();
    write_message(
        &mut peer,
        &Hello {
            protocol: PROTOCOL_VERSION,
            token: 42,
        },
    )
    .unwrap();
    drop(peer);
    let stream = listener.accept().expect("accept disconnected peer");
    let mut transport = Transport::new(stream).expect("initialize disconnected peer");
    assert!(matches!(
        transport.receive(Duration::from_secs(2)),
        Ok(Ok(Incoming::Hello(Hello { token: 42, .. })))
    ));
    assert!(matches!(
        transport.receive(Duration::from_secs(2)),
        Err(Stopped::Closed)
    ));
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
#[ignore = "supervised socket benchmark; run in release mode with --nocapture"]
fn supervised_framing_benchmark() {
    use plughost_core::ipc::MessageWriter;
    for payload in [0, 16 << 20] {
        let rounds = if payload == 0 { 20_000 } else { 4 };
        let request = if payload == 0 {
            Request::Reset
        } else {
            Request::ImportPreset {
                slot: 0,
                bytes: vec![42; payload].into(),
            }
        };
        let (mut transport, mut peer) = connection();
        identify(&mut transport, &mut peer);
        let worker = std::thread::spawn(move || {
            let mut reader = MessageReader::new(&peer);
            let mut writer = MessageWriter::new(&peer);
            for _ in 0..rounds {
                let received = reader.read::<Request>().unwrap();
                if payload == 0 {
                    assert_eq!(received, Request::Reset);
                } else {
                    let Request::ImportPreset { slot, bytes } = received else {
                        panic!("wrong request")
                    };
                    assert_eq!(slot, 0);
                    assert_eq!(bytes.len(), payload);
                    assert!(bytes.iter().all(|&byte| byte == 42));
                }
                writer.write(&Response::Done).unwrap();
            }
        });
        let timeout = Duration::from_secs(5);
        let started = Instant::now();
        for _ in 0..rounds {
            transport.queue(&request).unwrap();
            transport.flush(timeout).unwrap();
            assert!(matches!(
                transport.receive(timeout),
                Ok(Ok(Incoming::Response(Response::Done)))
            ));
        }
        let elapsed = started.elapsed();
        worker.join().unwrap();
        writeln!(
            std::io::stdout(),
            "payload={payload} rounds={rounds} ms={:.3}",
            elapsed.as_secs_f64() * 1000.0
        )
        .unwrap();
    }
}

#[test]
fn a_timeout_past_any_deadline_waits_for_the_message() {
    let (mut transport, mut peer) = connection();
    identify(&mut transport, &mut peer);
    transport.queue(&Request::Changes).unwrap();
    transport.flush(Duration::MAX).unwrap();
    assert!(matches!(
        MessageReader::new(&peer).read::<Request>(),
        Ok(Request::Changes)
    ));
    let writer = std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(50));
        write_message(&mut peer, &Response::EditorOpen(true)).unwrap();
        peer
    });
    assert!(matches!(
        transport.receive(Duration::MAX),
        Ok(Ok(Incoming::Response(Response::EditorOpen(true))))
    ));
    drop(writer.join().unwrap());
    transport.close();
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
        plugins: vec![plughost_core::PluginRef::clap(
            "x.clap",
            "x".repeat(8 * 1024 * 1024),
        )],
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
        plugins: vec![plughost_core::PluginRef::clap(
            "x.clap",
            (0..8 * 1024 * 1024)
                .map(|index| char::from(b'a' + (index % 26) as u8))
                .collect::<String>(),
        )],
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
        state: std::borrow::Cow::Owned(plughost_core::PluginState::new(
            &plughost_core::PluginInfo::new(
                plughost_core::PluginId::new(plughost_core::PluginFormat::Clap, ""),
                "",
                plughost_core::PluginKind::Effect,
            ),
            vec![0; plughost_core::ipc::MAX_MESSAGE_BYTES],
            None,
        )),
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

#[cfg(target_os = "macos")]
#[test]
fn disconnected_writes_do_not_signal_the_embedding_process() {
    const CHILD: &str = "PLUGHOST_TEST_DEFAULT_SIGPIPE";
    if std::env::var_os(CHILD).is_some() {
        // SAFETY: this subprocess runs only this test. Model a C/Swift embedding process,
        // which need not ignore SIGPIPE as a Rust executable normally does.
        unsafe { libc::signal(libc::SIGPIPE, libc::SIG_DFL) };
        let (mut transport, peer) = connection();
        drop(peer);
        transport.queue(&Request::Shutdown).unwrap();
        assert!(matches!(
            transport.flush(Duration::from_secs(1)),
            Err(Stopped::Closed)
        ));
        return;
    }
    let output = std::process::Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "helper::transport::tests::disconnected_writes_do_not_signal_the_embedding_process",
            "--nocapture",
        ])
        .env(CHILD, "1")
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "child ended with {}: {}",
        output.status,
        String::from_utf8_lossy(&output.stderr)
    );
}
