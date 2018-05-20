extern crate arccell2;
use arccell2::ArcCell;

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering, ATOMIC_USIZE_INIT};

#[test]
fn basic() {
    let r = ArcCell::new(Arc::new(0));
    assert_eq!(*r.get(), 0);
    r.set(Arc::new(1));
    assert_eq!(*r.get(), 1);
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
    drop(ArcCell::new(Arc::new(Foo)));
    assert_eq!(DROPS.load(Ordering::SeqCst), 1);

    // ... twice when pointing to two different
    let c = ArcCell::new(Arc::new(Foo));
    c.set(Arc::new(Foo));
    assert_eq!(DROPS.load(Ordering::SeqCst), 1);
    drop(c);
    assert_eq!(DROPS.load(Ordering::SeqCst), 3);

    // ... when the last reference drops
    let c = ArcCell::new(Arc::new(Foo));
    let a = c.get();
    c.set(Arc::new(Foo));
    let b = c.get();
    drop(c);
    assert_eq!(DROPS.load(Ordering::SeqCst), 3);
    drop(a);
    drop(b);
    assert_eq!(DROPS.load(Ordering::SeqCst), 5);
}

#[test]
fn debug_fmt() {
    #[derive(Debug)]
    struct Foo{bar:()}
    let r = ArcCell::new(Arc::new(Foo{bar:()}));
    assert_eq!(format!("{:?}", r), format!("{:?}", *r.get()));
    assert_eq!(format!("{:#?}", r), format!("{:#?}", *r.get()));
}


#[test]
fn default() {
    assert_eq!(*ArcCell::<bool>::default().get(), bool::default());
}
