//! Buffer-acquisition strategies for the workload benchmarks.
//!
//! Every strategy exposes the same interface — `get(len)` returns an RAII
//! guard that dereferences to a writable `[u8]` of exactly `len` readable
//! bytes — so a single generic workload drives all of them. The guards are
//! `Send` because the pipeline frees buffers on a different thread than
//! the one that acquired them.

#![allow(dead_code)]

use std::cell::RefCell;
use std::ops::{Deref, DerefMut};

use crate::common::config;

/// A source of writable byte buffers.
pub trait BufStrategy: Sync {
    /// RAII buffer guard. `Send` so buffers can cross threads.
    type Buf<'a>: DerefMut<Target = [u8]> + Send
    where
        Self: 'a;

    /// Strategy name used in benchmark IDs.
    fn name() -> &'static str;

    /// Buffer of exactly `len` readable bytes (contents unspecified but
    /// initialized).
    fn get(&self, len: usize) -> Self::Buf<'_>;
}

// ── zeropool ───────────────────────────────────────────────────────────

/// ZeroPool-backed strategy.
pub struct ZeroPoolAlloc(pub zeropool::ZeroPool);

impl ZeroPoolAlloc {
    /// Build the shared benchmark pool.
    pub fn new() -> Self {
        Self(config::zeropool())
    }
}

impl BufStrategy for ZeroPoolAlloc {
    type Buf<'a> = zeropool::Buf<'a>;

    fn name() -> &'static str {
        "zeropool"
    }

    fn get(&self, len: usize) -> Self::Buf<'_> {
        self.0.alloc(len)
    }
}

// ── vec_fresh ──────────────────────────────────────────────────────────

/// Fresh `vec![0; len]` per call — every get pays the kernel.
pub struct VecFresh;

impl BufStrategy for VecFresh {
    type Buf<'a> = Vec<u8>;

    fn name() -> &'static str {
        "vec_fresh"
    }

    fn get(&self, len: usize) -> Self::Buf<'_> {
        vec![0u8; len]
    }
}

// ── vec_reuse ──────────────────────────────────────────────────────────
//
// Per-thread free list. When a buffer is dropped on a different thread
// than the one that acquired it, it lands in the *consumer's* stash, so
// the producer keeps allocating — this models thread-local reuse breaking
// down under cross-thread free.

thread_local! {
    static STASH: RefCell<Vec<Vec<u8>>> = const { RefCell::new(Vec::new()) };
}

/// RAII guard that returns its `Vec` to the current thread's stash on drop.
pub struct ReusedVec(Vec<u8>);

impl Deref for ReusedVec {
    type Target = [u8];

    fn deref(&self) -> &[u8] {
        &self.0
    }
}

impl DerefMut for ReusedVec {
    fn deref_mut(&mut self) -> &mut [u8] {
        &mut self.0
    }
}

impl Drop for ReusedVec {
    fn drop(&mut self) {
        let vec = std::mem::take(&mut self.0);
        STASH.with(|stash| {
            let mut stash = stash.borrow_mut();
            if stash.len() < config::REUSE_STASH_CAP {
                stash.push(vec);
            }
        });
    }
}

/// Per-thread buffer reuse via a capped free list.
pub struct VecReuse;

impl BufStrategy for VecReuse {
    type Buf<'a> = ReusedVec;

    fn name() -> &'static str {
        "vec_reuse"
    }

    fn get(&self, len: usize) -> Self::Buf<'_> {
        let mut vec = STASH.with(|stash| stash.borrow_mut().pop()).unwrap_or_default();
        vec.clear();
        vec.resize(len, 0);
        ReusedVec(vec)
    }
}

// ── bytes ──────────────────────────────────────────────────────────────

/// Fresh `BytesMut` per call.
#[cfg(feature = "bench")]
pub struct BytesMutFresh;

#[cfg(feature = "bench")]
impl BufStrategy for BytesMutFresh {
    type Buf<'a> = bytes::BytesMut;

    fn name() -> &'static str {
        "bytes"
    }

    fn get(&self, len: usize) -> Self::Buf<'_> {
        let mut buf = bytes::BytesMut::with_capacity(len);
        buf.resize(len, 0);
        buf
    }
}

// ── opool / object_pool ────────────────────────────────────────────────
//
// One pool per power-of-two size class, matching ZeroPool's layout.

#[cfg(feature = "bench")]
use std::sync::Arc;

/// Guard over an opool `RcGuard`, exposing `&mut buf[..len]`.
#[cfg(feature = "bench")]
pub struct OpoolBuf {
    guard: opool::RcGuard<crate::common::competitors::ZeroedVecAlloc, Vec<u8>>,
    len: usize,
}

#[cfg(feature = "bench")]
impl Deref for OpoolBuf {
    type Target = [u8];

    fn deref(&self) -> &[u8] {
        &self.guard[..self.len]
    }
}

#[cfg(feature = "bench")]
impl DerefMut for OpoolBuf {
    fn deref_mut(&mut self) -> &mut [u8] {
        &mut self.guard[..self.len]
    }
}

/// opool with one pool per size class.
#[cfg(feature = "bench")]
pub struct OpoolClasses {
    pools: Vec<Arc<opool::Pool<crate::common::competitors::ZeroedVecAlloc, Vec<u8>>>>,
}

#[cfg(feature = "bench")]
impl OpoolClasses {
    /// Build one pool per class, each capped at `POOL_CAP_PER_CLASS`.
    pub fn new() -> Self {
        let pools = (0..config::NUM_CLASSES)
            .map(|i| {
                Arc::new(opool::Pool::new(
                    config::POOL_CAP_PER_CLASS,
                    crate::common::competitors::ZeroedVecAlloc(config::class_size(i)),
                ))
            })
            .collect();
        Self { pools }
    }
}

#[cfg(feature = "bench")]
impl BufStrategy for OpoolClasses {
    type Buf<'a> = OpoolBuf;

    fn name() -> &'static str {
        "opool"
    }

    fn get(&self, len: usize) -> Self::Buf<'_> {
        let mut guard = self.pools[config::class_index(len)].clone().get_rc();
        guard.resize(len, 0);
        OpoolBuf { guard, len }
    }
}

/// Guard over an object_pool `ReusableOwned`, exposing `&mut buf[..len]`.
#[cfg(feature = "bench")]
pub struct ObjectPoolBuf {
    guard: object_pool::ReusableOwned<Vec<u8>>,
    len: usize,
}

#[cfg(feature = "bench")]
impl Deref for ObjectPoolBuf {
    type Target = [u8];

    fn deref(&self) -> &[u8] {
        &self.guard[..self.len]
    }
}

#[cfg(feature = "bench")]
impl DerefMut for ObjectPoolBuf {
    fn deref_mut(&mut self) -> &mut [u8] {
        &mut self.guard[..self.len]
    }
}

/// object_pool with one pool per size class.
#[cfg(feature = "bench")]
pub struct ObjectPoolClasses {
    pools: Vec<Arc<object_pool::Pool<Vec<u8>>>>,
}

#[cfg(feature = "bench")]
impl ObjectPoolClasses {
    /// Build one pool per class, each capped at `POOL_CAP_PER_CLASS`.
    pub fn new() -> Self {
        let pools = (0..config::NUM_CLASSES)
            .map(|i| {
                Arc::new(object_pool::Pool::new(config::POOL_CAP_PER_CLASS, move || {
                    vec![0u8; config::class_size(i)]
                }))
            })
            .collect();
        Self { pools }
    }
}

#[cfg(feature = "bench")]
impl BufStrategy for ObjectPoolClasses {
    type Buf<'a> = ObjectPoolBuf;

    fn name() -> &'static str {
        "object_pool"
    }

    fn get(&self, len: usize) -> Self::Buf<'_> {
        let i = config::class_index(len);
        let mut guard = self.pools[i].pull_owned(|| vec![0u8; config::class_size(i)]);
        guard.resize(len, 0);
        ObjectPoolBuf { guard, len }
    }
}
