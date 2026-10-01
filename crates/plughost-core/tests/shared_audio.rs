//! Actual subprocess mapping/handle transfer; the child touches the shared PCM and event slots.
use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;
use std::io;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use interprocess::local_socket::prelude::*;
use interprocess::local_socket::{
    GenericNamespaced, ListenerNonblockingMode, ListenerOptions, Stream,
};
use plughost_core::ipc::shared::{Descriptor, SharedAudio, SlotConfig, Submission, transfer};
use plughost_core::ipc::{MessageReader, MessageWriter};
use plughost_core::{AutomationEvent, Event, MAX_BLOCK_EVENTS, ParameterChange, SampleFormat};

const CHILD_SOCKET: &str = "PLUGHOST_SHARED_TEST_SOCKET";

thread_local! {
    static HEAP_REQUESTS: Cell<u64> = const { Cell::new(0) };
}

/// Counts the current thread's heap requests so `measured` can assert that a call does not
/// allocate.
struct Counting;

#[global_allocator]
static ALLOCATOR: Counting = Counting;

fn count() {
    HEAP_REQUESTS.with(|requests| requests.set(requests.get() + 1));
}

// SAFETY: every call is delegated unchanged to the system allocator.
unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        count();
        unsafe { System.alloc(layout) }
    }
    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        count();
        unsafe { System.alloc_zeroed(layout) }
    }
    unsafe fn dealloc(&self, pointer: *mut u8, layout: Layout) {
        count();
        unsafe { System.dealloc(pointer, layout) }
    }
    unsafe fn realloc(&self, pointer: *mut u8, layout: Layout, size: usize) -> *mut u8 {
        count();
        unsafe { System.realloc(pointer, layout, size) }
    }
}

fn measured<T>(operation: impl FnOnce() -> T) -> T {
    let before = HEAP_REQUESTS.with(Cell::get);
    let result = operation();
    assert_eq!(HEAP_REQUESTS.with(Cell::get) - before, 0);
    result
}

struct Running(Child);
impl Drop for Running {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

#[test]
#[ignore = "subprocess entry point invoked by the parent test"]
fn shared_audio_child() {
    let Ok(name) = std::env::var(CHILD_SOCKET) else {
        return;
    };
    let stream = Stream::connect(name.to_ns_name::<GenericNamespaced>().unwrap()).unwrap();
    let (read, write) = stream.split();
    let mut read = MessageReader::new(read);
    let mut write = MessageWriter::new(write);
    let receiver = transfer::Receiver::take_from_stdin().unwrap();
    let (descriptor, token): (Descriptor, u64) = read.read().unwrap();
    // SAFETY: this test's parent transferred one live anonymous file handle and never exposes it.
    let mut memory = unsafe { receiver.receive(descriptor, token) }.unwrap();
    write.write(&true).unwrap();
    let mut edits = Vec::with_capacity(MAX_BLOCK_EVENTS);
    let mut notes = Vec::with_capacity(MAX_BLOCK_EVENTS);
    let mut audio32 = vec![vec![0.0f32; descriptor.config.max_frames]; 2];
    let mut audio64 = vec![vec![0.0f64; descriptor.config.max_frames]; 2];
    while let Some(submission) = read.read::<Option<Submission>>().unwrap() {
        measured(|| match descriptor.config.sample_format {
            SampleFormat::F32 => {
                memory
                    .read_input_f32(submission, &mut audio32, &mut edits, &mut notes)
                    .unwrap();
                for channel in &mut audio32 {
                    for sample in channel {
                        *sample *= -0.5;
                    }
                }
                memory
                    .write_output_f32(submission, &[&audio32[0], &audio32[1]], &[])
                    .unwrap();
            }
            SampleFormat::F64 => {
                memory
                    .read_input_f64(submission, &mut audio64, &mut edits, &mut notes)
                    .unwrap();
                for channel in &mut audio64 {
                    for sample in channel {
                        *sample *= -0.5;
                    }
                }
                memory
                    .write_output_f64(submission, &[&audio64[0], &audio64[1]], &[])
                    .unwrap();
            }
        });
        write.write(&(edits.clone(), notes.clone())).unwrap();
    }
}

#[test]
fn anonymous_slots_round_trip_across_processes_and_remain_owned_after_child_exit() {
    for (trial, sample_format) in [SampleFormat::F32, SampleFormat::F64]
        .into_iter()
        .enumerate()
    {
        let name = format!("plughost-shared-test-{}-{trial}.sock", std::process::id());
        let listener = ListenerOptions::new()
            .name(name.as_str().to_ns_name::<GenericNamespaced>().unwrap())
            .nonblocking(ListenerNonblockingMode::Accept)
            .create_sync()
            .unwrap();
        let (sender, child_stdin) = transfer::Sender::new().unwrap();
        let mut child = Running(
            Command::new(std::env::current_exe().unwrap())
                .args(["--exact", "shared_audio_child", "--ignored", "--nocapture"])
                .env(CHILD_SOCKET, &name)
                .stdin(child_stdin)
                .stdout(Stdio::null())
                .stderr(Stdio::inherit())
                .spawn()
                .unwrap(),
        );
        let deadline = Instant::now() + Duration::from_secs(10);
        let stream = loop {
            match listener.accept() {
                Ok(stream) => break stream,
                Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                    assert!(Instant::now() < deadline);
                    assert!(child.0.try_wait().unwrap().is_none());
                    std::thread::sleep(Duration::from_millis(5));
                }
                Err(error) => panic!("{error}"),
            }
        };
        stream.set_nonblocking(false).unwrap();
        let descriptor = Descriptor {
            generation: 7,
            config: SlotConfig {
                sample_format,
                max_frames: 17,
                input_channels: 2,
                output_channels: 2,
            },
        };
        let mut memory = SharedAudio::new(descriptor).unwrap();
        let token = sender.send(&memory, &child.0).unwrap();
        let (read, write) = stream.split();
        let mut read = MessageReader::new(read);
        let mut write = MessageWriter::new(write);
        write.write(&(descriptor, token)).unwrap();
        assert!(read.read::<bool>().unwrap());
        let mut output32 = vec![vec![0.0f32; 17]; 2];
        let mut output64 = vec![vec![0.0f64; 17]; 2];
        let mut final_submission = None;
        for (index, frames) in [17, 1, 0, 9].into_iter().enumerate() {
            let edits: Vec<_> = if frames == 0 {
                vec![]
            } else {
                [0.25, 0.75]
                    .into_iter()
                    .map(|value| AutomationEvent {
                        slot: 2,
                        change: ParameterChange {
                            id: 123,
                            offset: 0,
                            value,
                        },
                    })
                    .collect()
            };
            let notes = if frames == 0 {
                vec![]
            } else {
                vec![Event::note_on(0, 1, 64, 100), Event::note_off(0, 1, 64, 0)]
            };
            let input32 = vec![0.25f32; frames];
            let input64 = vec![1.0 + f64::EPSILON; frames];
            let submission = measured(|| match sample_format {
                SampleFormat::F32 => memory.write_input_f32(
                    index as u64 + 1,
                    frames,
                    &[&input32, &input32],
                    &edits,
                    &notes,
                ),
                SampleFormat::F64 => memory.write_input_f64(
                    index as u64 + 1,
                    frames,
                    &[&input64, &input64],
                    &edits,
                    &notes,
                ),
            })
            .unwrap();
            write.write(&Some(submission)).unwrap();
            let received: (Vec<AutomationEvent>, Vec<Event>) = read.read().unwrap();
            assert_eq!(received, (edits, notes));
            if index == 3 {
                // Terminate without Drop/Shutdown in the peer. The owner's map remains readable.
                child.0.kill().unwrap();
                child.0.wait().unwrap();
            }
            match sample_format {
                SampleFormat::F32 => {
                    let mut views: Vec<&mut [f32]> =
                        output32.iter_mut().map(|c| &mut c[..frames]).collect();
                    measured(|| memory.read_output_f32(submission, &mut views, 0, &mut Vec::new()))
                        .unwrap();
                    assert!(
                        output32
                            .iter()
                            .all(|channel| channel[..frames] == vec![-0.125; frames])
                    );
                }
                SampleFormat::F64 => {
                    let mut views: Vec<&mut [f64]> =
                        output64.iter_mut().map(|c| &mut c[..frames]).collect();
                    measured(|| memory.read_output_f64(submission, &mut views, 0, &mut Vec::new()))
                        .unwrap();
                    assert!(
                        output64.iter().all(|channel| channel[..frames]
                            == vec![-0.5 * (1.0 + f64::EPSILON); frames])
                    );
                }
            }
            final_submission = Some(submission);
        }
        let mut replacement = SharedAudio::new(Descriptor {
            generation: 8,
            ..descriptor
        })
        .unwrap();
        assert!(
            replacement
                .read_output_f32(
                    final_submission.unwrap(),
                    &mut output32.iter_mut().map(|c| &mut c[..9]).collect::<Vec<_>>(),
                    0,
                    &mut Vec::new()
                )
                .is_err()
        );
        assert!(sender.send(&replacement, &child.0).is_err());
    }
}
