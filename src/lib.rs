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
//! // Create a triple buffer:
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
pub struct TripleBuffer<T: Send> {
    /// Input object used by producers to send updates
    input: Input<T>,

    /// Output object used by consumers to read the current value
    output: Output<T>,
}
//
impl<T: Clone + Send> TripleBuffer<T> {
    /// Construct a triple buffer with a certain initial value
    pub fn new(initial: T) -> Self {
        Self::new_impl(|| initial.clone())
    }
}
//
impl<T: Default + Send> Default for TripleBuffer<T> {
    /// Construct a triple buffer with a default-constructed value
    fn default() -> Self {
        Self::new_impl(|| T::default())
    }
}
//
impl<T: Send> TripleBuffer<T> {
    /// Construct a triple buffer, using a functor to generate initial values
    fn new_impl<F: FnMut() -> T>(mut generator: F) -> Self {
        // Start with the shared state...
        let shared_state = Arc::new(SharedState {
                                        buffers:
                                            [UnsafeCell::new(generator()),
                                             UnsafeCell::new(generator()),
                                             UnsafeCell::new(generator())],
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
impl<T: PartialEq + Send> PartialEq for TripleBuffer<T> {
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
pub struct Input<T: Send> {
    /// Reference-counted shared state
    shared: Arc<SharedState<T>>,

    /// Index of the write buffer (which is private to the producer)
    write_idx: BufferIndex,
}
//
// Public interface
impl<T: Send> Input<T> {
    /// Write a new value into the triple buffer, tell whether an unread value
    /// was overwritten because the consumer did not read it quickly enough.
    pub fn write(&mut self, value: T) -> bool {
        // Update the write buffer
        *self.write_buffer() = value;

        // Publish our updates and tell whether an overwrite occured
        self.publish()
    }

    /// Check if the consumer has read our last submission yet
    ///
    /// This method is only intended for diagnostics purposes. Please do not let
    /// it inform your decision of sending or not sending a value, as that would
    /// effectively be building a very poor spinlock-based double buffer
    /// implementation. If what you truly need is a double buffer, build
    /// yourself a proper blocking one instead of wasting CPU time.
    ///
    pub fn consumed(&self) -> bool {
        let back_info = self.shared.back_info.load(Ordering::Relaxed);
        back_info & BACK_DIRTY_BIT == 0
    }

    /// Get raw access to the write buffer
    ///
    /// This advanced interface allows you to update the write buffer in place,
    /// which can in some case improve performance by avoiding to create values
    /// of type T repeatedy when this is an expensive process.
    ///
    /// However, by opting into it, you force yourself to take into account
    /// subtle implementation details which you could normally ignore.
    ///
    /// First, the buffer does not contain the last value that you sent (which
    /// is now into the hands of the reader). In fact, the consumer is allowed
    /// to write complete garbage into it if it feels so inclined. All you can
    /// safely assume is that it contains a valid value of type T.
    ///
    /// Second, we do not send updates automatically. You need to call
    /// raw_publish() in order to propagate a buffer update to the consumer.
    /// Alternative designs based on Drop were considered, but ultimately deemed
    /// too magical for the target audience of this method.
    ///
    #[cfg(raw)]
    pub fn raw_write_buffer(&mut self) -> &mut T {
        self.write_buffer()
    }

    /// Unconditionally publish an update, checking for overwrites
    ///
    /// After updating the write buffer using raw_write_buffer(), you can use
    /// this method to publish your updates to the consumer. Like write(), this
    /// method will tell you whether an unread value was overwritten.
    ///
    #[cfg(raw)]
    pub fn raw_publish(&mut self) -> bool {
        self.publish()
    }
}
//
// Internal interface
impl<T: Send> Input<T> {
    /// Access the write buffer
    ///
    /// This is safe because the synchronization protocol ensures that we have
    /// exclusive access to this buffer.
    ///
    fn write_buffer(&mut self) -> &mut T {
        let write_ptr = self.shared.buffers[self.write_idx].get();
        unsafe { &mut *write_ptr }
    }

    /// Tell which memory ordering should be used for buffer swaps
    ///
    /// The right answer depends on whether the consumer is allowed to write
    /// into its read buffer or not. If it can, then we must synchronize with
    /// its writes. If not, we only need to propagate our own writes.
    ///
    fn swap_ordering() -> Ordering {
        if cfg!(raw) {
            Ordering::AcqRel
        } else {
            Ordering::Release
        }
    }

    /// Publish an update, checking for overwrites (internal version)
    fn publish(&mut self) -> bool {
        // Swap the write buffer and the back buffer, setting the dirty bit
        let former_back_info = self.shared.back_info.swap(
            self.write_idx | BACK_DIRTY_BIT,
            Self::swap_ordering()  // Propagate buffer updates as well
        );

        // The old back buffer becomes our new write buffer
        self.write_idx = former_back_info & BACK_INDEX_MASK;

        // Tell whether we have overwritten unread data
        former_back_info & BACK_DIRTY_BIT != 0
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
pub struct Output<T: Send> {
    /// Reference-counted shared state
    shared: Arc<SharedState<T>>,

    /// Index of the read buffer (which is private to the consumer)
    read_idx: BufferIndex,
}
//
// Public interface
impl<T: Send> Output<T> {
    /// Access the latest value from the triple buffer
    pub fn read(&mut self) -> &T {
        // Fetch updates from the producer
        self.update();

        // Give access to the read buffer
        self.read_buffer()
    }

    /// Tell whether a read buffer update is incoming from the producer
    ///
    /// This method is only intended for diagnostics purposes. Please do not let
    /// it inform your decision of reading a value or not, as that would
    /// effectively be building a very poor spinlock-based double buffer
    /// implementation. If what you truly need is a double buffer, build
    /// yourself a proper blocking one instead of wasting CPU time.
    ///
    pub fn updated(&self) -> bool {
        let back_info = self.shared.back_info.load(Ordering::Relaxed);
        back_info & BACK_DIRTY_BIT != 0
    }

    /// Get raw access to the read buffer
    ///
    /// This advanced interface allows you to modify the contents of the
    /// read buffer, which can in some case improve performance by avoiding to
    /// create values of type T when this is an expensive process. One possible
    /// application, for example, is to post-process values from the producer.
    ///
    /// However, by opting into it, you force yourself to take into account
    /// subtle implementation details which you could normally ignore.
    ///
    /// First, keep in mind that you can lose access to the current read buffer
    /// any time read() or raw_update() is called, as it can be replaced by an
    /// updated buffer from the producer automatically.
    ///
    /// Second, to reduce the potential for the aforementioned usage error, this
    /// method does not update the read buffer automatically. You need to call
    /// raw_update() in order to fetch buffer updates from the producer.
    ///
    #[cfg(raw)]
    pub fn raw_read_buffer(&mut self) -> &mut T {
        self.read_buffer()
    }

    /// Update the read buffer
    ///
    /// Check for incoming updates from the producer, and if so, update our read
    /// buffer to the latest data version. This operation will overwrite any
    /// changes which you have commited into the read buffer.
    ///
    /// Return a flag telling whether an update was carried out
    ///
    #[cfg(raw)]
    pub fn raw_update(&mut self) -> bool {
        self.update()
    }
}
//
// Internal interface
impl<T: Send> Output<T> {
    /// Access the read buffer (internal version)
    ///
    /// This is safe because the synchronization protocol ensures that we have
    /// exclusive access to this buffer.
    ///
    fn read_buffer(&mut self) -> &mut T {
        let read_ptr = self.shared.buffers[self.read_idx].get();
        unsafe { &mut *read_ptr }
    }

    /// Tell which memory ordering should be used for buffer swaps
    ///
    /// The right answer depends on whether the client is allowed to write into
    /// the read buffer or not. If it can, then we must propagate these writes
    /// to the producer. Otherwise, we only need to fetch producer updates.
    ///
    fn swap_ordering() -> Ordering {
        if cfg!(raw) {
            Ordering::AcqRel
        } else {
            Ordering::Acquire
        }
    }

    /// Check out incoming read buffer updates (internal version)
    fn update(&mut self) -> bool {
        // Access the shared state
        let ref shared_state = *self.shared;

        // Check if an update is present in the back-buffer
        let initial_back_info = shared_state.back_info.load(Ordering::Relaxed);
        let updated = initial_back_info & BACK_DIRTY_BIT != 0;
        if updated {
            // If so, exchange our read buffer with the back-buffer, thusly
            // acquiring exclusive access to the old back buffer while giving
            // the producer a new back-buffer to write to.
            let former_back_info = shared_state.back_info.swap(
                self.read_idx,
                Self::swap_ordering()  // Synchronize with buffer updates
            );

            // Make the old back-buffer our new read buffer
            self.read_idx = former_back_info & BACK_INDEX_MASK;
        }

        // Tell whether an update was carried out
        updated
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
struct SharedState<T: Send> {
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
impl<T: PartialEq + Send> SharedState<T> {
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
unsafe impl<T: Send> Sync for SharedState<T> {}


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
