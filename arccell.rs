/* Copyright 2018 Torbjørn Birch Moltu
 *
 * Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
 * http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
 * http://opensource.org/licenses/MIT>, at your option. This file may not be
 * copied, modified, or distributed except according to those terms.
 */

use std::fmt::{self, Debug};
use std::cell::UnsafeCell;
use std::sync::{Arc, Mutex};
use std::sync::atomic::AtomicUsize;
use std::sync::atomic::Ordering::*;
use std::marker::PhantomData;
use std::mem::forget;

/// Permits thread-safe stores and wait-free clones of an `Arc<T>`.
pub struct ArcCell<T:?Sized> {
    /// Incremented by two at the start of each read.
    /// Least significant bit stores index of the active slot.
    /// Wraparound is OK; only equality with completed_reads matter
    reads_current: AtomicUsize,
    /// Incremented by two at the end of each read, after they have
    /// increased the reference count of the Arc.
    finished_reads: [AtomicUsize; 2],
    /// Non-null pointers returned by Arc::into_raw()
    /// Modified by updates while holding the lock and after there are no more
    /// reads in progress.
    arcs: [UnsafeCell<*const T>; 2],
    /// lock used for serializing writes, stores the final read count of the
    /// inactive slot
    prev_reads: Mutex<usize>,
    /// Will drap (two) Arc<T>s
    _contains: PhantomData<Arc<T>>
}

/// `T` must be `Sync` because there might still be references in the threads
/// it's sent from.
unsafe impl<T:?Sized+Send+Sync> Send for ArcCell<T> {}
unsafe impl<T:?Sized+Send+Sync> Sync for ArcCell<T> {}

impl<T:?Sized> Drop for ArcCell<T> {
    fn drop(&mut self) {
        unsafe {
            drop(Arc::from_raw(*self.arcs[0].get()));
            drop(Arc::from_raw(*self.arcs[1].get()));
        }
    }
}

impl<T:?Sized> ArcCell<T> {
    /// Creates a new `ArcCell`.
    pub fn new(init: Arc<T>) -> Self {
        // force wraparound in tests to ensure that it isn't a problem.
        let reads_start = if cfg!(debug_assertions) {!0<<2} else {0};
        ArcCell {
            reads_current: AtomicUsize::new(reads_start),
            finished_reads: [
                AtomicUsize::new(reads_start),
                AtomicUsize::new(reads_start+1)
            ],
            arcs: [
                UnsafeCell::new(Arc::into_raw(init.clone())),
                UnsafeCell::new(Arc::into_raw(init))
            ],
            prev_reads: Mutex::new(reads_start+1),
            _contains: PhantomData
        }
    }

    /// Stores a new value in the `ArcCell`.
    ///
    /// Will block if another thread is currently storing a new value or
    /// if there are reads of the second last value that haven't finished yet.  
    /// (that is, reads of the `Arc<T>` that was made outdated by the previous
    /// call to `set()`)
    pub fn set(&self,  arc: Arc<T>) {

    }

    /// Returns a clone of the stored `Arc<T>`.
    ///
    /// Will never block in any way, and should run in constant time.
    pub fn get(&self) -> Arc<T> {
        unsafe {
            let current = self.reads_current.fetch_add(2, Acquire);
            let slot = current & 1;
            let ptr = *self.arcs[slot].get();
            let arc = Arc::from_raw(ptr);
            // increase reference count
            forget(arc.clone());
            // mark read as complete
            self.finished_reads[slot].fetch_add(2, Release);
            arc
        }
    }
}

impl<T:?Sized+Debug> Debug for ArcCell<T> {
    /// Forwards to `T`'s debug implementation.
    fn fmt(&self,  fmtr: &mut fmt::Formatter) -> fmt::Result {
        unimplemented!()
    }
}

impl<T:?Sized+Default> Default for ArcCell<T> {
    fn default() -> Self {
        unimplemented!()
    }
}
