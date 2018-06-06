extern crate pairlock;
use pairlock::{PairLock,TryUpdateError};

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering, ATOMIC_USIZE_INIT};
use std::ptr;

#[test]
fn basic() {
    let r = PairLock::new(1, 0);
    assert_eq!(r.view(|v| *v ), 1);
    assert_eq!(r.view(|v| *v ), 1);
    let mut updater = r.update();
    assert_eq!(*updater, 0);
    *updater = 2;
    drop(updater);
    assert_eq!(r.view(|v| *v ), 2);
    let mut updater = r.try_update().unwrap();
    assert_eq!(*updater, 1);
    *updater = 3;
    drop(updater);
    assert_eq!(r.view(|v| *v ), 3);
    let prev = r.set(4);
    assert_eq!(prev, 2);
}

#[test]
fn basic_clone() {
    let pl = PairLock::with_default(vec![1]);
    assert_eq!(pl.get_clone(), vec![1]);
    let default = pl.set(vec![2,3]);
    assert_eq!(default, Vec::default());
    assert_eq!(pl.get_clone(), vec![2,3]);
}
#[test]
fn basic_copy() {
    let pl = PairLock::with_default("one");
    assert_eq!(pl.read(), "one");
    pl.set("another");
    assert_eq!(pl.read(), "another");
}

#[test]
fn basic_arc() {
    let pl = PairLock::new_arc(0);
    assert_eq!(*pl.get(), 0);
    pl.set(Arc::new(1));
    assert_eq!(*pl.get(), 1);
}

#[test]
fn exclusive() {
    let mut pl = PairLock::new(1, 0);
    assert_eq!(*pl.get_mut_active(), 1);
    assert_eq!(*pl.get_mut_inactive(), 0);
    assert_eq!(pl.get_mut_both(), (&mut 1, &mut 0));
    *pl.update() = 2;
    assert_eq!(*pl.get_mut_active(), 2);
    assert_eq!(*pl.get_mut_inactive(), 1);
    assert_eq!(pl.get_mut_both(), (&mut 2, &mut 1));
    assert_eq!(pl.into_inner(), (2,1));
}

#[test]
fn singlethreaded_locking() {
    let r = PairLock::new((),());
    assert!(r.try_update().is_ok());
    r.view(|_| r.view(|_| assert!(r.try_update().is_ok()) ) );
    r.view(|_| {
        assert!(r.try_update().is_ok());
        assert_eq!(r.try_update(), Err(TryUpdateError::InactiveReads));
    });
    r.view(|_| {
        let _u = r.update();
        assert_eq!(r.try_update(), Err(TryUpdateError::OtherUpdate));
    });
}

#[test]
fn drop_runs() {
    static DROPS: AtomicUsize = ATOMIC_USIZE_INIT;

    struct Foo;

    impl Drop for Foo {
        fn drop(&mut self) {
            DROPS.fetch_add(1, Ordering::SeqCst);
        }
    }

    // ... once when both slots point to the same arc
    drop(PairLock::with_clone(Arc::new(Foo)));
    assert_eq!(DROPS.load(Ordering::SeqCst), 1);

    // ... twice when pointing to two different
    drop(PairLock::new(Arc::new(Foo), Arc::new(Foo)));
    assert_eq!(DROPS.load(Ordering::SeqCst), 3);

    // ... when the last reference drops
    let pl = PairLock::new_arc(Foo);
    let a = pl.get();
    pl.set(Arc::new(Foo));
    let b = pl.get();
    drop(pl);
    assert_eq!(DROPS.load(Ordering::SeqCst), 3);
    drop(a);
    drop(b);
    assert_eq!(DROPS.load(Ordering::SeqCst), 5);
}

#[test]
fn debug_fmt() {
    #[derive(Clone,Copy, Debug)]
    struct Foo{bar:&'static str}
    let pl = PairLock::new(Foo{bar:"baz"}, Foo{bar:"quux"});
    assert_eq!(format!("{:?}", pl), format!("PairLock({:?}, _)", pl.read()));
    assert_eq!(
        format!("{:#?}", pl),
        "PairLock(\n    Foo {\n        bar: \"baz\"\n    },\n    _\n)"
    );
}


#[test]
fn default() {
    assert_eq!(PairLock::<bool>::default().read(), bool::default());
}

#[test]
fn pointers() {
    let t1 = Arc::new(true);
    let t1_ptr = &*t1 as *const bool;
    let c = PairLock::with_clone(t1.clone());
    assert!(ptr::eq(&*c.get(), t1_ptr));
    assert!(ptr::eq(&*c.get(), t1_ptr));
    c.set(t1);
    let t2 = Arc::new(true);
    let t2_ptr = &*t2 as *const bool;
    assert!(!ptr::eq(t2_ptr, t1_ptr));
    assert!(ptr::eq(&*c.get(), t1_ptr));
    c.set(t2);
    assert!(ptr::eq(&*c.get(), t2_ptr));
}
