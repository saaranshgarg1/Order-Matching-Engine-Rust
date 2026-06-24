# Architecture Decision Records

Each ADR records one decision: the context, the choice, the alternatives, and the trade-off.
These are the questions an interviewer will probe — the value is in the *rejected* options.

Status legend: **Accepted** · Proposed · Superseded

---

## ADR-001 — Language: Rust for the core engine
**Status:** Accepted

**Context.** The matching path needs C++-class latency with predictable tails (no GC pauses),
plus memory safety so the project doesn't become a UB-debugging story.

**Decision.** Write the engine in **Rust**.

**Alternatives.**
- *C++*: same latency, but no memory safety; interview time spent defending UB/leaks.
- *Go*: fast to write, but **GC stop-the-world pauses** are exactly the tail-latency killer an exchange can't tolerate.
- *Java*: viable (LMAX did it) but needs heavy JVM/GC tuning and off-heap tricks to hit the tail; Rust gets there without the fight.

**Trade-off.** Steeper learning curve and stricter borrow checker vs. a clean safety + latency story and zero-cost abstractions. Worth it for a fintech-targeted resume project.

---

## ADR-002 — Concurrency: single-writer per symbol (LMAX), not a lock-free shared book
**Status:** Accepted

**Context.** Matching a single book is inherently serial (price-time priority is a total order).
We need throughput without sacrificing correctness or determinism.

**Decision.** **One thread owns one symbol's book.** No locks on the matching path. Parallelism
comes from **sharding by symbol** across threads/cores. Orders reach the shard via a bounded ring buffer.

**Alternatives.**
- *Lock-free concurrent book* (CAS on levels): enormous complexity, ABA hazards, and cache-line contention; little gain because the work is serial anyway.
- *Fine-grained locks per price level*: contention + lock ordering bugs + non-determinism.
- *Global lock*: simple but serializes everything across symbols.

**Trade-off.** A single hot symbol can't be parallelized (fundamental). But determinism is free, there are no data races by construction, and throughput scales linearly across symbols. This is the headline systems story.

---

## ADR-003 — Persistence: command-sourcing (log inputs) vs event-sourcing (log outputs)
**Status:** Accepted — **command-sourcing**, with events also logged for the audit/market-data tape

**Context.** We need crash recovery and an audit trail. Two journaling styles exist.

**Decision.** **Log the sequenced commands** (`seq`, `ts`, `Command`) before/at application;
replay = re-feed commands through the same pure `apply`. Separately persist `OutputEvent`s as
the immutable **audit + market-data tape**.

**Why command-sourcing for recovery.** Commands are smaller and the *engine is deterministic*
(ADR pure-core), so replaying commands reproduces all events exactly. It also naturally records
*rejected* orders (regulatory-relevant), which an events-only log might drop.

**Alternatives.**
- *Event-sourcing only*: replay applies state diffs (no re-matching). Robust to engine logic changes, but larger log and loses the "why" (the original intent) and rejects. Good fallback if determinism ever can't be guaranteed.

**Trade-off.** Command replay depends on `apply` being byte-deterministic across versions — so the determinism property test (plan §7) is load-bearing. We keep the event tape too, so we get both audit fidelity and a determinism cross-check.

---

## ADR-004 — Durability: group-commit fsync, configurable
**Status:** Accepted

**Context.** fsync-per-record is safest but caps throughput at disk IOPS (~thousands/sec) — far
below the 1M orders/sec target.

**Decision.** **Group-commit**: batch many records, one fsync per batch (time- or size-windowed).
Acks that promise durability wait for the batch fsync; configurable down to per-record for
correctness demos and up to `Off` for pure-speed benchmarks.

**Trade-off.** A crash can lose the last sub-millisecond, un-fsynced batch (bounded, documented).
Real venues make the same trade and recover via replication. We expose the knob and report both numbers.

---

## ADR-005 — Order book structure: BTreeMap of price levels first, flat tick-array as an optimization
**Status:** Accepted

**Context.** Need fast best-price access, ordered level traversal, and O(1) cancel.

**Decision.** Start with `BTreeMap<Price, PriceLevel>` (asks ascending, bids via `Reverse`),
intrusive FIFO queue per level, `FxHashMap<OrderId, Location>` for O(1) cancel, orders in a
slab/arena. **Then**, once benchmarked, offer the **flat array indexed by tick** (`level[price_ticks]`
+ best-price pointer/bitmap) for O(1) best-price and level access when the price range is bounded.

**Alternatives.**
- *Skip list*: same asymptotics as BTreeMap, more code, no real win in Rust.
- *Sorted Vec*: O(n) inserts mid-book — bad.
- *Flat array first*: fastest but memory ∝ price range and best-pointer bookkeeping is fiddly — premature before correctness is locked.

**Trade-off.** BTreeMap is O(log P) where flat-array is O(1), but BTreeMap is simpler and correct out of the gate. Ship correct, benchmark, optimize the hot symbol with the array, show the measured delta. That progression *is* the DSA story.

---

## ADR-006 — Price representation: fixed-point integer ticks, never f64
**Status:** Accepted

**Context.** Prices must compare and key maps exactly; floating point doesn't.

**Decision.** Prices are **`i64` in ticks** (e.g. ×10⁴). Convert to/from dollars only at the
JSON edge. Binary protocol carries ticks directly.

**Trade-off.** Slightly more conversion code at the boundary vs. exact equality, deterministic
ordering, and replayable matching. Non-negotiable for a matching engine.

---

## ADR-007 — Hot path off the async runtime
**Status:** Accepted

**Context.** Tokio is great for I/O fan-out but its scheduler adds non-deterministic jitter —
poison for tail latency.

**Decision.** Matching runs on **dedicated, pinned OS threads**. Tokio handles only gateway I/O,
WS publishing, and admin API. The two planes communicate via bounded queues.

**Trade-off.** Two concurrency models in one process (threads + async) — slightly more cognitive
load — bought predictable matching latency. Worth it.

---

## ADR-008 — Wire protocol: custom fixed-size binary, plus JSON gateway, optional FIX subset
**Status:** Accepted

**Context.** Need a fast machine protocol, an easy-to-demo protocol, and fintech credibility.

**Decision.** Primary: **custom fixed-size little-endian binary** (ITCH/OUCH-inspired,
zero-copy decode). Secondary: **JSON line protocol** for trivial demos. Optional: **FIX 4.2
subset** (35=D / 35=8) as a credibility talking point.

**Alternatives.** Protobuf/FlatBuffers (variable-length, schema overhead, slower decode than
fixed-offset); pure-FIX (verbose text, slow to parse — real venues use it for order entry but binary for market data).

**Trade-off.** Maintaining three codecs vs. covering speed, demoability, and "I speak FIX." The
binary and JSON share the same logical `Command`/`OutputEvent`, so it's one model, three encodings.

---

## ADR-009 — Market data: WebSocket primary, UDP multicast optional
**Status:** Accepted

**Context.** Clients (web UI, ML sidecar, external) need trades + book updates; real venues
multicast.

**Decision.** **WebSocket** per-symbol channels (snapshot + seq-ordered deltas) as the primary
feed — trivial to consume from a browser and Python. **UDP multicast** (sequence-numbered,
snapshot-recoverable) as an optional stretch to mirror real venues.

**Trade-off.** WS is TCP (head-of-line blocking, not how HFT venues broadcast) but universally
easy; multicast is realistic but lossy and harder to demo. Offer both; default to WS.

---

## ADR-010 — ML layer is an external sidecar on the public feed, not an engine plugin
**Status:** Accepted

**Context.** Want a useful ML component without polluting the deterministic core or giving the
model unfair private access.

**Decision.** ML runs as a **separate process** (Python: numpy/scikit-learn/lightgbm) consuming
the same public WS feed; if it trades, it submits through the **same gateway** as any client.

**Alternatives.** In-process Rust `linfa` (no second language, but couples ML lifecycle to the
engine and tempts privileged hooks); embedding the model in the hot path (latency + determinism risk).

**Trade-off.** A second language/process to run vs. a clean, honest separation: the engine stays
pure and deterministic, and the ML is evaluated on exactly the data a real participant sees. Pick
*one* ML task first (mid-price prediction) and keep it small.

---

## ADR-011 — Latency measured with HdrHistogram percentiles, never averages
**Status:** Accepted

**Context.** In trading, the tail (p99/p999/max) is the number that matters; averages hide it.

**Decision.** Record per-stage latency into **`hdrhistogram`**; export p50/p99/p999/max. Hot-path
cost is a single `record()`; aggregation is off-path.

**Trade-off.** Histograms use more memory than a running mean — negligible, and the correct
tail numbers are the entire point of a latency story.
