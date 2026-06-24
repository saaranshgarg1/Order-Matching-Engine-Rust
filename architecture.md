# Architecture — Stock Order Book Matching Engine

This document describes the runtime architecture, the data flow of a single order, the
threading/concurrency model, and the responsibilities of each crate. Read `plan.md` first
for the build order and `interfaces.md` for concrete types.

---

## 1. System overview

```
                            ┌──────────────────────────────────────────────────────┐
                            │                    EXCHANGE PROCESS                    │
   clients (TCP/WS/FIX)     │                                                        │
        │  binary/JSON      │   ┌──────────┐    ring buffer     ┌───────────────┐    │
        ▼  NewOrder/Cancel  │   │ GATEWAY  │  (SPSC/MPSC, per   │  MATCHING      │    │
   ┌─────────────┐  ───────────▶│ (Tokio)  │──── shard) ───────▶│  SHARD #k      │    │
   │   client    │          │   │ decode + │                    │  (1 thread,    │    │
   │   /loadgen  │◀───────────  │ sequence │◀── responses ──────│   owns book)   │    │
   └─────────────┘  acks/fills │   └──────────┘                    └───────┬───────┘    │
        ▲                     │        ▲                                  │ events     │
        │  market data (WS)   │        │                                  ▼            │
        │                     │   ┌──────────┐   broadcast bus   ┌───────────────┐    │
   ┌─────────────┐            │   │ MARKET   │◀──────────────────│  EGRESS        │    │
   │  web UI /    │◀───────────  │ DATA PUB │   trades + deltas  │  (events out)  │    │
   │  ML sidecar  │   WS feed  │   └──────────┘                    └───────┬───────┘    │
   └─────────────┘            │                                          │            │
                              │                                          ▼            │
                              │                                    ┌───────────┐      │
                              │                                    │   WAL     │ disk │
                              │   metrics (Prometheus scrape) ◀────│  writer   │      │
                              │                                    └───────────┘      │
                              └──────────────────────────────────────────────────────┘
                                          │
                                          ▼
                                 Prometheus → Grafana
```

Two planes:
- **Control / hot plane** (latency-critical): gateway → ring buffer → matching shard → egress. Kept lock-free and predictable.
- **Side plane** (throughput-tolerant): WAL persistence, market-data fan-out, metrics. These must never block the matching thread; they consume from queues asynchronously.

---

## 2. Crate responsibilities

| Crate | Owns | Depends on | Hard rule |
|---|---|---|---|
| `core` | `Order`, `OrderBook`, `PriceLevel`, `matcher`, `OutputEvent`, lifecycle | nothing async, no I/O | **Pure & deterministic.** Same inputs → same outputs, always. No clock reads, no logging on the hot path, no allocation per match if avoidable. |
| `wal` | append-only segmented log, snapshots, replay | `core` (types), `protocol` (encoding) | Durable before acked (configurable fsync policy). |
| `protocol` | binary + JSON + FIX-subset encode/decode | `core` (types) | No engine logic; pure codec. Fuzzable. |
| `engine` | shards, ring buffers, sequencer, thread pinning, egress routing | `core`, `wal`, `metrics` | Owns the threading model. The only place threads are spawned for matching. |
| `gateway` | TCP/WS accept, frame decode, route to shard, write responses | `protocol`, `engine` | Tokio lives here, **not** in `core`/`engine` matching path. |
| `marketdata` | broadcast bus, WS publisher, snapshot/delta builder, (UDP multicast) | `core`, `protocol` | Read-only consumer of engine events. Never mutates the book. |
| `metrics` | `hdrhistogram` recorders, Prometheus exporter | — | Sampling cheap enough for the hot path; aggregation off-path. |
| `loadgen` | order generator, latency client, scenarios | `protocol` | Test/bench only. |

The `core` boundary is sacred: if `core` ever needs `tokio`, a socket, or `std::time::SystemTime`, the design has leaked. Push that concern up into `engine`/`gateway`.

---

## 3. Life of an order (end-to-end data flow)

1. **Arrival.** Client sends `NewOrder` bytes over TCP. Gateway (Tokio task) reads the frame.
2. **Decode.** `protocol` decodes bytes → `Command::New(NewOrder)`. Malformed → immediate `Reject`, no engine involvement.
3. **Sequence.** The **sequencer** assigns a global monotonic `seq: u64` and an ingress `ts: u64` (nanoseconds, monotonic). This is the moment that defines ordering and starts the latency clock.
4. **Route.** Gateway pushes the sequenced command into the **ring buffer of the shard** that owns `symbol` (`shard = hash(symbol) % N`).
5. **Persist-intent (optional, see ADR-003).** Either here (command-sourcing: log the command before applying) or at egress (event-sourcing: log resulting events). Pick one; document it.
6. **Match.** The shard thread pops the command in seq order and calls `core::matcher::apply(&mut book, command)`:
   - validates (known symbol, sane price/qty, dup order id),
   - runs price-time matching, mutating the book,
   - produces a `Vec<OutputEvent>` (`Accepted`, `Trade`×n, `Filled`/`PartiallyFilled`/`Cancelled`/`Rejected`).
7. **Egress.** Events are pushed to the outbound queue with the originating `seq`. From there they fan out to:
   - **WAL writer** (durable audit log),
   - **market-data bus** (trades + book deltas → WS subscribers),
   - **gateway response path** (ack/fill back to the submitting client),
   - **metrics** (record end-to-end latency = now − ingress ts).
8. **Stop triggers.** After matching, the shard checks resting stop orders against the new last-trade price; triggered stops are injected as fresh commands (re-entering at step 6) — preserving determinism.

Every hop after step 6 is **off the hot path**: the matching thread hands events to a queue and immediately returns to the next order. WAL fsync, WS sends, and Prometheus aggregation happen on other threads.

---

## 4. The matching shard (core of the system)

```
loop {
    cmd = ring_buffer.recv();          // blocks/spins; in-order; single consumer
    let t0 = clock();                  // latency start (cheap monotonic)
    let events = matcher::apply(&mut self.book, cmd);   // the ONLY mutation of book
    check_stop_triggers(&mut self.book, &mut events);
    egress.push(events, cmd.seq, t0);  // hand off, do not block
}
```

Properties:
- **Single writer**: `self.book` is touched by this thread only → no locks, no atomics on book fields, no data races by construction.
- **Deterministic**: `apply` is a pure function of `(book_state, cmd)`. Replaying the same command sequence reproduces the book exactly.
- **Bounded work per order**: matching is O(levels touched + fills). Cancel is O(1). Insert is O(log P) with BTreeMap, O(1) with the flat-array tick book.

A shard owns a disjoint set of symbols, so two shards never touch the same book — safe to run on separate cores with zero coordination.

---

## 5. Concurrency & threading

| Thread / pool | Role | Sharing |
|---|---|---|
| Tokio worker pool | gateway accept/decode, WS publish, admin API | Shares nothing mutable with books; communicates via queues only. |
| Matching shard threads (1 per shard, pinned) | own and mutate one set of books | No shared mutable state across shards. |
| WAL writer thread(s) | drain egress → encode → append → fsync | Consumes a queue; back-pressures via bounded queue if disk lags. |
| Market-data thread(s) | drain egress → build deltas → broadcast | Read-only on event stream. |

**Cross-thread transport:**
- Gateway → shard: bounded **ring buffer** (SPSC if one gateway thread per shard; MPSC if many). Bounded = natural back-pressure; if a shard falls behind, producers slow rather than blowing memory.
- Shard → egress consumers: bounded queue + `tokio::broadcast` for fan-out.

**Why bounded everywhere:** unbounded queues hide latency as memory growth and eventually OOM. Bounded queues surface overload as back-pressure you can measure.

**Clocks:** ingress timestamp uses a monotonic source (`Instant` / TSC). Wall-clock is recorded separately only for human-facing audit records, never for ordering.

---

## 6. Persistence architecture (WAL)

```
   egress events ──▶ [encode record: seq | ts | type | payload | crc32]
                         │
                         ▼
                 ┌──────────────────┐   roll at N MB    ┌──────────────────┐
                 │ segment 0001.wal │ ───────────────▶ │ segment 0002.wal │ ...
                 └──────────────────┘                   └──────────────────┘
                         ▲
            snapshot ────┘  [book state @ seq S]  (periodic, bounds replay)

   recovery:  load latest snapshot (seq S) → replay records with seq > S → live
```

- **Append-only**, one write per event/command, each framed with a length + CRC for torn-write detection.
- **Segmented** so old data can be archived/compacted; replay reads segments in order.
- **Snapshots** periodically dump full book state + last-applied seq so startup doesn't replay all history.
- **fsync policy** is configurable (per-record = safest/slowest, group-commit = batched fsync = fast + durable). See ADR-004.
- **Recovery test** (headline): a run that crashes and a run that doesn't must yield identical final book + trade tape.

---

## 7. Market data plane

- The shard emits **trade events** and **book-change deltas** (level added/removed/qty-changed).
- The **market-data builder** maintains an L2 view and emits:
  - **snapshots** (full top-N depth) on subscribe and periodically,
  - **incremental deltas** between snapshots (compact, ordered by seq).
- Transport: **WebSocket** per-symbol channels (easy browser/ML demo). Optional **UDP multicast** to mirror real venues (fire-and-forget, sequence-numbered, clients recover gaps via snapshot).
- Subscribers: the web depth UI, the ML sidecar, and any external client. They consume the **same public feed** — the ML layer gets no privileged access (keeps the architecture honest and the engine clean).

---

## 8. Observability plane

- **Latency**: `hdrhistogram` per stage (ingress→match, match→ack, end-to-end). Histograms merged and exported as p50/p99/p999/max. (Averages are banned — they hide the tail that matters in trading.)
- **Throughput**: counters for orders/sec, trades/sec, rejects/sec, cancels/sec, per symbol/shard.
- **Book health**: depth, spread, best bid/ask gauges.
- **Export**: Prometheus endpoint scraped into Grafana (`deploy/docker-compose.yml`).
- **Depth viz**: web UI renders L2 depth + trade tape live over the WS feed.

Sampling on the hot path is a single histogram `record()` (a few ns); all aggregation/exposition happens off-path.

---

## 9. Failure & back-pressure behavior

| Failure | Behavior |
|---|---|
| Malformed wire frame | Gateway rejects, never reaches engine. |
| Unknown/duplicate order id | `matcher` emits `Rejected`; book unchanged. |
| Shard ring buffer full | Producer back-pressures (bounded) → gateway applies flow control to that client. |
| WAL disk slow/full | Egress queue fills → back-pressure to shard → to producers. Surfaced as a metric/alert, not silent loss. |
| Process crash | Restart → load snapshot → replay WAL tail → resume. Determinism test guards correctness. |
| Slow market-data subscriber | Dropped from broadcast / sent a fresh snapshot on reconnect; never blocks the engine. |

The invariant: **a slow side-plane consumer (disk, a subscriber) must never stall the matching thread**, only back-pressure ingress.

---

## 10. Security / integrity notes (toy-exchange scope)
- Order-entry sessions should carry a client id; reject orders for unknown sessions. (No real auth/clearing — state this boundary.)
- WAL records are CRC-protected against corruption, not tampering (no crypto signing — out of scope, but name it).
- Input validation (price/qty bounds, tick size, lot size) happens at `matcher` validation, before any book mutation.

---

## 11. Diagram: module dependency (compile-time)

```
            loadgen        web/ml (external)
               │                 │
               ▼                 ▼
   gateway ─▶ engine ─▶ wal ─▶ protocol ─▶ core
      │          │                          ▲
      └─▶ protocol│                          │
            marketdata ───────────────────────┘
                  └─▶ metrics
```
Arrows = "depends on". `core` is the sink (depends on nothing internal). Nothing depends on `gateway` except `loadgen`/external clients. This acyclic shape keeps the latency-critical core testable and reusable.
