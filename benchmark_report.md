# Benchmark Report — `exchange-core` Matching Engine

> **Date:** 2026-07-01  
> **Machine:** Linux (commodity desktop/laptop hardware)  
> **Rust toolchain:** stable (release profile)  
> **Benchmark framework:** [Criterion.rs](https://bheisler.github.io/criterion.rs/book/)  
> **Crate under test:** `exchange-core` (`crates/core`)

---

## 1. What Was Tested

The benchmark targets the **pure matching core** — the `apply()` function in
[`crates/core/src/matcher.rs`](crates/core/src/matcher.rs).

`apply()` is the single entry point for all order book mutations:

```
apply(book: &mut OrderBook, cmd: &Sequenced, out: &mut Vec<OutputEvent>)
```

It is **intentionally free of I/O, async, networking, and heap allocation on
the hot path** — which makes it independently benchmarkable and directly
comparable to theoretical throughput limits.

Three scenarios were benchmarked (defined in
[`crates/core/benches/matcher_bench.rs`](crates/core/benches/matcher_bench.rs)):

| Benchmark | What it measures |
|---|---|
| `insert_1000_no_cross` | 1 000 limit inserts with no price crossing — pure resting-book insert path |
| `match_throughput/{n}` | n asks pre-loaded, then n crossing buys sweep the book — full match + fill path |
| `cancel_1000` | 1 000 inserts followed by 1 000 cancels — cancel O(1) intrusive-list path |

---

## 2. Test Process

### Step 1 — Build in release mode (implicit)

`cargo bench` automatically compiles with `--release`. No manual build step
is needed.

### Step 2 — Run the Criterion suite

```bash
cargo bench -p exchange-core
```

Criterion executes each benchmark in three phases:

1. **Warm-up** (3 s) — the function runs repeatedly to bring the instruction
   cache and branch predictor to a stable state.
2. **Sample collection** (100 samples) — Criterion decides how many iterations
   fit in ≈5 s and collects timing samples.
3. **Statistical analysis** — reports mean time ± confidence interval and flags
   outliers.

The backend used for plots was **Plotters** (Gnuplot was not installed on this
machine; results are unaffected).

### Step 3 — Read results

Results print to stdout immediately after each benchmark finishes.
HTML reports are written to `target/criterion/` if you want graphs.

---

## 3. Results

All timings are **wall-clock time per full batch iteration** as reported by
Criterion (mean ± ~95 % CI). The ops/sec column is derived by dividing the
number of `apply()` calls in one iteration by the mean time.

### Raw Criterion output

```
insert_1000_no_cross    time:   [36.544 µs  36.815 µs  37.111 µs]
match_throughput/100    time:   [ 6.9200 µs  6.9551 µs  6.9932 µs]
match_throughput/1000   time:   [61.749 µs  62.210 µs  62.690 µs]
match_throughput/10000  time:   [644.24 µs  647.52 µs  651.03 µs]
cancel_1000             time:   [49.133 µs  49.572 µs  49.994 µs]
```

### Derived throughput table

| Benchmark | `apply()` calls / iter | Mean time | **ns / op** | **Ops / sec** |
|---|---|---|---|---|
| `insert_1000_no_cross` | 1 000 | 36.82 µs | **36.8 ns** | **~27.2 M/s** |
| `match_throughput/100` | 200 | 6.96 µs | **34.8 ns** | **~28.8 M/s** |
| `match_throughput/1000` | 2 000 | 62.21 µs | **31.1 ns** | **~32.2 M/s** |
| `match_throughput/10000` | 20 000 | 647.5 µs | **32.4 ns** | **~30.9 M/s** |
| `cancel_1000` | 2 000 | 49.57 µs | **24.8 ns** | **~40.3 M/s** |

> **Note on `apply()` call count:**  
> `match_throughput/n` pre-loads *n* resting asks (each is one `apply()` call)
> then sweeps with *n* crossing buys (each is another `apply()` call), for
> **2n total calls** per Criterion iteration.  
> `cancel_1000` inserts 1 000 orders then cancels all 1 000 — **2 000 calls**
> per iteration.

---

## 4. Analysis

### Does the project's ≥1 M orders/sec claim hold?

**Yes — with significant margin in the pure core.**

The design goal stated in [`plan.md §1`](plan.md) is:

> *"High throughput: ≥1M orders/sec per symbol shard on commodity hardware."*

The measured pure-core throughput is **~27–40 M ops/sec**, which is
**27× – 40× above the 1 M/sec target**.

### Why such headroom?

The design choices in the core directly explain the numbers:

| Design choice | Impact |
|---|---|
| `BTreeMap<Price, PriceLevel>` (O(log P) insert) | Scales well for realistic price ranges; ~32 ns/op at 10 000-order depth |
| Intrusive doubly-linked FIFO per price level | O(1) cancel-from-middle — explains why `cancel_1000` is the *fastest* per-op |
| `FxHashMap<OrderId, Location>` (rustc-hash) | O(1) cancel/replace lookup, faster than std SipHash |
| Slab/arena for order storage | No per-order heap allocation on the hot path |
| Zero I/O in `exchange-core` | No system-call overhead — pure CPU |

### Where does the remaining budget go end-to-end?

The full pipeline adds stages on top of `apply()`:

```
TCP recv → JSON parse → ring buffer crossing → apply() → WAL group-commit → egress broadcast
```

At 1 M orders/sec the engine has a **1 µs per-order budget** end-to-end.
The pure `apply()` consumes only **~32–37 ns** of that budget, leaving
**~963 ns** for all other stages. JSON parsing + TCP I/O is the most
likely bottleneck in the demo path; the binary wire protocol (ADR-008)
and WAL group-commit (ADR-004) are explicitly designed to keep this overhead
below 1 µs/order.

### Scaling note

The `match_throughput` numbers are **stable across 100×, 1000×, and 10 000×
depth** (~31–35 ns/op), confirming that the `BTreeMap` cost scales as
O(log P) rather than O(n orders) — matching the theoretical expectation.
The flat tick-array optimization (ADR-005) would push this toward O(1),
but is not necessary to satisfy the 1 M/sec target.

---

## 5. How to Reproduce

### Prerequisites

```bash
# Install Rust stable toolchain (if not already installed)
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
source $HOME/.cargo/env

# Verify
rustc --version   # should print rustc 1.xx.x (stable)
cargo --version
```

### Run the benchmarks

```bash
# From the repository root
cd /path/to/DSA-PROJ

# Run only the exchange-core micro-benchmarks
cargo bench -p exchange-core

# Results print to stdout.
# HTML reports (with graphs) are written to:
#   target/criterion/insert_1000_no_cross/report/index.html
#   target/criterion/match_throughput/*/report/index.html
#   target/criterion/cancel_1000/report/index.html
```

### Run a specific benchmark only

```bash
# Only the match_throughput group
cargo bench -p exchange-core -- match_throughput

# Only cancel
cargo bench -p exchange-core -- cancel_1000
```

### Run the full end-to-end macro benchmark (sustained throughput)

```bash
# Terminal 1 — start the engine
cargo run --release -p gateway

# Terminal 2 — saturate it (rate=0 means no throttle)
cargo run --release -p loadgen -- --symbol AAPL --rate 0 --count 5000000 --tasks 8

# The loadgen prints "rate=X/s" every second and a final summary.
# Compare X against the 1 M/sec target under realistic conditions.
```

### Expected output (approximate — varies by hardware)

```
insert_1000_no_cross    time:   [~35 µs  ~37 µs  ~39 µs]
match_throughput/100    time:   [ ~6.5 µs  ~7.0 µs  ~7.5 µs]
match_throughput/1000   time:   [~58 µs   ~62 µs   ~66 µs]
match_throughput/10000  time:   [~620 µs  ~650 µs  ~680 µs]
cancel_1000             time:   [~46 µs   ~50 µs   ~54 µs]
```

Numbers will vary with CPU clock speed, cache size, and system load.
Run on a **quiet machine** (no background compilation, no browser, etc.) for
most reproducible results.

---

## 6. Related Files

| File | Purpose |
|---|---|
| [`crates/core/benches/matcher_bench.rs`](crates/core/benches/matcher_bench.rs) | Criterion benchmark definitions |
| [`crates/core/src/matcher.rs`](crates/core/src/matcher.rs) | `apply()` implementation under test |
| [`crates/core/src/book.rs`](crates/core/src/book.rs) | `OrderBook` data structure |
| [`crates/loadgen/src/main.rs`](crates/loadgen/src/main.rs) | End-to-end macro load generator |
| [`plan.md §1`](plan.md) | Original ≥1 M/sec throughput goal |
| [`adr.md — ADR-004`](adr.md) | Group-commit WAL fsync decision (enables high throughput) |
| [`adr.md — ADR-005`](adr.md) | BTreeMap first, flat-array as measured upgrade |
