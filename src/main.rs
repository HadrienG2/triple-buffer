use std::sync::atomic::{self, AtomicUsize, Ordering};


/// Indexes used for triple buffer access
/// TODO: Switch to i8 / AtomicI8 once they are stable
type TripleBufferIndex = usize;
type AtomicTripleBufferIndex = AtomicUsize;


/// A triple buffer, suitable for continuously updating a shared data block
/// in a thread-safe and non-blocking fashion when there is only one producer
/// and one consumer of the data.
struct TripleBuffer<T: Clone> {
    /// Storage for triple-buffered data
    storage: [T; 3],

    /// Index of the write buffer (private to the writer)
    write_idx: TripleBufferIndex,

    /// Index of the back-buffer (shared between reader and writer)
    back_idx: AtomicTripleBufferIndex,

    /// Index of the read buffer (private to the reader)
    read_idx: TripleBufferIndex,

    /// Index of the most up-to-date buffer (written by writer, read by reader)
    last_idx: AtomicTripleBufferIndex,
}


// Implementation of the triple buffer
impl<T: Clone> TripleBuffer<T> {
    /// Create a new triple buffer with some initial data value
    fn new(initial: T) -> Self {
        TripleBuffer {
            storage: [initial.clone(), initial.clone(), initial.clone()],
            write_idx: 0,
            back_idx: AtomicTripleBufferIndex::new(1),
            read_idx: 2,
            last_idx: AtomicTripleBufferIndex::new(2),
        }
    }

    /// Write a new value into the triple buffer
    fn write(&mut self, value: T) {
        // Select a write buffer that we have exclusive access to
        let active_idx = self.write_idx;

        // Move the input value into the write buffer
        self.storage[active_idx] = value;

        // Swap the back buffer and the write buffer. This makes the new data
        // block available to the reader and gives us a new buffer to write to.
        self.write_idx = self.back_idx.swap(active_idx, Ordering::Relaxed);

        // Allow the reader to detect and synchronize with this update
        self.last_idx.store(active_idx, Ordering::Release);
    }

    /// Access the latest value from the triple buffer
    fn read(&mut self) -> &T {
        // Check which buffer was last written to
        let last_idx = self.last_idx.load(Ordering::Relaxed);

        // Update the read buffer if an update has occured since the last read
        if self.read_idx != last_idx {
            // Synchronize with the writer's updates
            atomic::fence(Ordering::Acquire);

            // Swap the back buffer and the read buffer. We get exclusive read
            // access to the data, and the writer gets a new back buffer.
            self.read_idx = self.back_idx.swap(self.read_idx, Ordering::Relaxed);
        }

        // Access data from the current read buffer
        &self.storage[self.read_idx]
    }
}


/// Unit tests for triple buffers
#[cfg(test)]
mod tests {
    /// Test that triple buffers are properly initialized
    #[test]
    fn test_init() {
        // Let's create a triple buffer
        let buf = ::TripleBuffer::new(42);

        // Cache atomic indices for convenience
        let back_idx = buf.back_idx.load(::Ordering::Relaxed);
        let last_idx = buf.last_idx.load(::Ordering::Relaxed);

        // Write, read and back-buffer indexes must be in range
        assert!(index_in_range(buf.read_idx));
        assert!(index_in_range(buf.write_idx));
        assert!(index_in_range(back_idx));

        // Write, read and back-buffer indexes must be distinct
        assert!(buf.read_idx != buf.write_idx);
        assert!(buf.read_idx != back_idx);
        assert!(buf.write_idx != back_idx);

        // Read buffer must be properly initialized
        assert!(buf.storage[buf.read_idx] == 42);

        // Last-written index must initially point to read buffer
        assert_eq!(last_idx, buf.read_idx);
    }
    
    /// Test that (sequentially) writing to a triple buffer works
    #[test]
    fn test_seq_write() {
        // Let's create a triple buffer
        let mut buf = ::TripleBuffer::new(false);

        // Back up initial state
        let init_storage = buf.storage.clone();
        let init_write_idx = buf.write_idx;
        let init_back_idx = buf.back_idx.load(::Ordering::Relaxed);
        let init_read_idx = buf.read_idx;

        // Perform a write
        buf.write(true);

        // Only the write buffer should have changed
        let mut expected_storage = init_storage.clone();
        expected_storage[init_write_idx] = true;
        assert_eq!(buf.storage, expected_storage);

        // Write index should point to the former back buffer
        assert_eq!(buf.write_idx, init_back_idx);

        // Back index should point to the former write buffer
        let new_back_idx = buf.back_idx.load(::Ordering::Relaxed);
        assert_eq!(new_back_idx, init_write_idx);

        // Read index should not change
        assert_eq!(buf.read_idx, init_read_idx);

        // Last index should point to the newly written buffer
        let new_last_idx = buf.last_idx.load(::Ordering::Relaxed);
        assert_eq!(new_last_idx, init_write_idx);
    }
    
    /// Check that (sequentially) reading from a triple buffer works
    #[test]
    fn test_seq_read() {
        // Let's create a triple buffer and write into it
        let mut buf = ::TripleBuffer::new(1.0);
        buf.write(4.2);

        // Test readout from dirty (freshly written) triple buffer
        {
            // Back up initial state
            let init_storage = buf.storage.clone();
            let init_write_idx = buf.write_idx;
            let init_read_idx = buf.read_idx;
            let init_last_idx = buf.last_idx.load(::Ordering::Relaxed);

            // Read from the buffer
            let result = *buf.read();

            // Output value should be correct
            assert_eq!(result, 4.2);

            // Storage should not change
            assert_eq!(buf.storage, init_storage);

            // Write index should not change
            assert_eq!(buf.write_idx, init_write_idx);

            // Back index should point to the former read buffer
            let new_back_idx = buf.back_idx.load(::Ordering::Relaxed);
            assert_eq!(new_back_idx, init_read_idx);

            // Read index should point to the last written index
            assert_eq!(buf.read_idx, init_last_idx);

            // Last index should not change
            let new_last_idx = buf.last_idx.load(::Ordering::Relaxed);
            assert_eq!(new_last_idx, init_last_idx);
        }

        // Test readout from clean (unchanged) triple buffer
        {
            // Back up initial state
            let init_storage = buf.storage.clone();
            let init_write_idx = buf.write_idx;
            let init_back_idx = buf.back_idx.load(::Ordering::Relaxed);
            let init_read_idx = buf.read_idx;
            let init_last_idx = buf.last_idx.load(::Ordering::Relaxed);

            // Read from the buffer
            let result = *buf.read();

            // Output value should be correct
            assert_eq!(result, 4.2);

            // Storage should not change
            assert_eq!(buf.storage, init_storage);

            // Write index should not change
            assert_eq!(buf.write_idx, init_write_idx);

            // Back index should not change
            let new_back_idx = buf.back_idx.load(::Ordering::Relaxed);
            assert_eq!(new_back_idx, init_back_idx);

            // Read index should not change
            assert_eq!(buf.read_idx, init_read_idx);

            // Last index should not change
            let new_last_idx = buf.last_idx.load(::Ordering::Relaxed);
            assert_eq!(new_last_idx, init_last_idx);
        }
    }

    // TODO: Check that concurrent reads and writes work

    /// Range check for triple buffer indexes
    #[allow(unused_comparisons)]
    fn index_in_range(idx: ::TripleBufferIndex) -> bool {
        (idx >= 0) & (idx <= 2)
    }
}


fn main() {
    println!("Hello, world!");

    let mut buf = TripleBuffer::new("Hello!");
    println!("Initial value is {}", buf.read());

    buf.write("World!");
    println!("New value is {}", buf.read())
}
