#!/usr/bin/env bash
# Run the full ZeroPool benchmark suite with the comparison crates enabled.
#
# Usage:
#   scripts/bench-full.sh
#   ZEROPOOL_SCALE_OPS=100000 scripts/bench-full.sh
#
# Each group is run separately so the output stays readable. Groups that
# require the `bench` feature are flagged. After the micro groups it runs
# the workload benches (pipeline, tcp, file_hash, framing, compress, tiles)
# and emits target/criterion-tables.md via scripts/bench.py.

set -euo pipefail

cd "$(dirname "$0")/.."

# 1. Hot path: requires bench feature for opool/object_pool/bytes.
cargo bench --features bench -- hot_path

# 2. Realistic single-thread and multi-thread: requires bench feature.
cargo bench --features bench -- realistic_write_st
cargo bench --features bench -- realistic_write_mt

# 3. Sustained throughput: requires bench feature.
ZEROPOOL_SCALE_OPS="${ZEROPOOL_SCALE_OPS:-10000}" \
    cargo bench --features bench -- scale_sustained

# 4. Mixed sizes: requires bench feature.
cargo bench --features bench -- mixed_uniform
cargo bench --features bench -- mixed_zipf

# 5. Burst: requires bench feature.
cargo bench --features bench -- burst

# 6. Contention: ZeroPool only, no comparison crates needed.
cargo bench -- contention

# 7. Stats overhead: ZeroPool only.
cargo bench -- stats_overhead

# 8. Workload benchmarks: real-workload pipelines generic over the buffer
#    strategy; comparison crates require the bench feature.
cargo bench --features bench --bench pipeline
cargo bench --features bench --bench tcp
cargo bench --features bench --bench file_hash
cargo bench --features bench --bench framing
cargo bench --features bench --bench compress
cargo bench --features bench --bench tiles

# 9. Emit markdown tables from the Criterion estimates.
if command -v uv >/dev/null 2>&1; then
    uv run --project scripts scripts/bench.py --no-run --tables
else
    python3 scripts/bench.py --no-run --tables
fi

echo
echo "Done. Estimates written to target/criterion/ and tables to"
echo "target/criterion-tables.md. See BENCHMARKS.md for how to read them."
