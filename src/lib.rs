//! Triple buffering in Rust
//!
//! In this crate, we propose a Rust implementation of triple buffering. This is
//! a non-blocking thread synchronization mechanism that can be used when a
//! single producer thread is frequently updating a shared data block, and a
//! single consumer thread wants to be able to read the latest available version
//! of the shared data whenever it feels like it.
//!
//! # Examples
//!
//! ```
//! // Create a triple buffer of any Clone type:
//! use triple_buffer::TripleBuffer;
//! let buf = TripleBuffer::new(0);
//!
//! // Split it into an input and output interface, to be respectively sent to
//! // the producer thread and the consumer thread:
//! let (mut buf_input, mut buf_output) = buf.split();
//!
//! // The producer can move a value into the buffer at any time
//! buf_input.write(42);
//!
//! // The consumer can access the latest value from the producer at any time
//! let latest_value_ref = buf_output.read();
//! assert_eq!(*latest_value_ref, 42);
//! ```

#![deny(missing_docs)]

extern crate testbench;

use std::cell::UnsafeCell;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;


/// A triple buffer, useful for nonblocking and thread-safe data sharing
///
/// A triple buffer is a single-producer single-consumer nonblocking
/// communication channel which behaves like a shared variable: writer submits
/// regular updates, reader accesses latest available value at any time.
///
/// The input and output fields of this struct are what producers and consumers
/// actually use in practice. They can safely be moved away from the
/// TripleBuffer struct after construction, and are further documented below.
///
#[derive(Debug)]
pub struct TripleBuffer<T: Clone + Send> {
    /// Input object used by producers to send updates
    input: Input<T>,

    /// Output object used by consumers to read the current value
    output: Output<T>,
}
//
impl<T: Clone + Send> TripleBuffer<T> {
    /// Construct a triple buffer with a certain initial value
    pub fn new(initial: T) -> Self {
        // Start with the shared state...
        let shared_state = Arc::new(SharedState {
                                        buffers:
                                            [UnsafeCell::new(initial.clone()),
                                             UnsafeCell::new(initial.clone()),
                                             UnsafeCell::new(initial)],
                                        back_info: AtomicBackBufferInfo::new(0),
                                    });

        // ...then construct the input and output structs
        TripleBuffer {
            input: Input {
                shared: shared_state.clone(),
                write_idx: 1,
            },
            output: Output {
                shared: shared_state,
                read_idx: 2,
            },
        }
    }

    /// Extract input and output of the triple buffer
    pub fn split(self) -> (Input<T>, Output<T>) {
        (self.input, self.output)
    }
}
//
// The Clone and PartialEq traits are used internally for testing.
//
impl<T: Clone + Send> Clone for TripleBuffer<T> {
    fn clone(&self) -> Self {
        // Clone the shared state. This is safe because at this layer of the
        // interface, one needs an Input/Output &mut to mutate the shared state.
        let shared_state = Arc::new(unsafe { (*self.input.shared).clone() });

        // ...then the input and output structs
        TripleBuffer {
            input: Input {
                shared: shared_state.clone(),
                write_idx: self.input.write_idx,
            },
            output: Output {
                shared: shared_state,
                read_idx: self.output.read_idx,
            },
        }
    }
}
//
impl<T: Clone + PartialEq + Send> PartialEq for TripleBuffer<T> {
    fn eq(&self, other: &Self) -> bool {
        // Compare the shared states. This is safe because at this layer of the
        // interface, one needs an Input/Output &mut to mutate the shared state.
        let shared_states_equal =
            unsafe { (*self.input.shared).eq(&*other.input.shared) };

        // Compare the rest of the triple buffer states
        shared_states_equal &&
        (self.input.write_idx == other.input.write_idx) &&
        (self.output.read_idx == other.output.read_idx)
    }
}


/// Producer interface to the triple buffer
///
/// The producer of data can use this struct to submit updates to the triple
/// buffer whenever he likes. These updates are nonblocking: a collision between
/// the producer and the consumer will result in cache contention, but deadlocks
/// and scheduling-induced slowdowns cannot happen.
///
#[derive(Debug)]
pub struct Input<T: Clone + Send> {
    /// Reference-counted shared state
    shared: Arc<SharedState<T>>,

    /// Index of the write buffer (which is private to the producer)
    write_idx: BufferIndex,
}
//
impl<T: Clone + Send> Input<T> {
    /// Write a new value into the triple buffer
    pub fn write(&mut self, value: T) {
        // Access the shared state
        let ref shared_state = *self.shared;

        // Determine which shared buffer is the write buffer
        let active_idx = self.write_idx;

        // Move the input value into the (exclusive-access) write buffer.
        let write_ptr = shared_state.buffers[active_idx].get();
        unsafe {
            *write_ptr = value;
        }

        // Publish our write buffer as the new back-buffer, setting the dirty
        // bit to tell the consumer that new data is available in there.
        let former_back_info = shared_state.back_info.swap(
            active_idx | BACK_DIRTY_BIT,
            Ordering::Release  // Publish buffer updates to the reader
        );

        // The previous back-buffer, which is not reader-visible anymore, will
        // become our new write buffer. Extract its index from the old info.
        self.write_idx = former_back_info & BACK_INDEX_MASK
    }
}


/// Consumer interface to the triple buffer
///
/// The consumer of data can use this struct to access the latest published
/// update from the producer whenever he likes. Readout is nonblocking: a
/// collision between the producer and consumer will result in cache contention,
/// but deadlocks and scheduling-induced slowdowns cannot happen.
///
#[derive(Debug)]
pub struct Output<T: Clone + Send> {
    /// Reference-counted shared state
    shared: Arc<SharedState<T>>,

    /// Index of the read buffer (which is private to the consumer)
    read_idx: BufferIndex,
}
//
impl<T: Clone + Send> Output<T> {
    /// Access the latest value from the triple buffer
    pub fn read(&mut self) -> &T {
        // Access the shared state
        let ref shared_state = *self.shared;

        // Check if an update is present in the back-buffer
        let initial_back_info = shared_state.back_info.load(Ordering::Relaxed);
        if initial_back_info & BACK_DIRTY_BIT != 0 {
            // If so, exchange our read buffer with the back-buffer, thusly
            // acquiring exclusive access to the old back buffer while giving
            // the producer a new back-buffer to write to.
            let former_back_info = shared_state.back_info.swap(
                self.read_idx,
                Ordering::Acquire  // Synchronize with buffer updates
            );

            // Extract the old back-buffer index as our new read buffer index
            self.read_idx = former_back_info & BACK_INDEX_MASK;
        }

        // Access data from the current (exclusive-access) read buffer
        let read_ptr = shared_state.buffers[self.read_idx].get();
        unsafe { &*read_ptr }
    }
}


/// Triple buffer shared state
///
/// In a triple buffering communication protocol, the reader and writer share
/// the following storage:
///
/// - Three memory buffers suitable for storing the data at hand
/// - Information about the back-buffer: which buffer is the current back-buffer
///   and whether an update was published since the last readout.
///
#[derive(Debug)]
struct SharedState<T: Clone + Send> {
    /// Data storage buffers
    buffers: [UnsafeCell<T>; 3],

    /// Information about the current back-buffer state
    back_info: AtomicBackBufferInfo,
}
//
impl<T: Clone + Send> SharedState<T> {
    /// Cloning the shared state is unsafe because you must ensure that no one
    /// is concurrently accessing it, since &self is enough for writing.
    unsafe fn clone(&self) -> Self {
        // The use of UnsafeCell makes buffers somewhat cumbersome to clone...
        let clone_buffer = |i: BufferIndex| -> UnsafeCell<T> {
            UnsafeCell::new((*self.buffers[i].get()).clone())
        };

        // ...so better define some shortcuts before getting started:
        SharedState {
            buffers: [clone_buffer(0), clone_buffer(1), clone_buffer(2)],
            back_info:
                AtomicBackBufferInfo::new(self.back_info
                                              .load(Ordering::Relaxed)),
        }
    }
}
//
impl<T: Clone + PartialEq + Send> SharedState<T> {
    /// Equality is unsafe for the same reason as cloning: you must ensure that
    /// no one is concurrently accessing the triple buffer to avoid data races.
    unsafe fn eq(&self, other: &Self) -> bool {
        // Check whether the contents of all buffers are equal...
        let buffers_equal = self.buffers
            .iter()
            .zip(other.buffers.iter())
            .all(|tuple| -> bool {
                     let (cell1, cell2) = tuple;
                     *cell1.get() == *cell2.get()
                 });

        // ...then check whether the rest of the shared state is equal
        buffers_equal &&
        (self.back_info.load(Ordering::Relaxed) ==
         other.back_info.load(Ordering::Relaxed))
    }
}
//
unsafe impl<T: Clone + Send> Sync for SharedState<T> {}


/// Index types used for triple buffering
///
/// These types are used to index into triple buffers. In addition, the
/// BackBufferInfo type is actually a bitfield, whose third bit (numerical
/// value: 4) is set to 1 to indicate that the producer published an update into
/// the back-buffer, and reset to 0 when the reader fetches the update.
///
/// TODO: Switch to i8 / AtomicI8 once the later is stable
///
type BufferIndex = usize;
//
type AtomicBackBufferInfo = AtomicUsize;
const BACK_INDEX_MASK: usize = 0b11; // Mask used to extract back-buffer index
const BACK_DIRTY_BIT: usize = 0b100; // Bit set by producer to signal updates


/// Unit tests
#[cfg(test)]
mod tests {
    use std::sync::atomic::Ordering;
    use std::thread;
    use std::time::Duration;
    use testbench;
    use testbench::race_cell::{Racey, UsizeRaceCell};

    /// Check that triple buffers are properly initialized
    #[test]
    fn initial_state() {
        // Let's create a triple buffer
        let buf = ::TripleBuffer::new(42);

        // Access the shared state and decode back-buffer information
        let ref buf_shared = *buf.input.shared;
        let back_info = buf_shared.back_info.load(Ordering::Relaxed);
        let back_idx = back_info & ::BACK_INDEX_MASK;
        let back_buffer_clean = back_info & ::BACK_DIRTY_BIT == 0;

        // Write-/read-/back-buffer indexes must be in range
        assert!(index_in_range(buf.input.write_idx));
        assert!(index_in_range(buf.output.read_idx));
        assert!(index_in_range(back_idx));

        // Write-/read-/back-buffer indexes must be distinct
        assert!(buf.input.write_idx != buf.output.read_idx);
        assert!(buf.input.write_idx != back_idx);
        assert!(buf.output.read_idx != back_idx);

        // Read buffer must be properly initialized
        let read_ptr = buf_shared.buffers[buf.output.read_idx].get();
        assert_eq!(unsafe { *read_ptr }, 42);

        // Back-buffer must be initially clean
        assert!(back_buffer_clean);
    }

    /// Check that (sequentially) writing to a triple buffer works
    #[test]
    fn sequential_write() {
        // Let's create a triple buffer
        let mut buf = ::TripleBuffer::new(false);

        // Back up the initial buffer state
        let old_buf = buf.clone();
        let ref old_shared = old_buf.input.shared;

        // Perform a write
        buf.input.write(true);

        // Check new implementation state
        {
            // Starting from the old buffer state...
            let mut expected_buf = old_buf.clone();
            let ref expected_shared = expected_buf.input.shared;

            // We expect the former write buffer to have received the new value
            let old_write_idx = old_buf.input.write_idx;
            let write_ptr = expected_shared.buffers[old_write_idx].get();
            unsafe {
                *write_ptr = true;
            }

            // We expect the former write buffer to become the new back buffer
            // and the back buffer's dirty bit to be set
            expected_shared.back_info
                .store(old_write_idx | ::BACK_DIRTY_BIT,
                       Ordering::Relaxed);

            // We expect the old back buffer to become the new write buffer
            let old_back_info = old_shared.back_info.load(Ordering::Relaxed);
            let old_back_idx = old_back_info & ::BACK_INDEX_MASK;
            expected_buf.input.write_idx = old_back_idx;

            // Nothing else should have changed
            assert_eq!(buf, expected_buf);
        }
    }

    /// Check that (sequentially) reading from a triple buffer works
    #[test]
    fn sequential_read() {
        // Let's create a triple buffer and write into it
        let mut buf = ::TripleBuffer::new(1.0);
        buf.input.write(4.2);

        // Test readout from dirty (freshly written) triple buffer
        {
            // Back up the initial buffer state
            let old_buf = buf.clone();
            let ref old_shared = old_buf.input.shared;

            // Read from the buffer
            let result = *buf.output.read();

            // Output value should be correct
            assert_eq!(result, 4.2);

            // Starting from the old buffer state...
            let mut expected_buf = old_buf.clone();
            let ref expected_shared = expected_buf.input.shared;

            // We expect the new back index to point to the former read buffer
            // and the back buffer's dirty bit to be unset.
            expected_shared.back_info.store(old_buf.output.read_idx,
                                            Ordering::Relaxed);

            // We expect the new read index to point to the former back buffer
            let old_back_info = old_shared.back_info.load(Ordering::Relaxed);
            let old_back_idx = old_back_info & ::BACK_INDEX_MASK;
            expected_buf.output.read_idx = old_back_idx;

            // Nothing else should have changed
            assert_eq!(buf, expected_buf);
        }

        // Test readout from clean (unchanged) triple buffer
        {
            // Back up the initial buffer state
            let old_buf = buf.clone();

            // Read from the buffer
            let result = *buf.output.read();

            // Output value should be correct
            assert_eq!(result, 4.2);

            // Buffer state should be unchanged
            assert_eq!(buf, old_buf);
        }
    }

    /// Check that contended concurrent reads and writes work
    ///
    /// **WARNING:** This test unfortunately needs to have timing-dependent
    /// behaviour to do its job. If it fails for you, try the following:
    ///
    /// - Close running applications in the background
    /// - Re-run the tests with only one OS thread (--test-threads=1)
    /// - Increase the writer sleep period
    ///
    #[test]
    #[ignore]
    fn contended_concurrent_access() {
        // We will stress the infrastructure by performing this many writes
        // as a reader continuously reads the latest value
        const TEST_WRITE_COUNT: usize = 100_000_000;

        // This is the buffer that our reader and writer will share
        let buf = ::TripleBuffer::new(UsizeRaceCell::new(0));
        let (mut buf_input, mut buf_output) = buf.split();

        // Concurrently run a writer which increments a shared value in a loop,
        // and a reader which makes sure that no unexpected value slips in.
        let mut last_value = 0usize;
        testbench::concurrent_test_2(
            move || {
                for value in 1..(TEST_WRITE_COUNT + 1) {
                    buf_input.write(UsizeRaceCell::new(value));
                }
            },
            move || {
                while last_value < TEST_WRITE_COUNT {
                    let new_racey_value = buf_output.read().get();
                    match new_racey_value {
                        Racey::Consistent(new_value) => {
                            assert!((new_value >= last_value) &&
                                    (new_value <= TEST_WRITE_COUNT));
                            last_value = new_value;
                        }
                        Racey::Inconsistent => {
                            panic!("Inconsistent state exposed by the buffer!");
                        }
                    }
                }
            }
        );
    }

    /// Check that uncontended concurrent reads and writes work
    ///
    /// **WARNING:** Caveats of contented concurrent access also apply here
    ///
    #[test]
    #[ignore]
    fn uncontended_concurrent_access() {
        // We will stress the infrastructure by performing this many writes
        // as a reader continuously reads the latest value
        const TEST_WRITE_COUNT: usize = 1_250;

        // This is the buffer that our reader and writer will share
        let buf = ::TripleBuffer::new(UsizeRaceCell::new(0));
        let (mut buf_input, mut buf_output) = buf.split();

        // Concurrently run a writer which slowly increments a shared value,
        // and a reader which checks that it can receive every update
        let mut last_value = 0usize;
        testbench::concurrent_test_2(
            move || {
                for value in 1..(TEST_WRITE_COUNT + 1) {
                    buf_input.write(UsizeRaceCell::new(value));
                    thread::yield_now();
                    thread::sleep(Duration::from_millis(16));
                }
            },
            move || {
                while last_value < TEST_WRITE_COUNT {
                    let new_racey_value = buf_output.read().get();
                    match new_racey_value {
                        Racey::Consistent(new_value) => {
                            assert!((new_value >= last_value) &&
                                    (new_value - last_value <= 1));
                            last_value = new_value;
                        }
                        Racey::Inconsistent => {
                            panic!("Inconsistent state exposed by the buffer!");
                        }
                    }
                }
            }
        );
    }

    /// Range check for triple buffer indexes
    #[allow(unused_comparisons)]
    fn index_in_range(idx: ::BufferIndex) -> bool {
        (idx >= 0) & (idx <= 2)
    }
}


/// Performance benchmarks
///
/// These benchmarks masquerading as tests are a stopgap solution until
/// benchmarking lands in Stable Rust. They should be compiled in release mode,
/// and run with only one OS thread. In addition, the default behaviour of
/// swallowing test output should obviously be suppressed.
///
/// TL;DR: cargo test --release -- --ignored --nocapture --test-threads=1
///
/// TODO: Switch to standard Rust benchmarks once they are stable
///
#[cfg(test)]
mod benchmarks {
    use testbench;

    /// Benchmark for clean read performance
    #[test]
    #[ignore]
    fn clean_read() {
        // Create a buffer
        let mut buf = ::TripleBuffer::new(0u32);

        // Benchmark clean reads
        testbench::benchmark(4_000_000_000, || {
            let read = *buf.output.read();
            assert!(read < u32::max_value());
        });
    }

    /// Benchmark for write performance
    #[test]
    #[ignore]
    fn write() {
        // Create a buffer
        let mut buf = ::TripleBuffer::new(0u32);

        // Benchmark writes
        let mut iter = 1u32;
        testbench::benchmark(440_000_000, || {
            buf.input.write(iter);
            iter+= 1;
        });
    }

    /// Benchmark for write + dirty read performance
    #[test]
    #[ignore]
    fn write_and_dirty_read() {
        // Create a buffer
        let mut buf = ::TripleBuffer::new(0u32);

        // Benchmark writes + dirty reads
        let mut iter = 1u32;
        testbench::benchmark(220_000_000u32, || {
            buf.input.write(iter);
            iter+= 1;
            let read = *buf.output.read();
            assert!(read < u32::max_value());
        });
    }

    /// Benchmark read performance under concurrent write pressure
    #[test]
    #[ignore]
    fn concurrent_read() {
        // Create a buffer
        let buf = ::TripleBuffer::new(0u32);
        let (mut buf_input, mut buf_output) = buf.split();

        // Benchmark reads under concurrent write pressure
        let mut counter = 0u32;
        testbench::concurrent_benchmark(
            40_000_000u32,
            move || {
                let read = *buf_output.read();
                assert!(read < u32::max_value());
            },
            move || {
                buf_input.write(counter);
                counter = (counter + 1) % u32::max_value();
            }
        );
    }

    /// Benchmark write performance under concurrent read pressure
    #[test]
    #[ignore]
    fn concurrent_write() {
        // Create a buffer
        let buf = ::TripleBuffer::new(0u32);
        let (mut buf_input, mut buf_output) = buf.split();

        // Benchmark writes under concurrent read pressure
        let mut iter = 1u32;
        testbench::concurrent_benchmark(
            60_000_000u32,
            move || {
                buf_input.write(iter);
                iter += 1;
            },
            move || {
                let read = *buf_output.read();
                assert!(read < u32::max_value());
            }
        );
    }
}
