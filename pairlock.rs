/* Copyright 2018 Torbjørn Birch Moltu
 *
 * Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
 * http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
 * http://opensource.org/licenses/MIT>, at your option. This file may not be
 * copied, modified, or distributed except according to those terms.
 */

use std::cell::UnsafeCell;
use std::sync::{Mutex,MutexGuard,TryLockError, Arc};
use std::sync::atomic::{AtomicUsize,fence, spin_loop_hint};
use std::sync::atomic::Ordering::{SeqCst,Relaxed};
use std::thread::yield_now;
use std::{ptr, mem};
use std::ops::{Deref,DerefMut};
use std::fmt::{self, Debug,Display};
use std::error::Error;

const MAX_UPDATE_SPINS: usize = 7; // not benchmarked

/// A reader-writer lock with wait-free reads.
///
/// Does not have poisoning.
///
/// # Examples
///
/// ```no_run
/// # use pairlock::PairLock;
/// # use std::sync::Arc;
/// # use std::thread;
/// # use std::time::Duration;
/// # fn load_config() -> String {String::new()}
/// # fn use_config(_: Arc<String>) {}
/// let ac = Arc::new(PairLock::new_arc(load_config()));
/// let ac2 = ac.clone();
/// thread::spawn(move|| {
///     loop {
///         use_config(ac2.get());;
///     }
/// });
/// loop {
///     thread::sleep(Duration::from_secs(60));
///     ac.set(Arc::new(load_config()));
/// }
/// ```
pub struct PairLock<T> {
    /// Incremented by two at the start of each view.
    /// Least significant bit stores index of the active slot.
    /// Wraparound is OK; only equality with completed_reads matter
    reads_active: AtomicUsize,
    /// Incremented by two at the end of each view, after they have
    /// increased the reference count of the Arc.
    finished_reads: [AtomicUsize; 2],
    /// Modified by updates while holding the lock and after there are no more
    /// reads in progress.
    values: [UnsafeCell<T>; 2],
    /// lock used for serializing writes, stores the final read count of the
    /// inactive slot
    inactive_reads: Mutex<usize>,
}

unsafe impl<T:Send> Send for PairLock<T> {}
/// `T` must be `Send` because a shared reference can replace stored values.
unsafe impl<T:Send+Sync> Sync for PairLock<T> {}

impl<T> PairLock<T> {
    /// Creates a new `PairLock`.
    pub fn new(active: T,  inactive: T) -> Self {
        PairLock {
            reads_active: AtomicUsize::new(0),
            // Initializing the second slot to !0 causes wraparound to be
            // reached in tests, ensuring that it doesn't cause problems.
            // Should be just as fast as initializing to 1.
            finished_reads: [AtomicUsize::new(0), AtomicUsize::new(!0)],
            values: [UnsafeCell::new(active), UnsafeCell::new(inactive)],
            inactive_reads: Mutex::new(!0),
        }
    }
    /// Creates a new `PairLock` with `init` as the active value
    /// and `T`'s default value as the inactive.
    pub fn with_default(init: T) -> Self where T: Default {
        Self::new(init, T::default())
    }
    /// Creates a new `PairLock` with `init` as the active value
    /// and its `.clone()` as the inactive.
    pub fn with_clone(init: T) -> Self where T: Clone {
        let second = init.clone();
        Self::new(init, second)
    }


    /// View the active value of this `PairLock` inside a closure.
    ///
    /// Views should be short-lived to avoid blocking subsequent updates.
    ///
    /// Reads must be performed inside a closure, because preventing memory
    /// unsafety in the face of repeated `mem::forget()`s of a read guard is
    /// non-trivial.
    ///
    /// Will never block in any way, and should run in constant time.
    pub fn view<F:FnOnce(&T)->R,R>(&self,  viewer: F) -> R {
        unsafe {
            // Acquire/Release doesn't work across different variables
            let active = self.reads_active.fetch_add(2, SeqCst);
            let slot = active & 1;
            // not releasing on unwind could cause use-after-free
            struct Releaser<'a>(&'a AtomicUsize);
            impl<'a> Drop for Releaser<'a> {
                fn drop(&mut self) {
                    // reads and release must not mix
                    fence(SeqCst);
                    // mark read as complete
                    self.0.fetch_add(2, Relaxed);
                }
            }
            let _defer = Releaser(&self.finished_reads[slot]);
            viewer(&*self.values[slot].get())
        }
    }
    /// Returns a clone of the active value.
    ///
    /// Will never block in any way, and should run in constant time.
    pub fn get_clone(&self) -> T where T: Clone {
        self.view(|v| v.clone() )
    }


    /// Creates an UpdateGuard if there are no unfinished reads of the inactive
    /// value.
    ///
    /// # Safety
    /// The mutex guard must be for the mutex in self.
    unsafe fn check_inactive<'a>(&'a self,  inactive_reads: MutexGuard<'a,usize>)
    -> Result<UpdateGuard<'a,T>, MutexGuard<'a,usize>> {
        let slot = *inactive_reads & 1;
        // make sure that all views of the previous value has finished
        if self.finished_reads[slot].load(Relaxed) == *inactive_reads {
            fence(SeqCst);
            Ok(UpdateGuard{ guard: inactive_reads,  pl: self })
        } else {
            Err(inactive_reads)
        }
    }

    /// Locks the inactive value, giving exclusive access to it through
    /// a RAII guard that will make it active when the guard is dropped.
    /// 
    /// Will block the thread waiting for reads of the inactive value or other
    /// updates to finish.
    ///
    /// Panicing while holding the guard does not poison the lock.
    ///
    /// # Examples
    /// Using the lock as a counter
    /// ```
    /// # use pairlock::{PairLock,UpdateGuard};
    /// let counter = PairLock::with_default(1);
    /// let mut guard = counter.update();
    /// *guard = UpdateGuard::active(&guard) + 1;
    /// drop(guard);
    /// assert_eq!(counter.read(), 2);
    /// ```
    ///
    /// Reusing an allocation while updating
    /// ```
    /// # use pairlock::{PairLock,UpdateGuard};
    /// let lock = PairLock::with_default(vec!["foo","bar"]);
    /// let mut guard = lock.update();
    /// {
    ///     let (mutable, active) = UpdateGuard::both(&mut guard);
    ///     mutable.clone_from(active);
    ///     mutable.push("baz");
    /// }
    /// drop(guard);
    /// lock.view(|v| assert_eq!(v[..], ["foo","bar","baz"][..]) );
    /// ```
    ///
    /// Doing nothing with the guard, and still changing the value of the lock:
    /// ```
    /// # use pairlock::PairLock;
    /// let lock = PairLock::new("foo", "bar");
    /// assert_eq!(lock.read(), "foo");
    /// let _ = lock.update();
    /// assert_eq!(lock.read(), "bar");
    /// ```
    pub fn update(&self) -> UpdateGuard<T> {
        loop {
            unsafe {
                let mut inactive_reads = self.inactive_reads.lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner() );
                for _ in 0..MAX_UPDATE_SPINS {
                    inactive_reads = match self.check_inactive(inactive_reads) {
                        Ok(success) => return success,
                        Err(retry) => retry
                    };
                    spin_loop_hint();
                }
                // release lock before yielding
                drop(inactive_reads);
            }
            yield_now();
        }
    }

    /// Attempts to lock the inactive value, giving exclusive access to it
    /// through a RAII guard that will make it active when the guard is dropped.
    ///
    /// # Errors
    /// Returns an error instead of blocking the thread.  
    /// The error tells which phase of aquiring the update lock that failed.
    ///
    /// # Examples
    /// ```
    /// # use pairlock::{PairLock,TryUpdateError};
    /// let pl = PairLock::new(String::new(), String::new());
    /// let _guard = pl.try_update().unwrap();
    /// assert_eq!(pl.try_update(), Err(TryUpdateError::OtherUpdate));
    /// ```
    pub fn try_update(&self) -> Result<UpdateGuard<T>,TryUpdateError> {
        unsafe {
            let guard = match self.inactive_reads.try_lock() {
                Ok(guard) => guard,
                Err(TryLockError::Poisoned(poisoned)) => poisoned.into_inner(),
                Err(TryLockError::WouldBlock) => return Err(TryUpdateError::OtherUpdate)
            };
            self.check_inactive(guard).map_err(|_| TryUpdateError::InactiveReads )
        }
    }

    /// Stores a new value in the `PairLock`,
    /// returning the previously inactive value.
    ///
    /// Will block if another update/replace/set is in progress.
    /// if there are reads of the second last value that haven't finished yet.  
    pub fn set(&self,  value: T) -> T {
        mem::replace(&mut*self.update(), value)
    }


    /// Consumes the `PairLock` and returns the active and inactive values.
    ///
    /// # Examples
    /// ```
    /// # use pairlock::PairLock;
    /// let lock = PairLock::new(true, false);
    /// let (active, inactive) = lock.into_inner();
    /// ```
    pub fn into_inner(self) -> (T, T) {
        // yay no custom drop impl
        let PairLock{ reads_active, values, .. } = self;
        let active = reads_active.into_inner() & 1;
        // but cannot destructure fixed-size arrays :(
        unsafe {
            let active_ = ptr::read(values[active].get());
            let inactive_ = ptr::read(values[active^1].get());
            mem::forget(values);
            (active_, inactive_)
        }
    }

    /// Given exclusive access this method returns mutable references to both
    /// the active and inactive value.
    ///
    /// # Examples
    /// ```
    /// # use pairlock::PairLock;
    /// let mut lock = PairLock::new(true, false);
    /// let (&mut active, &mut inactive) = lock.get_mut_both();
    /// ```
    pub fn get_mut_both(&mut self) -> (&mut T, &mut T) {
        let active = *self.reads_active.get_mut() & 1;
        unsafe {// safe because &mut self
            (&mut*self.values[active].get(), &mut*self.values[active^1].get())
        }
    }
    /// Given exclusive access this method returns a mutable reference to
    /// the active value.
    pub fn get_mut_active(&mut self) -> &mut T {
        self.get_mut_both().0
    }
    /// Given exclusive access this method returns a mutable reference to
    /// the inactive value.
    pub fn get_mut_inactive(&mut self) -> &mut T {
        self.get_mut_both().1
    }
}
impl<T> PairLock<Arc<T>> {
    /// Puts `value` into an `Arc<T>` and creates a new `PairLock<Arc<T>>`
    /// with it.
    pub fn new_arc(value: T) -> Self {
        PairLock::with_clone(Arc::new(value))
    }
}
impl<T:?Sized> PairLock<Arc<T>> {
    /// Returns a clone of the active `Arc<T>`.
    ///
    /// Will never block in any way, and should run in constant time.
    pub fn get(&self) -> Arc<T> {
        self.get_clone()
    }
}
impl<T:Copy> PairLock<T> {
    /// Returns a copy of the active value.
    ///
    /// Will never block in any way, and should run in constant time.
    pub fn read(&self) -> T {
        self.get_clone()
    }
}

impl<T:Debug> Debug for PairLock<T> {
    fn fmt(&self,  fmtr: &mut fmt::Formatter) -> fmt::Result {
        struct Hidden;
        impl Debug for Hidden {
            fn fmt(&self,  fmtr: &mut fmt::Formatter) -> fmt::Result {
                fmtr.write_str("_")
            }
        }
        self.view(|v| {
            fmtr.debug_tuple("PairLock")
            .field(v)
            .field(&Hidden)
            .finish()
        })
    }
}

impl<T:Default> Default for PairLock<T> {
    fn default() -> Self {
        Self::new(T::default(), T::default())
    }
}

impl<T:Clone> Clone for PairLock<T> {
    /// Returns a new `PairLock` initialized with the current `Arc` in `self`.
    ///
    /// Does not clone the content of the `Arc`.
    fn clone(&self) -> Self {
        Self::new(self.get_clone(), self.get_clone())
    }
    fn clone_from(&mut self,  source: &Self) {
        let (a,b) = unsafe {
            // OK because exclusive access
            (&mut*self.values[0].get(), &mut*self.values[1].get())
        };
        // lock source for as short as possible
        source.view(|init| a.clone_from(init) );
        b.clone_from(&*a);
    }
}


/// A RAII guard providing mutable access to the inactive value of a `PairLock`,
/// The values becomes active when the guard is dropped.
pub struct UpdateGuard<'a, T:'a> {
    guard: MutexGuard<'a, usize>,
    pl: &'a PairLock<T>,
}
impl<'a,T> Drop for UpdateGuard<'a,T> {
    /// Makes the value active and releases the update lock
    fn drop(&mut self) {
        let inactive_reads = *self.guard;
        fence(SeqCst);
        // makes the new value active
        let active_reads = self.pl.reads_active.swap(inactive_reads, SeqCst);
        *self.guard = active_reads;
        // and the mutex guard is dropped by the compiler
    }
}
// I assume these methods are not called many times per instance,
// and have therefero optimized for struct size.
impl<'a,T> Deref for UpdateGuard<'a,T> {
    type Target = T;
    fn deref(&self) -> &T {
        unsafe {
            let slot = *self.guard & 1;
            &*self.pl.values[slot].get()
        }
    }
}
impl<'a,T> DerefMut for UpdateGuard<'a,T> {
    fn deref_mut(&mut self) -> &mut T {
        unsafe {
            let slot = *self.guard & 1;
            &mut*self.pl.values[slot].get()
        }
    }
}
impl<'a,T> UpdateGuard<'a,T> {
    /// Returns a shared reference to the active value of the `PairLock`.
    ///
    /// It can not be mutate it while the `PairLock` is locked, and is therefore
    /// safe to read.
    pub fn active(this: &Self) -> &T {
        unsafe {
            let other_slot = (!*this.guard) & 1;
            &*this.pl.values[other_slot].get()
        }
    }
    /// Returns references to both the inactive (mutable) and active value of
    /// the `PairLock`.
    pub fn both(this: &mut Self) -> (&mut T, &T) {
        unsafe {
            let slot = *this.guard & 1;
            let values = &this.pl.values;
            (&mut*values[slot].get(), &*values[slot^1].get())
        }
    }
    /// Aborts the update by releasing the lock without making the mutable value
    /// active.
    /// 
    /// Any changes made to the inactive value will however be visible to the
    /// next `.update()` or `.replace()`.
    pub fn cancel(this: Self) {
        // unlock the mutex without changing reads_active or inactive_reads 
        unsafe {
            // forget self first in case the MutexGuard drop impl can unwind
            let guard = ptr::read(&this.guard);
            mem::forget(this);
            drop(guard);
        }
    }
}
impl<'a, T:Debug> Debug for UpdateGuard<'a,T> {
    fn fmt(&self,  fmtr: &mut fmt::Formatter) -> fmt::Result {
        fmtr.debug_struct("UpdateGuard")
            .field("mutable", &*self)
            .field("active", &*UpdateGuard::active(self))
            .finish()
    }
}
impl<'a,T> PartialEq for UpdateGuard<'a,T> {
    /// Convenience impl for comparing `Result`s containing this type.
    ///
    /// Only compares equal against itself (`ptr::eq(self, other)`).
    fn eq(&self,  other: &Self) -> bool {
        ptr::eq(self, other)
    }
}


/// Error returned when a `PairLock.try_update()` fails,
/// because it would otherwise have blocked.
#[derive(Clone,Copy, PartialEq,Eq)]
pub enum TryUpdateError {
    /// Was locked by another update.  
    OtherUpdate,
    /// There were unfinished reads of the inactive value.
    InactiveReads,
}
impl Error for TryUpdateError {
    fn description(&self) -> &'static str {
        match *self {
            TryUpdateError::OtherUpdate => "locked by another update",
            TryUpdateError::InactiveReads => "unfinished reads of the inactive value",
        }
    }
}
impl Display for TryUpdateError {
    fn fmt(&self,  fmtr: &mut fmt::Formatter) -> fmt::Result {
        fmtr.write_str(self.description())
    }
}
impl Debug for TryUpdateError {
    fn fmt(&self,  fmtr: &mut fmt::Formatter) -> fmt::Result {
        Display::fmt(self, fmtr)
    }
}
