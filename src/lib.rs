//! A triple buffering implementation
//!
//! In this crate, we attempt to implement a triple buffering mechanism,
//! suitable for emulating shared memory cells in a thread-safe and
//! non-blocking fashion between one single writer and one single reader.

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
pub struct TripleBuffer<T: Clone + PartialEq> {
    input: TripleBufferInput<T>,
    output: TripleBufferOutput<T>,
}
//
impl<T: Clone + PartialEq> TripleBuffer<T> {
    /// Construct a triple buffer with a certain initial value
    pub fn new(initial: T) -> Self {
        // Start with the shared state...
        let shared_state = Arc::new(
            TripleBufferSharedState {
                buffers: [
                    UnsafeCell::new(initial.clone()),
                    UnsafeCell::new(initial.clone()),
                    UnsafeCell::new(initial)
                ],
                back_idx: AtomicTripleBufferIndex::new(0),
                last_idx: AtomicTripleBufferIndex::new(2),
            }
        );

        // ...then construct the input and output structs
        TripleBuffer {
            input: TripleBufferInput {
                shared: shared_state.clone(),
                write_idx: 1,
            },
            output: TripleBufferOutput {
                shared: shared_state,
                read_idx: 2,
            },
        }
    }

    /// Extract input and output of the triple buffer
    pub fn split(self) -> (TripleBufferInput<T>, TripleBufferOutput<T>) {
        (self.input, self.output)
    }
}
//
// The Clone and PartialEq traits are used internally for testing.
//
impl<T: Clone + PartialEq> Clone for TripleBuffer<T> {
    fn clone(&self) -> Self {
        // Clone the shared state. This is safe because at this layer of the
        // interface, one needs an Input/Output &mut to mutate the shared state.
        let shared_state = Arc::new(
            unsafe { (*self.input.shared).clone() }
        );

        // ...then the input and output structs
        TripleBuffer {
            input: TripleBufferInput {
                shared: shared_state.clone(),
                write_idx: self.input.write_idx,
            },
            output: TripleBufferOutput {
                shared: shared_state,
                read_idx: self.output.read_idx,
            },
        }
    }
}
//
impl<T: Clone + PartialEq> PartialEq for TripleBuffer<T> {
    fn eq(&self, other: &Self) -> bool {
        // Compare the shared states. This is safe because at this layer of the
        // interface, one needs an Input/Output &mut to mutate the shared state.
        let shared_states_equal = unsafe {
            (*self.input.shared).eq(&*other.input.shared)
        };

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
pub struct TripleBufferInput<T: Clone + PartialEq> {
    shared: Arc<TripleBufferSharedState<T>>,
    write_idx: TripleBufferIndex,
}
//
impl<T: Clone + PartialEq> TripleBufferInput<T> {
    /// Write a new value into the triple buffer
    pub fn write(&mut self, value: T) {
        // Access the shared state
        let ref shared_state = *self.shared;

        // Determine which shared buffer is the write buffer
        let active_idx = self.write_idx;

        // Move the input value into the (exclusive-access) write buffer.
        let write_ptr = shared_state.buffers[active_idx].get();
        unsafe { *write_ptr = value; }

        // Swap the back buffer and the write buffer. This makes the new data
        // block available to the reader and gives us a new buffer to write to.
        self.write_idx = shared_state.back_idx.swap(
            active_idx,
            Ordering::Release  // Need this for new buffer state to be visible
        );

        // Notify the reader that we submitted an update
        shared_state.last_idx.store(
            active_idx,
            Ordering::Release  // Need this for back_idx to be visible
        );
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
pub struct TripleBufferOutput<T: Clone + PartialEq> {
    shared: Arc<TripleBufferSharedState<T>>,
    read_idx: TripleBufferIndex,
}
//
impl<T: Clone + PartialEq> TripleBufferOutput<T> {
    /// Access the latest value from the triple buffer
    pub fn read(&mut self) -> &T {
        // Access the shared state
        let ref shared_state = *self.shared;

        // Check if the writer has submitted an update
        let last_idx = shared_state.last_idx.load(Ordering::Acquire);

        // If an update is pending in the back buffer, make it our read buffer
        if self.read_idx != last_idx {
            // Swap the back buffer and the read buffer. We get exclusive read
            // access to the data, and the writer gets a new back buffer.
            self.read_idx = shared_state.back_idx.swap(
                self.read_idx,
                Ordering::Acquire  // Another update could have occured!
            );
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
/// - One index pointing to the "back" buffer, which no one is currently using
/// - One index pointing to the most recently updated buffer
///
#[derive(Debug)]
struct TripleBufferSharedState<T: Clone + PartialEq> {
    /// Data storage buffers
    buffers: [UnsafeCell<T>; 3],

    /// Index of the back-buffer (which no one is currently accessing)
    back_idx: AtomicTripleBufferIndex,

    /// Index of the most recently updated buffer
    last_idx: AtomicTripleBufferIndex,
}
//
impl<T: Clone + PartialEq> TripleBufferSharedState<T> {
    /// Cloning the shared state is unsafe because you must ensure that no one
    /// is concurrently accessing it, since &self is enough for writing.
    unsafe fn clone(&self) -> Self {
        // The use of UnsafeCell makes buffers somewhat cumbersome to clone...
        let clone_buffer = | i: TripleBufferIndex | -> UnsafeCell<T> {
            UnsafeCell::new(
                (*self.buffers[i].get()).clone()
            )
        };

        // ...and atomics aren't much better...
        let clone_atomic = | ai: &AtomicTripleBufferIndex | {
            AtomicTripleBufferIndex::new(ai.load(Ordering::Relaxed))
        };

        // ...so better define some shortcuts before getting started:
        TripleBufferSharedState {
            buffers: [
                clone_buffer(0),
                clone_buffer(1),
                clone_buffer(2),
            ],
            back_idx: clone_atomic(&self.back_idx),
            last_idx: clone_atomic(&self.last_idx),
        }
    }

    /// Equality is unsafe for the same reason as cloning: you must ensure that
    /// no one is concurrently accessing the triple buffer to avoid data races.
    unsafe fn eq(&self, other: &Self) -> bool {
        // The use of UnsafeCell makes buffers somewhat cumbersome to compare...
        let buffer_eq = | i: TripleBufferIndex | -> bool {
            *self.buffers[i].get() == *other.buffers[i].get()
        };

        // ...and atomics aren't much better...
        let atomics_eq = | x: &AtomicTripleBufferIndex,
                           y: &AtomicTripleBufferIndex | -> bool {
            x.load(Ordering::Relaxed) == y.load(Ordering::Relaxed)
        };

        // ...so better define some shortcuts before getting started:
        buffer_eq(0) && buffer_eq(1) && buffer_eq(2) &&
        atomics_eq(&self.back_idx, &other.back_idx) &&
        atomics_eq(&self.last_idx, &other.last_idx)
    }
}
//
unsafe impl<T: Clone + PartialEq> Sync for TripleBufferSharedState<T> {}


/// Index types used for triple buffering
///
/// These types are used to index into triple buffers
///
/// TODO: Switch to i8 / AtomicI8 once the later is stable
///
type TripleBufferIndex = usize;
type AtomicTripleBufferIndex = AtomicUsize;


/// Tests and benchmarks
///
/// Unit tests are provided to ease library evolution.
///
#[cfg(test)]
mod tests {
    use std::sync::Barrier;
    use std::thread;
    use std::time::Duration;

    /// Check that triple buffers are properly initialized
    #[test]
    fn initial_state() {
        // Let's create a triple buffer
        let buf = ::TripleBuffer::new(42);

        // Access the shared state
        let ref buf_shared = *buf.input.shared;
        let back_idx = buf_shared.back_idx.load(::Ordering::Relaxed);
        let last_idx = buf_shared.last_idx.load(::Ordering::Relaxed);

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
        assert!(unsafe { *read_ptr } == 42);

        // Last-written index must initially point to the read buffer
        assert_eq!(last_idx, buf.output.read_idx);
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
            let write_buf_ptr = expected_shared.buffers[old_write_idx].get();
            unsafe { *write_buf_ptr = true; }

            // We expect the former write buffer to be the new back buffer
            expected_shared.back_idx.store(
                old_write_idx,
                ::Ordering::Relaxed
            );

            // We expect the last updated index to point to former write buffer
            expected_shared.last_idx.store(
                old_write_idx,
                ::Ordering::Relaxed
            );

            // We expect the old back buffer to become the new write buffer
            let old_back_idx = old_shared.back_idx.load(::Ordering::Relaxed);
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
            expected_shared.back_idx.store(
                old_buf.output.read_idx,
                ::Ordering::Relaxed
            );

            // We expect the new read index to point to the former back buffer
            let old_back_idx = old_shared.back_idx.load(::Ordering::Relaxed);
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

    /// Check that concurrent reads and writes work
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
    fn concurrent_access() {
        // We will stress the infrastructure by performing this many writes
        // as a reader continuously reads the latest value
        let test_write_count = 10000;

        // This is the buffer that our reader and writer will share
        let buf = ::TripleBuffer::new(0u64);

        // Extract the input stage so that we can send it to the writer
        let (mut buf_input, mut buf_output) = buf.split();

        // Setup a barrier so that the reader & writer can start synchronously
        let barrier = ::Arc::new(Barrier::new(2));
        let w_barrier = barrier.clone();

        // The writer continuously increments the buffered value, with some
        // rate limiting to ensure that the reader can see the updates
        let writer = thread::spawn(move || {
            w_barrier.wait();
            for value in 1 .. test_write_count+1 {
                buf_input.write(value);
                thread::yield_now();
                thread::sleep(Duration::from_millis(1));
            }
        });

        // The reader continuously checks the buffered value, and should see
        // every update without any incoherent value
        let mut last_value = 0u64;
        barrier.wait();
        while last_value < test_write_count {
            let new_value = *buf_output.read();
            assert!(
                (new_value >= last_value) && (new_value-last_value <= 1)
            );
            last_value = new_value;
        }

        // Wait for the writer to finish
        writer.join().unwrap();
    }

    /// Range check for triple buffer indexes
    #[allow(unused_comparisons)]
    fn index_in_range(idx: ::TripleBufferIndex) -> bool {
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
    use std::sync::Barrier;
    use std::sync::atomic::AtomicBool;
    use std::thread;
    use std::time::Instant;

    /// Benchmark for clean read performance
    #[test]
    #[ignore]
    fn clean_read() {
        // Create a buffer
        let mut buf = ::TripleBuffer::new(0u32);

        // Benchmark clean reads
        benchmark(
            4000000000u32,
            |iter| {
                let read = *buf.output.read();
                assert!(read < u32::max_value());
            }
        );
    }

    /// Benchmark for write performance
    #[test]
    #[ignore]
    fn write() {
        // Create a buffer
        let mut buf = ::TripleBuffer::new(0u32);

        // Benchmark writes
        benchmark(
            440000000u32,
            |iter| buf.input.write(iter)
        );
    }

    /// Benchmark for write + dirty read performance
    #[test]
    #[ignore]
    fn write_and_dirty_read() {
        // Create a buffer
        let mut buf = ::TripleBuffer::new(0u32);

        // Benchmark writes + dirty reads
        benchmark(
            220000000u32,
            |iter| {
                buf.input.write(iter);
                let read = *buf.output.read();
                assert!(read < u32::max_value());
            }
        );
    }

    /// Benchmark read performance under concurrent write pressure
    #[test]
    #[ignore]
    fn concurrent_read() {
        // Create a buffer
        let buf = ::TripleBuffer::new(0u32);

        // Extract the triple buffer's input and output
        let (mut buf_input, mut buf_output) = buf.split();

        // Set up a barrier so that we can wait for the writer to start
        let barrier = ::Arc::new(Barrier::new(2));
        let w_barrier = barrier.clone();

        // Set up a shared boolean flag so that we can stop the writer
        let run_flag = ::Arc::new(AtomicBool::new(true));
        let w_run_flag = run_flag.clone();

        // Set up a writer that continuously updates the shared value
        let writer = thread::spawn(move || {
            let counter = 1;
            w_barrier.wait();
            while w_run_flag.load(::Ordering::Relaxed) {
                buf_input.write(counter);
                counter.wrapping_add(1);
            }
        });

        // Wait for the writer to be running
        barrier.wait();

        // Benchmark reads
        benchmark(
            40000000u32,
            |iter| {
                let read = *buf_output.read();
                assert!(read < u32::max_value());
            }
        );

        // Tell the writer to stop
        run_flag.store(false, ::Ordering::Relaxed);
        writer.join().unwrap();
    }

    /// Benchmark write performance under concurrent read pressure
    #[test]
    #[ignore]
    fn concurrent_write() {
        // Create a buffer
        let buf = ::TripleBuffer::new(0u32);

        // Extract the triple buffer's input and output
        let (mut buf_input, mut buf_output) = buf.split();

        // Set up a barrier so that we can wait for the reader to start
        let barrier = ::Arc::new(Barrier::new(2));
        let r_barrier = barrier.clone();

        // Set up a shared boolean flag so that we can stop the reader
        let run_flag = ::Arc::new(AtomicBool::new(true));
        let r_run_flag = run_flag.clone();

        // Set up a reader that continuously accesses the shared value
        let reader = thread::spawn(move || {
            r_barrier.wait();
            while r_run_flag.load(::Ordering::Relaxed) {
                let read = *buf_output.read();
                assert!(read < u32::max_value());
            }
        });

        // Wait for the reader to be running
        barrier.wait();

        // Benchmark writes
        benchmark(
            60000000u32,
            |iter| {
                buf_input.write(iter);
            }
        );

        // Tell the reader to stop
        run_flag.store(false, ::Ordering::Relaxed);
        reader.join().unwrap();
    }

    /// Simple benchmark harness while I'm waiting for #[bench] to stabilize
    fn benchmark<F: FnMut(u32)>(num_iterations: u32,
                                mut iteration: F) {
        // Run benchmark loop
        let start_time = Instant::now();
        for iter in 1 .. num_iterations {
            iteration(iter)
        }
        let total_duration = start_time.elapsed();

        // Put results in readable units
        let total_ms = (total_duration.as_secs() as u32) * 1000
                     + total_duration.subsec_nanos() / 1000000;
        let iter_ns = (total_duration / num_iterations).subsec_nanos();

        // Display the results
        print!(
            "{} ms ({} iters, <{} ns/iter): ",
            total_ms,
            num_iterations,
            iter_ns+1
        );
    }
}
