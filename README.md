# PairLock

A reader-writer lock for scenarios with frequent reads and infrequent writes.

Reads are wait-free, and writes are not starved by reads thet started after the
previous write. Writes block each other.

This is accomplished by storing two values of `T` and marking one of them as
active: Reads see the active value, while writes mutate the inactive one before
switching the active status.

I'm no expert on lock-free programming, and I've only tested on x86_64,
however the code makes liberal use of `fence(SeqCst)`.

It can be used with any (sized) type, but wrapping them in a `Box` or `Arc`
might improve performance by reducing false sharing.

Does not have poisoning.

## Implementation details

`PairLock` is a variation of differential reference counting that doesn't
require double-word atomics:

One `AtomicUsize` stores the index of the active slot and the all-time number of
started reads of that slot. A slot-specific `AtomicUsize` stores the all-time
number of finished reads of that slot. The current read count is the difference
between those variables. Wrap-around is OK as long as there are no more than
`usize::MAX/2` current reads. The number of started reads of the inactive slot
is stored in a mutex only used used by writes.

Reads start by incrementing the first variable, and finish by incrementing the
second one.
Writes start by locking the mutex and waiting untill all reads of the inactive
slot have finished, and finish by swapping the value of the first variable with
the one in the mutex.

The algorithm is similar to left-right locking, but simpler and not as
efficient: Left-right reduces sharing by having multiple counters, and
readers only needing to modify one of them.


## License

Licensed under either of

 * Apache License, Version 2.0, ([LICENSE-APACHE](LICENSE-APACHE) or http://www.apache.org/licenses/LICENSE-2.0)
 * MIT license ([LICENSE-MIT](LICENSE-MIT) or http://opensource.org/licenses/MIT)

at your option.

### Contribution

Unless you explicitly state otherwise, any contribution intentionally
submitted for inclusion in the work by you, as defined in the Apache-2.0
license, shall be dual licensed as above, without any additional terms or
conditions.
