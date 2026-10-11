//! Additional application heap used while sending caller-owned state to a native helper.
use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;
use std::io::Write;
use std::time::{Duration, Instant};

use plughost::{PluginFormat, StatePurpose};

mod support;

#[derive(Clone, Copy, Default)]
struct Heap {
    enabled: bool,
    bytes: isize,
    peak: isize,
}

thread_local! {
    static HEAP: Cell<Heap> = const { Cell::new(Heap { enabled: false, bytes: 0, peak: 0 }) };
}

fn record(delta: isize) {
    let _ = HEAP.try_with(|cell| {
        let mut heap = cell.get();
        if heap.enabled {
            heap.bytes += delta;
            heap.peak = heap.peak.max(heap.bytes);
            cell.set(heap);
        }
    });
}

struct Counting;
#[global_allocator]
static ALLOCATOR: Counting = Counting;

// SAFETY: allocation and deallocation are delegated unchanged to the system allocator.
unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let pointer = unsafe { System.alloc(layout) };
        if !pointer.is_null() {
            record(layout.size() as isize);
        }
        pointer
    }
    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        let pointer = unsafe { System.alloc_zeroed(layout) };
        if !pointer.is_null() {
            record(layout.size() as isize);
        }
        pointer
    }
    unsafe fn dealloc(&self, pointer: *mut u8, layout: Layout) {
        unsafe { System.dealloc(pointer, layout) };
        record(-(layout.size() as isize));
    }
    unsafe fn realloc(&self, pointer: *mut u8, layout: Layout, size: usize) -> *mut u8 {
        let result = unsafe { System.realloc(pointer, layout, size) };
        if !result.is_null() {
            record(size as isize - layout.size() as isize);
        }
        result
    }
}

struct Measurement {
    heap: Heap,
    elapsed: Duration,
}

fn measure<T>(action: impl FnOnce() -> T) -> (T, Measurement) {
    HEAP.set(Heap {
        enabled: true,
        ..Heap::default()
    });
    let started = Instant::now();
    let result = action();
    let elapsed = started.elapsed();
    let heap = HEAP.replace(Heap::default());
    (result, Measurement { heap, elapsed })
}

fn transfer(payload: usize) -> [Measurement; 2] {
    let mut chain = support::spawn(&[support::delay(PluginFormat::Clap)]);
    chain.set_parameter(0, 0, 0.375).unwrap();
    let mut state = chain.save_state(0, StatePurpose::Project).unwrap();
    // This fixture reads the initial gain and ignores the remaining native state bytes.
    state.component.resize(payload, 42);
    let (_, restore) = measure(|| {
        chain
            .restore_state(0, &state, StatePurpose::Project)
            .unwrap()
    });
    assert_eq!(chain.parameters(0).unwrap()[0].1, 0.375);
    let (mut recovered, recover) = measure(|| chain.recover(std::slice::from_ref(&state)).unwrap());
    assert_eq!(recovered.parameters(0).unwrap()[0].1, 0.375);
    [restore, recover]
}

#[test]
#[ignore = "needs helper and native delay fixture"]
fn state_transfer_releases_large_scratch() {
    const PAYLOAD: usize = 8 << 20;
    for measurement in transfer(PAYLOAD) {
        assert!(
            measurement.heap.bytes < 1 << 20,
            "retained {}",
            measurement.heap.bytes
        );
        // One encoded copy of the state; a second copy of the payload would exceed this.
        assert!(
            measurement.heap.peak < (PAYLOAD + PAYLOAD / 2) as isize,
            "peak {}",
            measurement.heap.peak
        );
    }
}

#[test]
#[ignore = "state transfer benchmark; run in release mode with --nocapture"]
fn state_transfer_benchmark() {
    for (operation, measurement) in ["restore", "recover"].into_iter().zip(transfer(64 << 20)) {
        writeln!(
            std::io::stdout(),
            "operation={operation} payload_mib=64 peak_bytes={} retained_bytes={} ms={:.3}",
            measurement.heap.peak,
            measurement.heap.bytes,
            measurement.elapsed.as_secs_f64() * 1000.0
        )
        .unwrap();
    }
}
