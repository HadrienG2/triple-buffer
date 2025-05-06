//! In this crate, we propose a Rust implementation of triple buffering. This is
//! a non-blocking thread synchronization mechanism that can be used when a
//! single producer thread is frequently updating a shared data block, and a
//! single consumer thread wants to be able to read the latest available version
//! of the shared data whenever it feels like it.
//!
//! # Examples
//!
//! For many use cases, you can use the ergonomic write/read interface, where
//! the producer moves values into the buffer and the consumer accesses the
//! latest buffer by shared reference:
//!
//! ```
//! // Create a triple buffer
//! use triple_buffer::triple_buffer;
//! let (mut buf_input, mut buf_output) = triple_buffer(&0);
//!
//! // The producer thread can move a value into the buffer at any time
//! let producer = std::thread::spawn(move || buf_input.write(42));
//!
//! // The consumer thread can read the latest value at any time
//! let consumer = std::thread::spawn(move || {
//!     let latest = buf_output.read();
//!     assert!(*latest == 42 || *latest == 0);
//! });
//!
//! # producer.join().unwrap();
//! # consumer.join().unwrap();
//! ```
//!
//! In situations where moving the original value away and being unable to
//! modify it on the consumer's side is too costly, such as if creating a new
//! value involves dynamic memory allocation, you can use a lower-level API
//! which allows you to access the producer and consumer's buffers in place
//! and to precisely control when updates are propagated:
//!
//! ```
//! // Create and split a triple buffer
//! use triple_buffer::triple_buffer;
//! let (mut buf_input, mut buf_output) = triple_buffer(&String::with_capacity(42));
//!
//! // --- PRODUCER SIDE ---
//!
//! // Mutate the input buffer in place
//! {
//!     // Acquire a reference to the input buffer
//!     let input = buf_input.input_buffer_mut();
//!
//!     // In general, you don't know what's inside of the buffer, so you should
//!     // always reset the value before use (this is a type-specific process).
//!     input.clear();
//!
//!     // Perform an in-place update
//!     input.push_str("Hello, ");
//! }
//!
//! // Publish the above input buffer update
//! buf_input.publish();
//!
//! // --- CONSUMER SIDE ---
//!
//! // Manually fetch the buffer update from the consumer interface
//! buf_output.update();
//!
//! // Acquire a read-only reference to the output buffer
//! let output = buf_output.output_buffer();
//! assert_eq!(*output, "Hello, ");
//!
//! // Or acquire a mutable reference if necessary
//! let output_mut = buf_output.output_buffer_mut();
//!
//! // Post-process the output value before use
//! output_mut.push_str("world!");
//! ```
//!
//! Finally, as a middle ground before the maximal ergonomics of the
//! [`write()`](Input::write) API and the maximal control of the
//! [`input_buffer_mut()`](Input::input_buffer_mut)/[`publish()`](Input::publish)
//! API, you can also use the
//! [`input_buffer_publisher()`](Input::input_buffer_publisher) RAII API on the
//! producer side, which ensures that `publish()` is automatically called when
//! the resulting input buffer handle goes out of scope:
//!
//! ```
//! // Create and split a triple buffer
//! use triple_buffer::triple_buffer;
//! let (mut buf_input, _) = triple_buffer(&String::with_capacity(42));
//!
//! // Mutate the input buffer in place and publish it
//! {
//!     // Acquire a reference to the input buffer
//!     let mut input = buf_input.input_buffer_publisher();
//!
//!     // In general, you don't know what's inside of the buffer, so you should
//!     // always reset the value before use (this is a type-specific process).
//!     input.clear();
//!
//!     // Perform an in-place update
//!     input.push_str("Hello world!");
//!
//!     // Input buffer is automatically published at the end of the scope of
//!     // the "input" RAII guard
//! }
//!
//! // From this point on, the consumer can see the updated version
//! ```

#![cfg_attr(not(test), no_std)]
#![deny(missing_debug_implementations, missing_docs)]

#[cfg(feature = "alloc")]
extern crate alloc;

use crossbeam_utils::CachePadded;

#[cfg(feature = "alloc")]
use alloc::sync::Arc;
use core::{
    cell::UnsafeCell,
    fmt::{self, Debug, Display, Formatter},
    ops::{Deref, DerefMut},
    sync::atomic::{AtomicU8, AtomicUsize, Ordering},
};

/// A triple buffer, useful for nonblocking and thread-safe data sharing
///
/// A triple buffer is a single-producer single-consumer nonblocking
/// communication channel which behaves like a shared variable: the producer
/// thread can submit regular updates, and the consumer thread can access the
/// latest available value whenever it feels like it.
#[derive(Debug)]
#[repr(C)]
pub struct TripleBuffer<T: Send> {
    /// Data storage buffers
    ///
    /// Should be the first member to allow for maximal `#[link_section]`
    /// embedded DMA shenanigans. This is enforces via `repr(C)`.
    buffers: [CachePadded<UnsafeCell<T>>; 3],

    /// Atomic state used to synchronize access to buffers
    status: CachePadded<Status>,
}
//
impl<T: Clone + Send> TripleBuffer<T> {
    /// Set up a triple buffer with a certain initial value
    pub fn new(initial: &T) -> Self {
        Self::from_fn(|| initial.clone())
    }
}
//
impl<T: Default + Send> Default for TripleBuffer<T> {
    /// Set up a triple buffer with a default-constructed initial value
    fn default() -> Self {
        Self::from_fn(T::default)
    }
}
//
impl<T: Send> TripleBuffer<T> {
    /// Set up a triple buffer, generating the three inner initial values
    /// using the `generator` function
    ///
    /// The values that are produced by the `generator` functions should be
    /// _logically identical_, in the sense that...
    ///
    /// - If `T` implements `PartialEq`, then these values should compare equal
    ///   to each other.
    /// - You should not need to know which of these values the triple buffer's
    ///   producer and consumer interfaces will end up initially pointing to.
    ///   For most purpose, you should consider them to be the same value.
    ///
    /// It is, however, acceptable for these values to be _bitwise distinct_,
    /// for example they can be pointers to three distinct allocations that have
    /// identical content.
    pub fn from_fn(mut generator: impl FnMut() -> T) -> Self {
        Self::from_values(core::array::from_fn(|_| generator()))
    }

    /// Wrap three user-provided values into a `TripleBuffer`
    ///
    /// As with [`from_fn()`](Self::from_fn), the three input values should be
    /// _logically identical_, but can be _bitwise distinct_.
    pub fn from_values(values: [T; 3]) -> Self {
        Self {
            buffers: values.map(|v| CachePadded::new(UnsafeCell::new(v))),
            status: CachePadded::new(Status::new()),
        }
    }

    /// Set up the [`Input`] and [`Output`] of a reference-counted triple buffer
    ///
    /// This method is a common-case convenience shortcut for the
    /// [`inout()`](TripleBufferRef::inout) method of the [`TripleBufferRef`]
    /// trait, which would otherwise require you to import said trait and use
    /// the awkward fully qualified function call syntax for clarity.
    ///
    /// # Panics
    ///
    /// Panics if this triple buffer already has an [`Input`] or [`Output`]
    #[cfg(feature = "alloc")]
    pub fn inout_from_arc(self: Arc<Self>) -> (Input<Arc<Self>>, Output<Arc<Self>>) {
        <Arc<Self> as TripleBufferRef>::inout(self)
    }

    /// Set up the [`Input`] and [`Output`] of a triple buffer from a shared
    /// reference
    ///
    /// This method is a common-case convenience shortcut for the
    /// [`inout()`](TripleBufferRef::inout) method of the [`TripleBufferRef`]
    /// trait, which would otherwise require you to import said trait and use
    /// the awkward fully qualified function call syntax for clarity.
    ///
    /// Beware that when the producer and consumer interfaces are set up from a
    /// shared reference like this, they cannot be used outside of the scope
    /// where the underlying triple buffer is defined. In naive usage, this will
    /// often be too limiting for inter-thread communication.
    ///
    /// However, this set up method works for `no_alloc` use cases where
    /// synchronization primitives must commonly be allocated as static
    /// variables due to lack of heap allocations. In this case, the `'static`
    /// lifetime of the associated references will let you use the resulting
    /// `Input`s and `Output`s anywhere in the program.
    ///
    /// Another situation where this method can be used is "scoped thread"
    /// abstractions like `std::thread::scope()`. To support these, `'a` is
    /// not constrained to be `'static` even though that will often be what you
    /// want when spawning threads in a more traditional way.
    ///
    /// # Panics
    ///
    /// Panics if this triple buffer already has an [`Input`] or [`Output`]
    pub fn inout_from_ref<'a>(&'a self) -> (Input<&'a Self>, Output<&'a Self>) {
        <&'a Self as TripleBufferRef>::inout(self)
    }
}
//
/// Convenience shortcut to set up a reference-counted triple buffer and get
/// the associated producer and consumer interfaces
#[cfg(feature = "alloc")]
pub fn triple_buffer<T: Clone + Send>(
    initial: &T,
) -> (Input<Arc<TripleBuffer<T>>>, Output<Arc<TripleBuffer<T>>>) {
    Arc::new(TripleBuffer::new(initial)).inout_from_arc()
}
//
impl<T: Send> Drop for TripleBuffer<T> {
    fn drop(&mut self) {
        let next_io_indices = self.status.next_io_indices.load(Ordering::Relaxed);
        let next_input_idx =
            (next_io_indices & NEXT_INPUT_MASK) >> NEXT_INPUT_MASK.trailing_zeros();
        let next_output_idx =
            (next_io_indices & NEXT_OUTPUT_MASK) >> NEXT_OUTPUT_MASK.trailing_zeros();
        for next_idx in [next_input_idx, next_output_idx] {
            debug_assert_ne!(
                next_idx, NO_BUFFER_IDX,
                "No Input or Output should exist at TripleBuffer destruction time"
            );
        }
    }
}
//
unsafe impl<T: Send> Sync for TripleBuffer<T> {}
//
// Cloning and equality are used internally for testing and I don't
// want to commit to supporting them publicly for now.
#[doc(hidden)]
impl<T: Clone + Send> TripleBuffer<T> {
    /// Like `Clone`, but requires exclusive access for safety
    fn seq_clone(&mut self) -> Self {
        Self {
            buffers: core::array::from_fn(|i| {
                CachePadded::new(UnsafeCell::new(self.buffers[i].get_mut().clone()))
            }),
            status: self.status.clone(),
        }
    }
}
//
#[doc(hidden)]
impl<T: PartialEq + Send> TripleBuffer<T> {
    /// Like `Eq`, but requires exclusive access for safety.
    fn seq_eq(&mut self, other: &mut Self) -> bool {
        // Check whether the contents of all buffers are equal...
        let buffers_equal = self
            .buffers
            .iter_mut()
            .zip(&mut other.buffers)
            .all(|(cell1, cell2)| -> bool { cell1.get_mut() == cell2.get_mut() });

        // ...then check whether the rest of the shared state is equal
        buffers_equal && self.status == other.status
    }
}

/// Access to a [`TripleBuffer`] from a shared reference or [`Arc`] thereof
///
/// While it may seem like this trait is unnecessary, it is actually needed to
/// keep the generic types of `Input` and `Output` simple and to allow for a
/// generic implementation of `inout()`.
pub trait TripleBufferRef: Clone + Deref + Send {
    /// Type of data that is being shared via this [`TripleBuffer`], i.e. the
    /// `T` argument to `TripleBuffer<T>`.
    type T: Send;

    /// Set up the [`Input`] and [`Output`] of this triple buffer in a single
    /// transaction
    ///
    /// This is more efficient than calling [`Input::new()`] and
    /// [`Output::new()`] separately. For maximal efficiency, you should also
    /// keep `Input`s and `Output`s alive as long as possible, ideally for the
    /// entire lifetime of the underlying triple buffer.
    ///
    /// Because this trait is implemented for multiple reference types and
    /// Rust's auto-deref makes calling trait methods on reference types
    /// confusing, it is strongly advised to call this method using the `<Ref as
    /// TripleBufferRef>::inout(...)` fully qualified syntax.
    ///
    /// # Panics
    ///
    /// Panics if this triple buffer already has an [`Input`] or [`Output`]
    fn inout(self) -> (Input<Self>, Output<Self>) {
        // Atomically fetch and clear the next input and output buffer indices
        let io_indices = self
            .status()
            .next_io_indices
            .swap(NEXT_NOTHING, Ordering::Acquire);

        // Decode input and output indices and make sure that no Input and
        // Output currently exists for this triple buffer
        let input_idx = (io_indices & NEXT_INPUT_MASK) >> NEXT_INPUT_MASK.trailing_zeros();
        let output_idx = (io_indices & NEXT_OUTPUT_MASK) >> NEXT_OUTPUT_MASK.trailing_zeros();
        for io_idx in [input_idx, output_idx] {
            assert_ne!(
                io_idx, NO_BUFFER_IDX,
                "This triple buffer already has an Input or Output"
            );
        }

        // Set up the producer and consumer interfaces
        let input = Input {
            shared: self.clone(),
            input_idx,
        };
        (
            input,
            Output {
                shared: self,
                output_idx,
            },
        )
    }

    /// Internal access to the private `buffers` member
    #[doc(hidden)]
    fn buffers(&self) -> &[CachePadded<UnsafeCell<Self::T>>; 3];

    /// Internal access to the private `status` member
    #[allow(private_interfaces)]
    #[doc(hidden)]
    fn status<'a>(&'a self) -> &'a Status
    where
        Self::T: 'a;
}
//
impl<T, Ref> TripleBufferRef for Ref
where
    Ref: Clone + Deref<Target = TripleBuffer<T>> + Send,
    T: Send,
{
    type T = T;

    #[inline(always)]
    fn buffers(&self) -> &[CachePadded<UnsafeCell<T>>; 3] {
        &self.buffers
    }

    #[allow(private_interfaces)]
    #[inline(always)]
    fn status<'a>(&'a self) -> &'a Status
    where
        Self::T: 'a,
    {
        &self.status
    }
}

/// Producer interface to the triple buffer
///
/// The producer thread can use this struct to submit updates to the triple
/// buffer whenever he likes. These updates are nonblocking: a collision between
/// the producer and the consumer will result in cache contention, but deadlocks
/// and scheduling-induced slowdowns cannot happen.
#[derive(Debug)]
pub struct Input<Shared: TripleBufferRef> {
    /// Reference to the shared state
    shared: Shared,

    /// Index of the input buffer (which is private to the producer)
    input_idx: BufferIdx,
}
//
// Public interface
impl<Shared: TripleBufferRef> Input<Shared> {
    /// Set up the producer interface of a triple buffer
    ///
    /// It is more efficient to set up both the producer and consumer interfaces
    /// at the same time using [`TripleBufferRef::inout()`] and its convenience
    /// shortcuts [`TripleBuffer::inout_from_arc()`] and
    /// [`TripleBuffer::inout_from_ref()`].
    ///
    /// # Panics
    ///
    /// Panics if this triple buffer already has an [`Input`]
    pub fn new(shared: Shared) -> Self {
        let mut input_idx = NO_BUFFER_IDX;
        shared
            .status()
            .next_io_indices
            .fetch_update(Ordering::Release, Ordering::Relaxed, |old_next_indices| {
                input_idx =
                    (old_next_indices & NEXT_INPUT_MASK) >> NEXT_INPUT_MASK.trailing_zeros();
                assert_ne!(
                    input_idx, NO_BUFFER_IDX,
                    "This triple buffer already has an Input"
                );
                Some(
                    (old_next_indices & !NEXT_INPUT_MASK)
                        | (NO_BUFFER_IDX << NEXT_INPUT_MASK.trailing_zeros()),
                )
            })
            .expect("Not allowed to fail");
        Self { shared, input_idx }
    }

    /// Write a new value into the triple buffer
    pub fn write(&mut self, value: Shared::T) {
        // Update the input buffer
        *self.input_buffer_mut() = value;

        // Publish our update to the consumer
        self.publish();
    }

    /// Check if the consumer has fetched our latest submission yet
    ///
    /// This method is only intended for diagnostics purposes. Please do not let
    /// it inform your decision of sending or not sending a value, as that would
    /// effectively be building a very poor spinlock-based double buffer
    /// implementation. If what you truly need is a double buffer, build
    /// yourself a proper blocking one instead of wasting CPU time.
    pub fn consumed(&self) -> bool {
        let back_info = self.shared.status().back_info.load(Ordering::Relaxed);
        back_info & BACK_UPDATED_BIT == 0
    }

    /// Query the current value of the input buffer
    ///
    /// This is a read-only version of
    /// [`input_buffer_mut()`](Input::input_buffer_mut). Please read the
    /// documentation of that method for more information on the precautions
    /// that need to be taken when accessing the input buffer in place.
    pub fn input_buffer(&self) -> &Shared::T {
        // This is safe because the synchronization protocol ensures that we
        // have exclusive access to this buffer.
        let input_ptr = self.shared.buffers()[self.input_idx as usize].get();
        unsafe { &*input_ptr }
    }

    /// Access the input buffer directly
    ///
    /// This advanced interface allows you to update the input buffer in place,
    /// so that you can avoid creating values of type T repeatedy just to push
    /// them into the triple buffer when doing so is expensive.
    ///
    /// However, by using it, you force yourself to take into account some
    /// implementation subtleties that you could otherwise ignore.
    ///
    /// First, the buffer does not contain the last value that you published
    /// (which is now available to the consumer thread). In fact, what you get
    /// may not match _any_ value that you sent in the past, but rather be a new
    /// value that was written in there by the consumer thread. All you can
    /// safely assume is that the buffer contains a valid value of type T, which
    /// you may need to "clean up" before use using a type-specific process
    /// (like calling the `clear()` method of a `Vec`/`String`).
    ///
    /// Second, we do not send updates automatically. You need to call
    /// [`publish()`](Input::publish) in order to propagate a buffer update to
    /// the consumer. If you would prefer this to be done automatically when the
    /// input buffer reference goes out of scope, consider using the
    /// [`input_buffer_publisher()`](Input::input_buffer_publisher) RAII
    /// interface instead.
    pub fn input_buffer_mut(&mut self) -> &mut Shared::T {
        // This is safe because the synchronization protocol ensures that we
        // have exclusive access to this buffer.
        let input_ptr = self.shared.buffers()[self.input_idx as usize].get();
        unsafe { &mut *input_ptr }
    }

    /// Publish the current input buffer, checking for overwrites
    ///
    /// After updating the input buffer in-place using
    /// [`input_buffer_mut()`](Input::input_buffer_mut), you can use this method
    /// to publish your updates to the consumer. Beware that this will replace
    /// the current input buffer with another one that has totally unrelated
    /// contents.
    ///
    /// It will also tell you whether you overwrote a value which was not read
    /// by the consumer thread.
    pub fn publish(&mut self) -> bool {
        // Swap the input buffer and the back buffer, setting the dirty bit
        //
        // The ordering must be AcqRel, because...
        //
        // - Our accesses to the old buffer must not be reordered after this
        //   operation (which mandates Release ordering), otherwise they could
        //   race with the consumer accessing the freshly published buffer.
        // - Our accesses from the buffer must not be reordered before this
        //   operation (which mandates Consume ordering, that is best
        //   approximated by Acquire in Rust), otherwise they would race with
        //   the consumer accessing the buffer as well before switching to
        //   another buffer.
        //   * This reordering may seem paradoxical, but could happen if the
        //     compiler or CPU correctly speculated the new buffer's index
        //     before that index is actually read, as well as on weird hardware
        //     with incoherent caches like GPUs or old DEC Alpha where keeping
        //     data in sync across cores requires manual action.
        let former_back_info = self.shared.status().back_info.swap(
            self.input_idx as BackInfo | BACK_UPDATED_BIT,
            Ordering::AcqRel,
        );

        // The old back buffer becomes our new input buffer
        self.input_idx = (former_back_info & BACK_IDX_MASK) as BufferIdx;

        // Tell whether we have overwritten unread data
        former_back_info & BACK_UPDATED_BIT != 0
    }

    /// Access the input buffer wrapped in the `InputPublishGuard`
    ///
    /// This is an RAII alternative to the [`input_buffer_mut()`]/[`publish()`]
    /// workflow where the [`publish()`] transaction happens automatically when
    /// the input buffer handle goes out of scope.
    ///
    /// Please check out the documentation of [`input_buffer_mut()`] and
    /// [`publish()`] to know more about the precautions that you need to take
    /// when using the lower-level in-place buffer access interface.
    ///
    /// [`input_buffer_mut()`]: Input::input_buffer_mut
    /// [`publish()`]: Input::publish
    pub fn input_buffer_publisher(&mut self) -> InputPublishGuard<Shared> {
        InputPublishGuard(self)
    }
}
//
impl<Shared: TripleBufferRef> Drop for Input<Shared> {
    fn drop(&mut self) {
        // Save the current input buffer index for the next time an `Input`
        // will be built and check state consistency in debug builds.
        self.shared
            .status()
            .next_io_indices
            .fetch_update(Ordering::Release, Ordering::Relaxed, |old_next_indices| {
                let old_next_input_idx =
                    (old_next_indices & NEXT_INPUT_MASK) >> NEXT_INPUT_MASK.trailing_zeros();
                debug_assert_eq!(
                    old_next_input_idx, NO_BUFFER_IDX,
                    "The shared state should not contain a next input index while an Input is live"
                );
                Some(
                    (old_next_indices & !NEXT_INPUT_MASK)
                        | (self.input_idx << NEXT_INPUT_MASK.trailing_zeros()),
                )
            })
            .expect("Not allowed to fail");
    }
}

/// RAII Guard to the buffer provided by an [`Input`].
///
/// The current buffer of the [`Input`] can be accessed through this guard via
/// its [`Deref`] and [`DerefMut`] implementations.
///
/// When the guard is dropped, [`Input::publish()`] will be called
/// automatically.
///
/// This structure is created by the [`Input::input_buffer_publisher()`] method.
pub struct InputPublishGuard<'a, Shared: TripleBufferRef>(&'a mut Input<Shared>);
//
impl<Shared: TripleBufferRef> Debug for InputPublishGuard<'_, Shared>
where
    Shared::T: Debug,
{
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        Debug::fmt(&self.0.input_buffer(), f)
    }
}
//q
impl<Shared: TripleBufferRef> Deref for InputPublishGuard<'_, Shared> {
    type Target = Shared::T;

    fn deref(&self) -> &Shared::T {
        self.0.input_buffer()
    }
}
//
impl<Shared: TripleBufferRef> DerefMut for InputPublishGuard<'_, Shared> {
    fn deref_mut(&mut self) -> &mut Shared::T {
        self.0.input_buffer_mut()
    }
}
//
impl<Shared: TripleBufferRef> Display for InputPublishGuard<'_, Shared>
where
    Shared::T: Display,
{
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        Display::fmt(self.0.input_buffer(), f)
    }
}
//
impl<Shared: TripleBufferRef> Drop for InputPublishGuard<'_, Shared> {
    #[inline]
    fn drop(&mut self) {
        self.0.publish();
    }
}

/// Consumer interface to the triple buffer
///
/// The consumer thread can use this struct to access the latest published
/// update from the producer whenever he likes. Readout is nonblocking: a
/// collision between the producer and consumer will result in cache contention,
/// but deadlocks and scheduling-induced slowdowns cannot happen.
#[derive(Debug)]
pub struct Output<Shared: TripleBufferRef> {
    /// Reference to the shared state
    shared: Shared,

    /// Index of the output buffer (which is private to the consumer)
    output_idx: BufferIdx,
}
//
// Public interface
impl<Shared: TripleBufferRef> Output<Shared> {
    /// Set up the consumer interface of a triple buffer
    ///
    /// It is more efficient to set up both the producer and consumer interfaces
    /// at the same time using [`TripleBufferRef::inout()`] and its convenience
    /// shortcuts [`TripleBuffer::inout_from_arc()`] and
    /// [`TripleBuffer::inout_from_ref()`].
    ///
    /// # Panics
    ///
    /// Panics if this triple buffer already has an [`Output`]
    pub fn new(shared: Shared) -> Self {
        let mut output_idx = NO_BUFFER_IDX;
        shared
            .status()
            .next_io_indices
            .fetch_update(Ordering::Release, Ordering::Relaxed, |old_next_indices| {
                output_idx =
                    (old_next_indices & NEXT_OUTPUT_MASK) >> NEXT_OUTPUT_MASK.trailing_zeros();
                assert_ne!(
                    output_idx, NO_BUFFER_IDX,
                    "This triple buffer already has an Output"
                );
                Some(
                    (old_next_indices & !NEXT_OUTPUT_MASK)
                        | (NO_BUFFER_IDX << NEXT_OUTPUT_MASK.trailing_zeros()),
                )
            })
            .expect("Not allowed to fail");
        Self { shared, output_idx }
    }

    /// Access the latest value from the triple buffer
    pub fn read(&mut self) -> &Shared::T {
        // Fetch updates from the producer
        self.update();

        // Give access to the output buffer
        self.output_buffer_mut()
    }

    /// Tell whether an updated value has been submitted by the producer
    ///
    /// This method is mainly intended for diagnostics purposes. Please do not
    /// let it inform your decision of reading a value or not, as that would
    /// effectively be building a very poor spinlock-based double buffer
    /// implementation. If what you truly need is a double buffer, build
    /// yourself a proper blocking one instead of wasting CPU time.
    pub fn updated(&self) -> bool {
        let back_info = self.shared.status().back_info.load(Ordering::Relaxed);
        back_info & BACK_UPDATED_BIT != 0
    }

    /// Query the current value of the output buffer
    ///
    /// This is a deprecated compatibility alias to
    /// [`output_buffer()`](Self::output_buffer). Please use that method
    /// instead, as `peek_output_buffer()` is scheduled for removal in the next
    /// major release of `triple-buffer`.
    #[deprecated = "Please use output_buffer() instead"]
    pub fn peek_output_buffer(&self) -> &Shared::T {
        self.output_buffer()
    }

    /// Query the current value of the output buffer
    ///
    /// This is a read-only version of
    /// [`output_buffer_mut()`](Output::output_buffer_mut). Please read the
    /// documentation of that method for more information on the precautions
    /// that need to be taken when accessing the output buffer in place.
    ///
    /// In particular, remember that this method does not update the output
    /// buffer automatically. You need to call [`update()`](Output::update) in
    /// order to fetch buffer updates from the producer.
    pub fn output_buffer(&self) -> &Shared::T {
        // This is safe because the synchronization protocol ensures that we
        // have exclusive access to this buffer.
        let output_ptr = self.shared.buffers()[self.output_idx as usize].get();
        unsafe { &*output_ptr }
    }

    /// Access the output buffer directly
    ///
    /// This advanced interface allows you to modify the contents of the output
    /// buffer, so that you can avoid copying the output value when this is an
    /// expensive process. One possible application, for example, is to
    /// post-process values from the producer before use.
    ///
    /// However, by using it, you force yourself to take into account some
    /// implementation subtleties that you could normally ignore.
    ///
    /// First, keep in mind that you can lose access to the current output
    /// buffer any time [`read()`] or [`update()`] is called, as it may be
    /// replaced by an updated buffer from the producer automatically.
    ///
    /// Second, to reduce the potential for the aforementioned usage error, this
    /// method does not update the output buffer automatically. You need to call
    /// [`update()`] in order to fetch buffer updates from the producer.
    ///
    /// [`read()`]: Output::read
    /// [`update()`]: Output::update
    pub fn output_buffer_mut(&mut self) -> &mut Shared::T {
        // This is safe because the synchronization protocol ensures that we
        // have exclusive access to this buffer.
        let output_ptr = self.shared.buffers()[self.output_idx as usize].get();
        unsafe { &mut *output_ptr }
    }

    /// Update the output buffer
    ///
    /// Check if the producer submitted a new data version, and if one is
    /// available, update our output buffer to use it. Return a flag that tells
    /// you whether such an update was carried out.
    ///
    /// Bear in mind that when this happens, you will lose any change that you
    /// performed to the output buffer via the
    /// [`output_buffer_mut()`](Output::output_buffer_mut) interface.
    pub fn update(&mut self) -> bool {
        // Check if an update is present in the back-buffer
        let updated = self.updated();
        if updated {
            // If so, exchange our output buffer with the back-buffer, thusly
            // acquiring exclusive access to the old back buffer while giving
            // the producer a new back-buffer to write to.
            //
            // The ordering must be AcqRel, because...
            //
            // - Our accesses to the previous buffer must not be reordered after
            //   this operation (which mandates Release ordering), otherwise
            //   they could race with the producer accessing the freshly
            //   liberated buffer.
            // - Our accesses from the buffer must not be reordered before this
            //   operation (which mandates Consume ordering, that is best
            //   approximated by Acquire in Rust), otherwise they would race
            //   with the producer writing into the buffer before publishing it.
            //   * This reordering may seem paradoxical, but could happen if the
            //     compiler or CPU correctly speculated the new buffer's index
            //     before that index is actually read, as well as on weird hardware
            //     like GPUs where CPU caches require manual synchronization.
            let former_back_info = self
                .shared
                .status()
                .back_info
                .swap(self.output_idx as BackInfo, Ordering::AcqRel);

            // Make the old back-buffer our new output buffer
            self.output_idx = (former_back_info & BACK_IDX_MASK) as BufferIdx;
        }

        // Tell whether an update was carried out
        updated
    }
}
//
impl<Shared: TripleBufferRef> Drop for Output<Shared> {
    fn drop(&mut self) {
        // Save the current output buffer index for the next time an `Output`
        // will be built and check state consistency in debug builds.
        self.shared
            .status()
            .next_io_indices
            .fetch_update(Ordering::Release, Ordering::Relaxed, |old_next_indices| {
                let old_next_output_idx =
                    (old_next_indices & NEXT_OUTPUT_MASK) >> NEXT_OUTPUT_MASK.trailing_zeros();
                debug_assert_eq!(
                    old_next_output_idx, NO_BUFFER_IDX,
                    "The shared state should not contain a next output index while an Output is live"
                );
                Some(
                    (old_next_indices & !NEXT_OUTPUT_MASK)
                        | (self.output_idx << NEXT_OUTPUT_MASK.trailing_zeros()),
                )
            })
            .expect("Not allowed to fail");
    }
}

/// Triple buffer status information
///
/// Should be [`CachePadded`] to avoid false sharing with the data buffers.
#[derive(Debug)]
struct Status {
    /// Back buffer index + truth that an update is pending
    back_info: AtomicBackInfo,

    /// Next input and output indices, bit-packed in a single word
    next_io_indices: AtomicNextIndices,
}
//
impl Status {
    fn new() -> Self {
        Self {
            back_info: AtomicBackInfo::new(BACK_INFO_INIT),
            next_io_indices: AtomicNextIndices::new(NEXT_INDICES_INIT),
        }
    }
}
//
impl Clone for Status {
    /// Meant for sequential testing code paths only
    ///
    /// Use on a [`Status`] that is being modified concurrently will result in
    /// unpredictable results, including observation of inconsistent states.
    fn clone(&self) -> Self {
        Self {
            back_info: AtomicBackInfo::new(self.back_info.load(Ordering::Relaxed)),
            next_io_indices: AtomicNextIndices::new(self.next_io_indices.load(Ordering::Relaxed)),
        }
    }
}
//
impl Default for Status {
    fn default() -> Self {
        Self::new()
    }
}
//
impl PartialEq for Status {
    /// Meant for sequential testing code paths only
    ///
    /// Use on a [`Status`] that is being modified concurrently will result in
    /// unpredictable results, including observation of inconsistent states.
    fn eq(&self, other: &Self) -> bool {
        self.back_info.load(Ordering::Relaxed) == other.back_info.load(Ordering::Relaxed)
            && self.next_io_indices.load(Ordering::Relaxed)
                == other.next_io_indices.load(Ordering::Relaxed)
    }
}

/// Type used for buffer indices (in range 0..=2)
type BufferIdx = u8;

/// Bitfield that tracks...
///
/// - The index of the current back buffer, i.e. the buffer which neither the
///   producer nor the consumer is currently processing.
/// - The truth that the producer published a new back buffer since the last
///   consumer update.
///
/// See the `BACK_XYZ` constants for layout details.
///
/// This amount of information fits in a [`u8`]. However `BackInfo` is a
/// [`usize`] because it is frequently updated using atomic operations and we
/// would rather not take bets about how the efficiency of sub-word atomic
/// operations on the target CPU.
type BackInfo = usize;
//
/// Atomic version of [`BackInfo`]
type AtomicBackInfo = AtomicUsize;
//
/// Low-order [`BackInfo`] bits track the index of the back buffer
const BACK_IDX_MASK: BackInfo = 0b0_11;
//
/// The next bit tracks the truth that the producer has updated the back buffer
/// since the last consumer update..
const BACK_UPDATED_BIT: BackInfo = 0b1_00;
//
/// Initial [`Status`]: back buffer is #0, no update
const BACK_INFO_INIT: BackInfo = 0b0_00;

/// Either a valid buffer index from 0 to 2, or a [`NO_BUFFER_INDEX`] sentinel
/// value denoting an absence of buffer index
type MaybeBufferIdx = BufferIdx;
//
/// Sentinel value stored in a [`MaybeBufferIdx`] to indicate an absence of
/// buffer index.
const NO_BUFFER_IDX: MaybeBufferIdx = 0b11;

/// A pair of [`MaybeBufferIdx`] packed into a single [`AtomicU8`], representing
/// the next input and output buffer indices if any.
///
/// Using the input index for the sake of illustration...
///
/// - If no [`Input`] currently exists, then the next input index is the buffer
///   index that a newly created [`Input`] should use
///   * At initialization time this is a hardcoded initial input index
///   * If an [`Input`] existed before, but has been destroyed, then this
///     is a backup of the previous [`Input::input_idx`], to be reused
///     next time an `Input` is created.
/// - As long an [`Input`] exists, this is [`NO_BUFFER_IDX`], because the
///   input index is tracked by the [`Input`] not the shared state.
///
/// The output index works identically, but with [`Output`] taking the place
/// of [`Input`], and the hardcoded initial output index is different.
///
/// This is a [`u8`] even though sub-word atomics may be inefficient, because
/// this data is only accessed when an [`Input`] or [`Output`] is built or
/// destroyed, and that operation should be rare in well-behaved client code.
///
/// For the same reason, this data can be packed in the same cache line as the
/// [`BackInfo`], as penalties related to this false sharing should not be paid
/// often. Packing two indices into a single [`u8`] is fair game too, and this
/// allows creating an [`Input`]/[`Output`] pair using a single atomic
/// transaction as a small efficiency bonus.
type NextIndices = u8;
//
/// Atomic version of [`NextIndices`]
type AtomicNextIndices = AtomicU8;
//
/// Mask covering the next input index
const NEXT_INPUT_MASK: NextIndices = 0b00_11;
//
/// Mask covering the next output index
const NEXT_OUTPUT_MASK: NextIndices = 0b11_00;
//
/// Initial [`Status`]: input buffer is #1, output buffer is #2
const NEXT_INDICES_INIT: NextIndices = 0b10_01;
//
/// Absence of next input and output index
///
/// Per the above definitions, this is the expected value of the next indices of
/// a [`TripleBuffer`] when it has both [`Input`] and an [`Output`].
const NEXT_NOTHING: NextIndices = (NO_BUFFER_IDX << NEXT_INPUT_MASK.trailing_zeros())
    | (NO_BUFFER_IDX << NEXT_OUTPUT_MASK.trailing_zeros());

/// Unit tests
#[cfg(test)]
mod tests {
    use super::{BufferIdx, TripleBuffer, STATUS_BACK_INDEX_MASK, STATUS_DIRTY_BIT};
    use std::{fmt::Debug, ops::Deref, sync::atomic::Ordering, thread, time::Duration};
    use testbench::race_cell::{RaceCell, Racey};

    /// Check that triple buffers are properly initialized
    #[test]
    fn initial_state() {
        // Let's create a triple buffer
        let mut buf = TripleBuffer::new(&42);
        check_buf_state(&mut buf, false);
        assert_eq!(*buf.output.read(), 42);
    }

    /// Check that the shared state's unsafe equality operator works
    #[test]
    fn partial_eq_shared() {
        // Let's create some dummy shared state
        let dummy_state = SharedState::<u16>::new(|i| [111, 222, 333][i], 0b10);

        // Check that the dummy state is equal to itself
        assert!(unsafe { dummy_state.eq(&dummy_state) });

        // Check that it's not equal to a state where buffer contents differ
        assert!(unsafe { !dummy_state.eq(&SharedState::<u16>::new(|i| [114, 222, 333][i], 0b10)) });
        assert!(unsafe { !dummy_state.eq(&SharedState::<u16>::new(|i| [111, 225, 333][i], 0b10)) });
        assert!(unsafe { !dummy_state.eq(&SharedState::<u16>::new(|i| [111, 222, 336][i], 0b10)) });

        // Check that it's not equal to a state where the back info differs
        assert!(unsafe {
            !dummy_state.eq(&SharedState::<u16>::new(
                |i| [111, 222, 333][i],
                STATUS_DIRTY_BIT & 0b10,
            ))
        });
        assert!(unsafe { !dummy_state.eq(&SharedState::<u16>::new(|i| [111, 222, 333][i], 0b01)) });
    }

    /// Check that TripleBuffer's PartialEq impl works
    #[test]
    fn partial_eq() {
        // Create a triple buffer
        let buf = TripleBuffer::new(&"test");

        // Check that it is equal to itself
        assert_eq!(buf, buf);

        // Make another buffer with different contents. As buffer creation is
        // deterministic, this should only have an impact on the shared state,
        // but the buffers should nevertheless be considered different.
        let buf2 = TripleBuffer::new(&"taste");
        assert_eq!(buf.input.input_idx, buf2.input.input_idx);
        assert_eq!(buf.output.output_idx, buf2.output.output_idx);
        assert!(buf != buf2);

        // Check that changing either the input or output buffer index will
        // also lead two TripleBuffers to be considered different (this test
        // technically creates an invalid TripleBuffer state, but it's the only
        // way to check that the PartialEq impl is exhaustive)
        let mut buf3 = TripleBuffer::new(&"test");
        assert_eq!(buf, buf3);
        let old_input_idx = buf3.input.input_idx;
        buf3.input.input_idx = buf3.output.output_idx;
        assert!(buf != buf3);
        buf3.input.input_idx = old_input_idx;
        buf3.output.output_idx = old_input_idx;
        assert!(buf != buf3);
    }

    /// Check that the shared state's unsafe clone operator works
    #[test]
    fn clone_shared() {
        // Let's create some dummy shared state
        let dummy_state = SharedState::<u8>::new(|i| [123, 231, 132][i], STATUS_DIRTY_BIT & 0b01);

        // Now, try to clone it
        let dummy_state_copy = unsafe { dummy_state.clone() };

        // Check that the contents of the original state did not change
        assert!(unsafe {
            dummy_state.eq(&SharedState::<u8>::new(
                |i| [123, 231, 132][i],
                STATUS_DIRTY_BIT & 0b01,
            ))
        });

        // Check that the contents of the original and final state are identical
        assert!(unsafe { dummy_state.eq(&dummy_state_copy) });
    }

    /// Check that TripleBuffer's Clone impl works
    #[test]
    fn clone() {
        // Create a triple buffer
        let mut buf = TripleBuffer::new(&4.2);

        // Put it in a nontrivial state
        unsafe {
            *buf.input.shared.buffers[0].get() = 1.2;
            *buf.input.shared.buffers[1].get() = 3.4;
            *buf.input.shared.buffers[2].get() = 5.6;
        }
        buf.input
            .shared
            .status_bits
            .store(STATUS_DIRTY_BIT & 0b01, Ordering::Relaxed);
        buf.input.input_idx = 0b10;
        buf.output.output_idx = 0b00;

        // Now clone it
        let buf_clone = buf.clone();

        // Check that the clone uses its own, separate shared data storage
        assert_eq!(
            as_ptr(&buf_clone.input.shared),
            as_ptr(&buf_clone.output.shared)
        );
        assert_ne!(as_ptr(&buf_clone.input.shared), as_ptr(&buf.input.shared));
        assert_ne!(as_ptr(&buf_clone.output.shared), as_ptr(&buf.output.shared));

        // Check that it is identical from PartialEq's point of view
        assert_eq!(buf, buf_clone);

        // Check that the contents of the original buffer did not change
        unsafe {
            assert_eq!(*buf.input.shared.buffers[0].get(), 1.2);
            assert_eq!(*buf.input.shared.buffers[1].get(), 3.4);
            assert_eq!(*buf.input.shared.buffers[2].get(), 5.6);
        }
        assert_eq!(
            buf.input.shared.status_bits.load(Ordering::Relaxed),
            STATUS_DIRTY_BIT & 0b01
        );
        assert_eq!(buf.input.input_idx, 0b10);
        assert_eq!(buf.output.output_idx, 0b00);
    }

    /// Check that the low-level publish/update primitives work
    #[test]
    fn swaps() {
        // Create a new buffer, and a way to track any changes to it
        let mut buf = TripleBuffer::new(&[123, 456]);
        let old_buf = buf.clone();
        let old_input_idx = old_buf.input.input_idx;
        let old_shared = &old_buf.input.shared;
        let old_back_info = old_shared.status_bits.load(Ordering::Relaxed);
        let old_back_idx = old_back_info & STATUS_BACK_INDEX_MASK;
        let old_output_idx = old_buf.output.output_idx;

        // Check that updating from a clean state works
        assert!(!buf.output.update());
        assert_eq!(buf, old_buf);
        check_buf_state(&mut buf, false);

        // Check that publishing from a clean state works
        assert!(!buf.input.publish());
        let mut expected_buf = old_buf.clone();
        expected_buf.input.input_idx = old_back_idx;
        expected_buf
            .input
            .shared
            .status_bits
            .store(old_input_idx | STATUS_DIRTY_BIT, Ordering::Relaxed);
        assert_eq!(buf, expected_buf);
        check_buf_state(&mut buf, true);

        // Check that overwriting a dirty state works
        assert!(buf.input.publish());
        let mut expected_buf = old_buf.clone();
        expected_buf.input.input_idx = old_input_idx;
        expected_buf
            .input
            .shared
            .status_bits
            .store(old_back_idx | STATUS_DIRTY_BIT, Ordering::Relaxed);
        assert_eq!(buf, expected_buf);
        check_buf_state(&mut buf, true);

        // Check that updating from a dirty state works
        assert!(buf.output.update());
        expected_buf.output.output_idx = old_back_idx;
        expected_buf
            .output
            .shared
            .status_bits
            .store(old_output_idx, Ordering::Relaxed);
        assert_eq!(buf, expected_buf);
        check_buf_state(&mut buf, false);
    }

    /// Check that writing to a triple buffer works
    #[test]
    fn vec_guarded_write() {
        let mut buf = TripleBuffer::new(&vec![]);

        // write new value, publish, read
        {
            let mut buffer = buf.input.input_buffer_publisher();
            buffer.push(0);
            buffer.push(1);
            buffer.push(2);

            // not yet published
            let back_info = buffer.reference.shared.status_bits.load(Ordering::Relaxed);
            let back_buffer_dirty = back_info & STATUS_DIRTY_BIT != 0;
            assert!(!back_buffer_dirty);
        }
        check_buf_state(&mut buf, true); // after publish, before read
        assert_eq!(*buf.output.read(), vec![0, 1, 2]);
        check_buf_state(&mut buf, false); // after publish and read

        // write new value, publish, don't read
        {
            buf.input.input_buffer_publisher().push(3);
        }
        check_buf_state(&mut buf, true);

        // write new value, publish, read
        {
            buf.input.input_buffer_publisher().push(4);
        }
        assert_eq!(*buf.output.read(), vec![4]);
        check_buf_state(&mut buf, false);

        // overwrite existing value, publish, surprising read
        {
            buf.input.input_buffer_publisher().push(5);
        }
        assert_eq!(*buf.output.read(), vec![3, 5]);
        check_buf_state(&mut buf, false);

        // to avoid surprise, always clear before write
        {
            let mut buffer = buf.input.input_buffer_publisher();
            buffer.clear();
            buffer.push(6);
        }
        assert_eq!(*buf.output.read(), vec![6]);
        check_buf_state(&mut buf, false);
    }

    /// Check that (sequentially) writing to a triple buffer works
    #[test]
    fn sequential_write() {
        // Let's create a triple buffer
        let mut buf = TripleBuffer::new(&false);

        // Back up the initial buffer state
        let old_buf = buf.clone();

        // Perform a write
        buf.input.write(true);

        // Check new implementation state
        {
            // Starting from the old buffer state...
            let mut expected_buf = old_buf.clone();

            // ...write the new value in and swap...
            *expected_buf.input.input_buffer_mut() = true;
            expected_buf.input.publish();

            // Nothing else should have changed
            assert_eq!(buf, expected_buf);
            check_buf_state(&mut buf, true);
        }
    }

    /// Check that (sequentially) writing to a triple buffer works
    #[test]
    fn sequential_guarded_write() {
        // Let's create a triple buffer
        let mut buf = TripleBuffer::new(&false);

        // Back up the initial buffer state
        let old_buf = buf.clone();

        // Perform a write
        *buf.input.input_buffer_publisher() = true;

        // Check new implementation state
        {
            // Starting from the old buffer state...
            let mut expected_buf = old_buf.clone();

            // ...write the new value in and swap...
            *expected_buf.input.input_buffer_mut() = true;
            expected_buf.input.publish();

            // Nothing else should have changed
            assert_eq!(buf, expected_buf);
            check_buf_state(&mut buf, true);
        }
    }

    /// Check that (sequentially) reading from a triple buffer works
    #[test]
    fn sequential_read() {
        // Let's create a triple buffer and write into it
        let mut buf = TripleBuffer::new(&1.0);
        buf.input.write(4.2);

        // Test readout from dirty (freshly written) triple buffer
        {
            // Back up the initial buffer state
            let old_buf = buf.clone();

            // Read from the buffer
            let result = *buf.output.read();

            // Output value should be correct
            assert_eq!(result, 4.2);

            // Result should be equivalent to carrying out an update
            let mut expected_buf = old_buf.clone();
            assert!(expected_buf.output.update());
            assert_eq!(buf, expected_buf);
            check_buf_state(&mut buf, false);
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
            check_buf_state(&mut buf, false);
        }
    }

    /// Check that (sequentially) reading from a triple buffer works
    #[test]
    fn sequential_guarded_read() {
        // Let's create a triple buffer and write into it
        let mut buf = TripleBuffer::new(&1.0);
        *buf.input.input_buffer_publisher() = 4.2;

        // Test readout from dirty (freshly written) triple buffer
        {
            // Back up the initial buffer state
            let old_buf: TripleBuffer<f64> = buf.clone();

            // Read from the buffer
            let result = *buf.output.read();

            // Output value should be correct
            assert_eq!(result, 4.2);

            // Result should be equivalent to carrying out an update
            let mut expected_buf = old_buf.clone();
            assert!(expected_buf.output.update());
            assert_eq!(buf, expected_buf);
            check_buf_state(&mut buf, false);
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
            check_buf_state(&mut buf, false);
        }
    }

    /// Check that contended concurrent reads and writes work
    #[test]
    #[ignore]
    fn contended_concurrent_read_write() {
        // We will stress the infrastructure by performing this many writes
        // as a reader continuously reads the latest value
        #[cfg(not(feature = "miri"))]
        const TEST_WRITE_COUNT: usize = 100_000_000;
        #[cfg(feature = "miri")]
        const TEST_WRITE_COUNT: usize = 3_000;

        // This is the buffer that our reader and writer will share
        let buf = TripleBuffer::new(&RaceCell::new(0));
        let (mut buf_input, mut buf_output) = buf.split();

        // Concurrently run a writer which increments a shared value in a loop,
        // and a reader which makes sure that no unexpected value slips in.
        let mut last_value = 0usize;
        testbench::concurrent_test_2(
            move || {
                for value in 1..=TEST_WRITE_COUNT {
                    buf_input.write(RaceCell::new(value));
                }
            },
            move || {
                while last_value < TEST_WRITE_COUNT {
                    let new_racey_value = buf_output.read().get();
                    match new_racey_value {
                        Racey::Consistent(new_value) => {
                            assert!((new_value >= last_value) && (new_value <= TEST_WRITE_COUNT));
                            last_value = new_value;
                        }
                        Racey::Inconsistent => {
                            panic!("Inconsistent state exposed by the buffer!");
                        }
                    }
                }
            },
        );
    }

    /// Check that uncontended concurrent reads and writes work
    ///
    /// **WARNING:** This test unfortunately needs to have timing-dependent
    /// behaviour to do its job. If it fails for you, try the following:
    ///
    /// - Close running applications in the background
    /// - Re-run the tests with only one OS thread (--test-threads=1)
    /// - Increase the writer sleep period
    #[test]
    #[ignore]
    fn uncontended_concurrent_read_write() {
        // We will stress the infrastructure by performing this many writes
        // as a reader continuously reads the latest value
        #[cfg(not(feature = "miri"))]
        const TEST_WRITE_COUNT: usize = 625;
        #[cfg(feature = "miri")]
        const TEST_WRITE_COUNT: usize = 200;

        // This is the buffer that our reader and writer will share
        let buf = TripleBuffer::new(&RaceCell::new(0));
        let (mut buf_input, mut buf_output) = buf.split();

        // Concurrently run a writer which slowly increments a shared value,
        // and a reader which checks that it can receive every update
        let mut last_value = 0usize;
        testbench::concurrent_test_2(
            move || {
                for value in 1..=TEST_WRITE_COUNT {
                    buf_input.write(RaceCell::new(value));
                    thread::yield_now();
                    thread::sleep(Duration::from_millis(32));
                }
            },
            move || {
                while last_value < TEST_WRITE_COUNT {
                    let new_racey_value = buf_output.read().get();
                    match new_racey_value {
                        Racey::Consistent(new_value) => {
                            assert!((new_value >= last_value) && (new_value - last_value <= 1));
                            last_value = new_value;
                        }
                        Racey::Inconsistent => {
                            panic!("Inconsistent state exposed by the buffer!");
                        }
                    }
                }
            },
        );
    }

    /// Through the low-level API, the consumer is allowed to modify its
    /// bufffer, which means that it will unknowingly send back data to the
    /// producer. This creates new correctness requirements for the
    /// synchronization protocol, which must be checked as well.
    #[test]
    #[ignore]
    fn concurrent_bidirectional_exchange() {
        // We will stress the infrastructure by performing this many writes
        // as a reader continuously reads the latest value
        #[cfg(not(feature = "miri"))]
        const TEST_WRITE_COUNT: usize = 100_000_000;
        #[cfg(feature = "miri")]
        const TEST_WRITE_COUNT: usize = 3_000;

        // This is the buffer that our reader and writer will share
        let buf = TripleBuffer::new(&RaceCell::new(0));
        let (mut buf_input, mut buf_output) = buf.split();

        // Concurrently run a writer which increments a shared value in a loop,
        // and a reader which makes sure that no unexpected value slips in.
        testbench::concurrent_test_2(
            move || {
                for new_value in 1..=TEST_WRITE_COUNT {
                    match buf_input.input_buffer_mut().get() {
                        Racey::Consistent(curr_value) => {
                            assert!(curr_value <= new_value);
                        }
                        Racey::Inconsistent => {
                            panic!("Inconsistent state exposed by the buffer!");
                        }
                    }
                    buf_input.write(RaceCell::new(new_value));
                }
            },
            move || {
                let mut last_value = 0usize;
                while last_value < TEST_WRITE_COUNT {
                    match buf_output.output_buffer().get() {
                        Racey::Consistent(new_value) => {
                            assert!((new_value >= last_value) && (new_value <= TEST_WRITE_COUNT));
                            last_value = new_value;
                        }
                        Racey::Inconsistent => {
                            panic!("Inconsistent state exposed by the buffer!");
                        }
                    }
                    if buf_output.updated() {
                        buf_output.output_buffer_mut().set(last_value / 2);
                        buf_output.update();
                    }
                }
            },
        );
    }

    /// Range check for triple buffer indexes
    #[allow(unused_comparisons)]
    fn index_in_range(idx: BufferIdx) -> bool {
        (0..=2).contains(&idx)
    }

    /// Get a pointer to the target of some reference (e.g. an &, an Arc...)
    fn as_ptr<P: Deref>(ref_like: &P) -> *const P::Target {
        &(**ref_like) as *const _
    }

    /// Check the state of a buffer, and the effect of queries on it
    fn check_buf_state<T>(buf: &mut TripleBuffer<T>, expected_dirty_bit: bool)
    where
        T: Clone + Debug + PartialEq + Send,
    {
        // Make a backup of the buffer's initial state
        let initial_buf = buf.clone();

        // Check that the input and output point to the same shared state
        assert_eq!(as_ptr(&buf.input.shared), as_ptr(&buf.output.shared));

        // Access the shared state and decode back-buffer information
        let back_info = buf.input.shared.status_bits.load(Ordering::Relaxed);
        let back_idx = back_info & STATUS_BACK_INDEX_MASK;
        let back_buffer_dirty = back_info & STATUS_DIRTY_BIT != 0;

        // Input-/output-/back-buffer indexes must be in range
        assert!(index_in_range(buf.input.input_idx));
        assert!(index_in_range(buf.output.output_idx));
        assert!(index_in_range(back_idx));

        // Input-/output-/back-buffer indexes must be distinct
        assert!(buf.input.input_idx != buf.output.output_idx);
        assert!(buf.input.input_idx != back_idx);
        assert!(buf.output.output_idx != back_idx);

        // Back-buffer must have the expected dirty bit
        assert_eq!(back_buffer_dirty, expected_dirty_bit);

        // Check that the "input buffer" query behaves as expected
        assert_eq!(
            as_ptr(&buf.input.input_buffer_mut()),
            buf.input.shared.buffers[buf.input.input_idx as usize].get()
        );
        assert_eq!(*buf, initial_buf);

        // Check that the "consumed" query behaves as expected
        assert_eq!(!buf.input.consumed(), expected_dirty_bit);
        assert_eq!(*buf, initial_buf);

        // Check that the output_buffer query works in the initial state
        assert_eq!(
            as_ptr(&buf.output.output_buffer()),
            buf.output.shared.buffers[buf.output.output_idx as usize].get()
        );
        assert_eq!(*buf, initial_buf);

        // Check that the output buffer query works in the initial state
        assert_eq!(buf.output.updated(), expected_dirty_bit);
        assert_eq!(*buf, initial_buf);
    }
}
