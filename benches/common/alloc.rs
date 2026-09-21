//! Optional global allocator overrides for benchmark binaries.
//!
//! Enable exactly one of the `bench-alloc-*` features to route the system
//! allocator through mimalloc, tcmalloc, or jemalloc for the whole suite.

#[cfg(feature = "bench-alloc-mimalloc")]
#[global_allocator]
static GLOBAL: mimalloc::MiMalloc = mimalloc::MiMalloc;

#[cfg(all(
    not(feature = "bench-alloc-mimalloc"),
    target_os = "linux",
    feature = "bench-alloc-tcmalloc"
))]
#[global_allocator]
static GLOBAL: tcmalloc_better::TCMalloc = tcmalloc_better::TCMalloc;

#[cfg(all(
    not(feature = "bench-alloc-mimalloc"),
    any(not(feature = "bench-alloc-tcmalloc"), not(target_os = "linux")),
    unix,
    feature = "bench-alloc-jemalloc"
))]
#[global_allocator]
static GLOBAL: tikv_jemallocator::Jemalloc = tikv_jemallocator::Jemalloc;
