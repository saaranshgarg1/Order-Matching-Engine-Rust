# CLAUDE.md — Working Agreement for This Repo

Guidance for AI agents (and humans) working in this codebase. Read `plan.md`,
`architecture.md`, `interfaces.md`, and `adr.md` before making non-trivial changes.

---

## What this project is
A high-throughput, low-latency **stock order book matching engine** in Rust. It matches
buy/sell orders by **price-time priority**, supports limit/market/IOC/FOK/stop orders with
partial fills, persists every event to a write-ahead log for crash recovery, streams trades
and book deltas to clients, and exposes latency/throughput metrics. A small ML sidecar
consumes the public feed for mid-price prediction / anomaly detection.

One-liner for any context: *price-time priority matching engine, single-writer per symbol,
WAL-recoverable, with a binary wire protocol and live market data.*

---

## The rules that must not be broken

1. **`core` is pure.** The `core` crate (order book + matcher) has **no async, no I/O, no
   socket, no `SystemTime`, no logging on the hot path**. `OrderBook::apply` must be a pure
   function of `(state, command)`. This is what makes replay deterministic and benchmarking
   honest. If you reach for `tokio` or a clock inside `core`, stop — push it up to `engine`/`gateway`.

2. **Single writer per symbol.** A book is mutated by exactly one thread. Never add a lock to
   the matching path or share a book across threads. Parallelism comes from sharding by symbol
   (ADR-002).

3. **Prices are integer ticks (`i64`), never `f64`.** Convert dollars↔ticks only at the JSON
   edge (ADR-006). Comparing or keying maps with floats is a correctness bug.

4. **Bounded queues everywhere on the control plane.** Overload must surface as back-pressure,
   not unbounded memory growth. No unbounded channels between gateway → shard → egress.

5. **Determinism is load-bearing.** Recovery via command replay depends on `apply` being
   byte-deterministic. Any change to matching logic must keep the determinism property test
   green (replay == live).

6. **Maker sets the price.** In a trade, the resting (maker) order's price is the execution
   price; the aggressor crosses the spread. Don't "fix" this.

7. **Side plane never stalls the hot plane.** A slow WAL disk or slow market-data subscriber
   must back-pressure ingress, never block the matching thread.

---

## Where things live
```
crates/core        matching engine, order book — PURE, no I/O          (start here)
crates/wal         append-only log, segments, snapshots, replay
crates/protocol    binary + JSON + FIX-subset codecs
crates/engine      shards, ring buffers, sequencer, thread pinning
crates/gateway     Tokio TCP/WS accept + routing
crates/marketdata  broadcast bus + WS publisher
crates/metrics     hdrhistogram + Prometheus
crates/loadgen     benchmark / order generator
ml/                Python sidecar (separate process)
web/               live depth-chart UI
deploy/            docker-compose: prometheus + grafana
docs are at repo root: plan.md, architecture.md, interfaces.md, adr.md
```

---

## Build order (don't skip ahead)
Follow `plan.md` phases. Each phase must be green + tested before the next:
1. Core matching (limit only) → 2. all order types → 3. WAL + recovery → 4. sharded runtime →
5. protocol + gateway → 6. market data → 7. observability → 8. ML sidecar → 9. benchmark.

**Phases 1–3 alone are a complete, defensible project.** Don't let later phases block shipping
a correct, recoverable engine.

---

## Testing expectations
- Unit tests for every matching scenario (cross/no-cross, partial fill, FIFO, cancel, each order type).
- **Property tests** (`proptest`) for the invariants in `plan.md` §7 — these matter more than coverage:
  no crossed book, qty conservation, price-time priority, FOK atomicity, no phantom liquidity,
  **replay determinism**, cancel correctness.
- A **crash-recovery test**: kill mid-stream, replay WAL, assert identical book + tape.
- `criterion` micro-benchmarks for `apply`/insert/cancel; `loadgen` macro benchmark for throughput.
- Don't mark a matching change done until the determinism property test passes.

---

## Conventions
- Rust stable, `cargo fmt` + `cargo clippy` clean (treat clippy warnings as errors in CI).
- No `unwrap()`/`panic!` on the order path — return `Reject` for bad input; reserve panics for
  truly impossible invariants (and document them).
- Avoid allocation on the hot path: reuse the events `Vec`, use the slab/arena for orders.
- Keep `unsafe` rare, localized, and commented with the invariant it upholds (e.g. intrusive
  list pointers, TSC reads). Prefer safe code until a benchmark justifies otherwise.
- Latency is reported as **p50/p99/p999/max**, never as an average (ADR-011).
- Commit messages: Conventional Commits, scoped by crate (e.g. `feat(core): add FOK matching`).

---

## When extending the system
- New order type → add to `OrderType`, handle in `matcher`, add unit + property tests, update
  `interfaces.md` and the protocol enums.
- New wire message → update `protocol` (binary + JSON), add round-trip + golden-bytes tests,
  fuzz the decoder, update `interfaces.md`.
- New ADR-worthy decision (anything that trades off correctness/latency/complexity) → append an
  ADR; don't bury the rationale in code.
- Touching the matching algorithm → re-read ADR-002/003/006, re-run determinism + invariant tests.

---

## Anti-goals (don't build these unprompted)
- Distributed consensus / multi-node replication (single authoritative node; replication is a
  documented stretch goal only).
- Real clearing/settlement/fees/auth beyond a toy session id.
- A full FIX engine (a subset is enough).
- A large ML system — the sidecar stays small and reads only the public feed (ADR-010).

---

## Quick "is it correct?" checklist before any matching PR
- [ ] Book never crossed after operations.
- [ ] Buy filled qty == sell filled qty per trade.
- [ ] Earlier order at same price fills first.
- [ ] FOK either fully fills or trades nothing.
- [ ] id-index qty sum == level qty sum.
- [ ] Replay(WAL) == live run (determinism).
- [ ] Cancelled order never trades after; bad cancel = clean reject.
- [ ] No `f64` prices, no I/O in `core`, no unbounded queues, no lock on the match path.
