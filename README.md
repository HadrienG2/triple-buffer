# Triple buffering in Rust

## What is this?

This is an implementation of triple buffering written in Rust. You may find it
useful for the following class of thread synchronization problems:

- There is one producer thread and one consumer thread
- The producer wants to update a shared memory value frequently
- The consumer wants to access the latest update from the producer at any time

It is currently used as follows:

```rust
// Create a buffer of any Clone type
let buf = TripleBuffer::new("Hello");

// Split it into an input and output interface, which will
// be respectively sent to the producer and consumer thread
let (mut buf_input, mut buf_output) = buf.split();

// The producer can move a value into the buffer at any time
buf_input.write("world");

// The consumer can access the latest value from the producer at any time
let ref latest_value = buf_output.read();
```


## Give me details! How does it compare to alternatives?

Compared to a mutex:

- Only works in single-producer, single-consumer scenarios
- Is nonblocking, and more precisely bounded wait-free. Concurrent accesses will
  be slowed down by cache contention, but no deadlock, livelock, or thread
  scheduling induced slowdown is possible)
- Allows the producer and consumer to work simultaneously
- Uses a lot more memory (3x payload + 4 integers vs 1x payload + 1 bool)
- Does not allow in-place updates, new value must be cloned or moved
- Should be slower if updates are rare, faster if updates are frequent

Compared to the read-copy-update (RCU) primitive from the Linux kernel:

- Only works in single-producer, single-consumer scenarios
- Has higher readout overhead on relaxed-memory architectures (ARM, POWER...)
- Does not require accounting for reader "grace periods": once the reader has
  gotten access to the latest value, the synchronization transaction is over
- Does not use the slow compare-and-swap hardware primitive on update
- Does not suffer from the ABA problem, allowing much simpler code
- Allocates memory on initialization only, rather than on every update
- May use more memory (3x payload + 4 integers vs 1x pointer + amount of
  payloads and refcounts that depends on the readout and update pattern)
- Should be slower if updates are rare, faster if updates are frequent

Compared to sending the updates on a message queue:

- Only works in single-producer, single-consumer scenarios (queues can work in
  other scenarios, although the implementations are much less efficient)
- Consumer only has access to the latest state, not the previous ones
- Consumer does not *need* to get through every previous state
- Is nonblocking AND uses bounded amounts of memory (with queues, it's a choice)
- Can transmit information in a single move, rather than two
- Should be faster for any compatible use case

In short, triple buffering is what you're after in scenarios where a shared
memory location is updated frequently by a single writer, read by a single
reader who only wants the latest version, and you can spare some RAM.

- If you need multiple producers and consumers, look somewhere else
- If you can't tolerate the RAM overhead or want to update the data in place,
  try a Mutex instead (or possibly an RWLock)
- If the shared value is updated very rarely (e.g. every second), try an RCU
- If the consumer must get every update, try a message queue


## How do I know your unsafe lock-free code is working?

By running the tests, of course! Which is unfortunately currently harder than
I'd like it to be.

First of all, we have sequential tests, which are very thorough but obviously
do not check the lock-free/synchronization part. You run them as follows:

    $ cargo test --release

Then we have a concurrent test where a reader thread continuously observes the
values from a rate-limited writer thread, and makes sure that he can see every
single update without any incorrect value slipping in the middle.

This test is a lot more powerful, but it is also more tricky to run because the
following conditions must be met:

- The testing host must have at least two physical CPU cores
- No other application should be consuming CPU in the background. This includes
  other tests, so multi-threaded testing should be disabled.
- The proper writing rate is OS- and hardware-dependent, what is configured in
  this test may not be the proper value for your machine.

For these reasons, and because this tests also takes quite a lot of time to run
(about 10 seconds), this test is ignored by default.

Finally, we have benchmarks, which allow you to test how well the code is
performing on your machine. Because cargo bench has not yet landed in Stable
Rust, these benchmarks masquerade as tests, which make them a bit unpleasant to
run. I apologize for the inconvenience.

To run the concurrent test and the benchmarks, make sure that all your
background applications are closed or CPU-quiet, then do:

    $ cargo test --release -- --ignored --nocapture --test-threads=1

Here is how the benchmarking results should be interpreted:

* `clean_read` measures the triple buffer readout performance when the data does
  not change. It should be extremely fast (a couple of CPU clock cycles).
* `write` measures the amount of time it takes to write data in the triple
  buffer when no one is reading.
* `write_and_dirty_read` performs a write as before, immediately followed by a
  sequential read. To get the dirty read performance, substract the write time
  from that result. Writes and dirty read should take comparable time.
* `concurrent_write` measures the write performance when a reader is
  continuously reading. Expect significantly worse performance: lock-free
  techniques can help against contention, but are not a panacea.
* `concurrent_read` measures the read performance when a writer is continuously
  writing. The performance should be even worse than for writes, because the
  reading CPU core also needs to fetch the data from the writer's cache.

On my laptop's CPU (Intel Core i7-4720HQ), typical results are as follows:

* Write: 7.7 ns
* Clean read: 0.85 ns
* Dirty read: 7.2 ns
* Concurrent write: 45 ns
* Concurrent read: 88 ns
