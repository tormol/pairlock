#![feature(test)]
extern crate test;
use test::{Bencher, black_box};
extern crate num_cpus;
extern crate pairlock;
use pairlock::PairLock;

use std::sync::Arc;
use std::sync::atomic::AtomicIsize;
use std::sync::atomic::Ordering::{SeqCst,Acquire};
use std::thread::spawn;
use std::env::var;
use std::str::FromStr;
use std::ptr;

fn other_threads() -> isize {
    var("BENCH_THREADS")
        .map(|env| usize::from_str(&*env).unwrap() )
        .unwrap_or_else(|_| num_cpus::get_physical() )
        as isize - 1
}

#[bench]
fn get_heavy_fat(b: &mut Bencher) {
    let arcs: Arc<[Arc<str>]> = Arc::from(vec![
        Arc::from("1"),
        Arc::from("22"),
        Arc::from("333"),
        Arc::from("4444"),
    ]);
    let c = Arc::new(PairLock::with_clone(arcs[0].clone()));
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
    let c = Arc::new(PairLock::with_clone(arcs[0].clone()));
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

#[bench]
fn get_big(b: &mut Bencher) {
    // try to detect races by checking for inconsistent values
    // every byte changes with each new value, making races obvoious
    let pattern = 0x01_01_01_01_01_01_01_01u64;
    let init = ([pattern<<7; 4], !0);
    let pl = Arc::new(PairLock::new(init, init));
    let s = pl.clone();
    spawn(move|| {
        for i in (0u64..10_000_000).rev() {
            let value = pattern << (i & 7);
            s.set(([value;4],i));
        }
    });
    b.iter(|| {
        let (arr,i) = pl.get_clone();
        let value = pattern << (i & 7);
        if arr != [value; 4] {
            panic!("race detected: {} gives pattern {:X}, but got\n[{:X},{:X},{:X},{:X}]",
                   i, value, arr[0], arr[1], arr[2], arr[3]);
        }
    });
}
