#![feature(test)]
#![allow(unused)]
extern crate test;
use test::{Bencher, black_box};
extern crate num_cpus;
extern crate crossbeam;
use crossbeam::sync::ArcCell;
extern crate pairlock;
use pairlock::PairLock;

use std::cell::Cell;
use std::sync::{Arc, RwLock};
use std::sync::atomic::AtomicIsize;
use std::sync::atomic::Ordering::*;
use std::thread::spawn;
use std::env::var;
use std::str::FromStr;
use std::{ptr,mem};

fn other_threads() -> isize {
    var("BENCH_THREADS")
        .map(|env| usize::from_str(&*env).unwrap() )
        .unwrap_or_else(|_| num_cpus::get_physical() )
        as isize - 1
}
#[derive(Clone,Copy, PartialEq)]
enum Do {Get,Set}
use Do::*;
#[derive(Clone,Copy, PartialEq)]
enum Other {Single, GetOnly, SetOnly, GetHeavy, SetHeavy, Mixed}
use Other::*;
#[derive(Clone,Copy)]
struct WorkLoad{bench: Option<Do>, other_getters: isize, other_setters: isize}
fn nw(bench: Do,  other_getters: isize,  other_setters: isize) -> WorkLoad {
    WorkLoad{ bench: Some(bench),  other_getters,  other_setters }
}
fn w(d: Do,  o: Other) -> WorkLoad {
    let min_other = match o {
        Single => 0,
        GetOnly | SetOnly => 1,
        GetHeavy => if d==Get {2} else {3},
        SetHeavy => if d==Set {2} else {3},
        Mixed => 4,
    };
    let other_threads = other_threads();
    if other_threads < min_other {
        return WorkLoad{ bench: None,  other_getters: 0,  other_setters: 0 };
    }
    match o {
        Single => nw(d, 0, 0),
        GetOnly => nw(d, other_threads, 0),
        SetOnly => nw(d, 0, other_threads),
        GetHeavy => nw(d, other_threads-1, 1),
        SetHeavy => nw(d, 1, other_threads-1),
        Mixed => {
            let setters = other_threads/2; // round down
            let getters = other_threads-setters; // favoring getters
            nw(d, getters, setters)
        }
    }
}

#[bench]
fn empty(_: &mut Bencher) {}
#[bench]
fn cell_get_single(b: &mut Bencher) {
    let c = Cell::new(0);
    b.iter(|| black_box(c.get()) );
}
#[bench]
fn cell_set_single(b: &mut Bencher) {
    let c = Cell::new(0);
    let mut i = 1;
    b.iter(|| {
        c.set(black_box(i));
        i += 1;
    });
}


fn arccell(bencher: &mut Bencher,  workload: WorkLoad) {
    let arc = Arc::new(AtomicIsize::new(0));
    let lock = Arc::new(ArcCell::new(arc.clone()));
    for _ in 0..workload.other_getters {
        let lock = lock.clone();
        spawn(move|| {
            lock.get().fetch_add(1, SeqCst);
            while lock.get().load(Acquire) != -1
                {}
        });
    }
    for _ in 0..workload.other_setters {
        let arc = arc.clone();
        let lock = lock.clone();
        spawn(move|| {
            arc.fetch_add(1, SeqCst);
            while arc.load(Acquire) != -1 {
                lock.set(arc.clone());
            }
        });
    }
    // wait untill threads have started
    while arc.load(Acquire) != workload.other_getters + workload.other_setters
        {}
    match workload.bench {
        Some(Get) => bencher.iter(|| lock.get() ),
        Some(Set) => bencher.iter(|| lock.set(arc.clone()) ),
        None => {}
    }
    arc.store(-1, Release);
}
#[bench]fn arccell_get_single(b: &mut Bencher) {arccell(b, w(Get, Single))}
#[bench]fn arccell_set_single(b: &mut Bencher) {arccell(b, w(Set, Single))}
#[bench]fn arccell_get_only(b: &mut Bencher) {arccell(b, w(Get, GetOnly))}
#[bench]fn arccell_set_only(b: &mut Bencher) {arccell(b, w(Set, SetOnly))}
#[bench]fn arccell_get_other_set(b: &mut Bencher) {arccell(b, w(Get, SetOnly))}
#[bench]fn arccell_set_other_get(b: &mut Bencher) {arccell(b, w(Set, GetOnly))}
#[bench]fn arccell_get_heavy(b: &mut Bencher) {arccell(b, w(Get, GetHeavy))}
#[bench]fn arccell_set_heavy(b: &mut Bencher) {arccell(b, w(Set, SetHeavy))}
#[bench]fn arccell_get_light(b: &mut Bencher) {arccell(b, w(Get, SetHeavy))}
#[bench]fn arccell_set_light(b: &mut Bencher) {arccell(b, w(Set, GetHeavy))}
#[bench]fn arccell_get_mixed(b: &mut Bencher) {arccell(b, w(Get, Mixed))}
#[bench]fn arccell_set_mixed(b: &mut Bencher) {arccell(b, w(Set, Mixed))}
#[bench]
fn arccell_set_single_alloc(b: &mut Bencher) {
    let lock = ArcCell::new(Arc::new(0));
    b.iter(|| lock.set(Arc::new(0)) );
}


fn pairlock(bencher: &mut Bencher,  workload: WorkLoad) {
    let arc = Arc::new(AtomicIsize::new(0));
    let lock = Arc::new(PairLock::new(arc.clone(), arc.clone()));
    for _ in 0..workload.other_getters {
        let lock = lock.clone();
        spawn(move|| {
            lock.get().fetch_add(1, SeqCst);
            while lock.get().load(Acquire) != -1
                {}
        });
    }
    for _ in 0..workload.other_setters {
        let arc = arc.clone();
        let lock = lock.clone();
        spawn(move|| {
            arc.fetch_add(1, SeqCst);
            while arc.load(Acquire) != -1 {
                lock.set(arc.clone());
            }
        });
    }
    // wait untill threads have started
    while arc.load(Acquire) != workload.other_getters + workload.other_setters
        {}
    match workload.bench {
        Some(Get) => bencher.iter(|| lock.get() ),
        Some(Set) => bencher.iter(|| lock.set(arc.clone()) ),
        None => {}
    }
    arc.store(-1, Release);
}
#[bench]fn pairlock_get_single(b: &mut Bencher) {pairlock(b, w(Get, Single))}
#[bench]fn pairlock_set_single(b: &mut Bencher) {pairlock(b, w(Set, Single))}
#[bench]fn pairlock_get_only(b: &mut Bencher) {pairlock(b, w(Get, GetOnly))}
#[bench]fn pairlock_set_only(b: &mut Bencher) {pairlock(b, w(Set, SetOnly))}
#[bench]fn pairlock_get_other_set(b: &mut Bencher) {pairlock(b, w(Get, SetOnly))}
#[bench]fn pairlock_set_other_get(b: &mut Bencher) {pairlock(b, w(Set, GetOnly))}
#[bench]fn pairlock_get_heavy(b: &mut Bencher) {pairlock(b, w(Get, GetHeavy))}
#[bench]fn pairlock_set_heavy(b: &mut Bencher) {pairlock(b, w(Set, SetHeavy))}
#[bench]fn pairlock_get_light(b: &mut Bencher) {pairlock(b, w(Get, SetHeavy))}
#[bench]fn pairlock_set_light(b: &mut Bencher) {pairlock(b, w(Set, GetHeavy))}
#[bench]fn pairlock_get_mixed(b: &mut Bencher) {pairlock(b, w(Get, Mixed))}
#[bench]fn pairlock_set_mixed(b: &mut Bencher) {pairlock(b, w(Set, Mixed))}
#[bench]
fn pairlock_set_single_alloc(b: &mut Bencher) {
    let lock = PairLock::new(Arc::new(0), Arc::new(0));
    b.iter(|| lock.set(Arc::new(0)) );
}


fn rwlock(bencher: &mut Bencher,  workload: WorkLoad) {
    let mut arc = Arc::new(AtomicIsize::new(0));
    let lock = Arc::new(RwLock::new(arc.clone()));
    for _ in 0..workload.other_getters {
        let lock = lock.clone();
        spawn(move|| {
            lock.read().unwrap().fetch_add(1, SeqCst);
            while lock.read().unwrap().load(Acquire) != -1
                {}
        });
    }
    for _ in 0..workload.other_setters {
        let mut arc = arc.clone();
        let lock = lock.clone();
        spawn(move|| {
            arc.fetch_add(1, SeqCst);
            while arc.load(Acquire) != -1 {
                mem::swap(&mut arc, &mut*lock.write().unwrap());
            }
        });
    }
    // wait untill threads have started
    while arc.load(Acquire) != workload.other_getters + workload.other_setters
        {}
    match workload.bench {
        Some(Get) => bencher.iter(|| assert!(ptr::eq(&**lock.read().unwrap(), &*arc)) ),
        Some(Set) => bencher.iter(|| mem::swap(&mut arc, &mut*lock.write().unwrap()) ),
        None => {}
    }
    arc.store(-1, Release);
}
#[bench]fn rwlock_get_single(b: &mut Bencher) {rwlock(b, w(Get, Single))}
#[bench]fn rwlock_set_single(b: &mut Bencher) {rwlock(b, w(Set, Single))}
#[bench]fn rwlock_get_only(b: &mut Bencher) {rwlock(b, w(Get, GetOnly))}
#[bench]fn rwlock_set_only(b: &mut Bencher) {rwlock(b, w(Set, SetOnly))}
#[bench]fn rwlock_get_other_set(b: &mut Bencher) {rwlock(b, w(Get, SetOnly))}
#[bench]fn rwlock_set_other_get(b: &mut Bencher) {rwlock(b, w(Set, GetOnly))}
#[bench]fn rwlock_get_heavy(b: &mut Bencher) {rwlock(b, w(Get, GetHeavy))}
#[bench]fn rwlock_set_heavy(b: &mut Bencher) {rwlock(b, w(Set, SetHeavy))}
#[bench]fn rwlock_get_light(b: &mut Bencher) {rwlock(b, w(Get, SetHeavy))}
#[bench]fn rwlock_set_light(b: &mut Bencher) {rwlock(b, w(Set, GetHeavy))}
#[bench]fn rwlock_get_mixed(b: &mut Bencher) {rwlock(b, w(Get, Mixed))}
#[bench]fn rwlock_set_mixed(b: &mut Bencher) {rwlock(b, w(Set, Mixed))}
#[bench]
fn rwlock_set_single_alloc(b: &mut Bencher) {
    let lock = RwLock::new(Arc::new(0));
    b.iter(|| *lock.write().unwrap() = Arc::new(0) );
}


#[bench]
fn get_heavy_fat(b: &mut Bencher) {
    let arcs: Arc<[Arc<str>]> = Arc::from(vec![
        Arc::from("1"),
        Arc::from("22"),
        Arc::from("333"),
        Arc::from("4444"),
    ]);
    let c = Arc::new(PairLock::new(arcs[0].clone(), arcs[0].clone()));
    let state = Arc::new(AtomicIsize::new(0));
    let mut threads = Vec::new();
    for _ in 0..other_threads()-1 {
        let (state, c, arcs) = (state.clone(), c.clone(), arcs.clone());
        threads.push(spawn(move|| {
            state.fetch_add(1, SeqCst);
            while state.load(Acquire) != -1 {
                let s = c.get();
                if !arcs.iter().any(|a| &**a == &*s ) {
                    panic!("Got unexpected string {:?}", s);
                }
            }
        }));
    }
    {
        let (state, c, arcs) = (state.clone(), c.clone(), arcs.clone());
        spawn(move|| {
            state.fetch_add(1, SeqCst);
            let mut i = 0;
            while state.load(Acquire) != -1 {
                c.set(arcs[i%arcs.len()].clone());
                i += 1;
            }
        });
    }
    {
        while state.load(Acquire) != other_threads()
            {}
        b.iter(|| {
            let s = c.get();
            if !arcs.iter().any(|a| &**a == &*s ) {
                panic!("Got unexpected string {:?}", s);
            }
        });
        state.store(-1, SeqCst);
    }
    for t in threads {
        t.join().unwrap();
    }
}

#[bench]
fn set_heavy_fat(b: &mut Bencher) {
    let arcs: [Arc<str>;4] = [
        Arc::from("1"),
        Arc::from("22"),
        Arc::from("333"),
        Arc::from("4444"),
    ];
    let c = Arc::new(PairLock::new(arcs[0].clone(), arcs[0].clone()));
    let state = Arc::new(AtomicIsize::new(0));
    for i in 1..other_threads() as usize {
        let (s, c, a) = (state.clone(), c.clone(), arcs[i].clone());
        spawn(move|| {
            s.fetch_add(1, SeqCst);
            while s.load(Acquire) != -1 {
                c.set(a.clone());
            }
        });
    }
    let getter = {
        let (state, c, arcs) = (state.clone(), c.clone(), arcs.clone());
        spawn(move|| {
            state.fetch_add(1, SeqCst);
            while state.load(Acquire) != -1 {
                let s = c.get();
                if !arcs.iter().any(|a| &**a == &*s  &&  ptr::eq(&**a, &*s) ) {
                    panic!("Got unexpected string {:?}", s);
                }
            }
        })
    };
    {
        while state.load(Acquire) != other_threads()
            {}
        b.iter(|| c.set(arcs[0].clone()) );
        state.store(-1, SeqCst);
    }
    getter.join().unwrap();
}
