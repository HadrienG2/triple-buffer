# Triple buffering in Rust

## What is this?

This is an implementation of triple buffering written in Rust. You may find it
useful for the following class of thread synchronization problems:

- There is one producer thread and one consumer thread
- The producer wants to update a shared memory value frequently
- The consumer wants to access the latest update from the producer


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
