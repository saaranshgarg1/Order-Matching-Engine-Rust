# Stock Order Book Matching Engine — Build Plan

> A high-throughput, low-latency limit order book and matching engine written in Rust,
> with a binary wire protocol, write-ahead-log persistence, real-time market data feeds,
> observability, and a small ML signal layer.

---

## 1. Goals & non-goals

### Goals
- **Correctness first**: deterministic matching, strict price-time priority, atomic fills.
- **Low latency**: microsecond-class matching on the hot path, measured honestly.
- **High throughput**: ≥1M orders/sec per symbol shard on commodity hardware.
- **Recoverable**: crash → replay WAL → identical book state.
- **Observable**: latency percentiles, throughput, live book depth.
- **Explainable**: every design choice has a one-sentence justification.

### Non-goals
- Not a real regulated exchange (no clearing, settlement, KYC, fees, halts beyond a toy).
- Not distributed consensus (single-node authoritative book; replication is a stretch goal).
- Not a full FIX engine (we implement a *subset* + a custom binary protocol).
- ML layer is a *signal sidecar*, not a trading firm.

---

## 2. Tech stack (with justification)

| Concern | Choice | Why |
|---|---|---|
| Core language | **Rust (stable)** | Zero-cost abstractions, no GC pauses (critical for tail latency), memory safety without a runtime, great for the lock-free/ownership |
| Async runtime / networking | **Tokio** | De-facto standard; needed for the gateway, WS feed, and admin API — but kept *off* the matching hot path. |
| Hot-path threading | **OS threads + `crossbeam` channels / custom SPSC ring buffer** | The matching core must be predictable; async tasks add scheduling jitter. One pinned thread per shard. |
| Serialization (wire + WAL) | **`rkyv`** (zero-copy) or **`bincode`** | `rkyv` lets you read structs straight out of a byte buffer with no deserialization cost — strong latency story. `bincode` is simpler if you want to start fast. |
| Order ID / maps | **`FxHashMap` (rustc-hash)** | Faster non-cryptographic hashing than the default SipHash; order lookup is hot. |
| Market data transport | **`tokio-tungstenite` (WebSocket)** + internal `tokio::broadcast` bus | WS is trivial to demo in a browser; broadcast bus fans out to N subscribers. UDP multicast is an optional "this is how real venues do it" stretch. |
| Metrics | **`metrics` + `metrics-exporter-prometheus`**, **`hdrhistogram`** for latency | Prometheus scrape → Grafana. `hdrhistogram` gives correct high-percentile (p999) numbers; naive averaging lies. |
| Tracing | **`tracing` + `tracing-subscriber`** | Structured spans; can be turned off on the hot path. |
| Benchmarking | **`criterion`** (micro) + a custom load generator (macro) | Criterion for per-op latency; load-gen client for end-to-end throughput. |
| Property testing | **`proptest`** | Matching invariants are perfect for property tests (see §7). |
| ML sidecar | **Python (FastAPI + numpy/scikit-learn / lightgbm)** OR Rust `linfa` | A separate process consuming the market-data feed keeps the engine clean and lets you use the Python ML ecosystem. Recommend Python sidecar for speed of building; mention Rust `linfa` as the "no second language" alternative. |
| Dashboards | **Grafana** (metrics) + a tiny **web UI** (depth chart over WS) | Visual demo sells the project. |
| Build/dev | **Cargo workspaces**, `just` or `make`, Docker Compose for Grafana/Prometheus | Multi-crate workspace keeps boundaries clean. |

**Why Rust over C++/Go/Java here:** C++ gives the same latency but no memory safety; Go's GC introduces tail-latency pauses that are exactly what an exchange cannot have; Java needs careful JVM tuning (this is literally what LMAX fought). Rust gives C++-class latency with safety

---

## 3. Workspace layout

```
matching-engine/
├── Cargo.toml                 # workspace
├── crates/
│   ├── core/                  # order book, matching, order types — NO I/O, NO async
│   │   ├── src/
│   │   │   ├── order.rs        # Order, OrderId, Side, OrderType, lifecycle state
│   │   │   ├── price_level.rs   # FIFO queue at a single price
│   │   │   ├── book.rs          # OrderBook: bids/asks BTreeMaps + id index
│   │   │   ├── matcher.rs       # the match() function: price-time priority
│   │   │   ├── events.rs        # OutputEvent (Trade, Ack, Cancel, Reject…)
│   │   │   └── lib.rs
│   ├── wal/                   # write-ahead log: append, segment, replay
│   ├── protocol/              # binary wire format encode/decode (+ FIX subset)
│   ├── engine/                # the runtime: shards, ring buffers, threading
│   ├── gateway/               # Tokio: accept connections, parse protocol, route
│   ├── marketdata/            # broadcast bus + WebSocket publisher
│   ├── metrics/               # hdrhistogram + prometheus glue
│   └── loadgen/               # benchmark client / order generator
├── ml/                        # Python sidecar (separate, not in cargo workspace)
│   ├── feed_consumer.py
│   ├── midprice_model.py
│   └── anomaly.py
├── web/                       # tiny depth-chart UI (static HTML + JS over WS)
├── deploy/                    # docker-compose: prometheus, grafana
└── docs/                      # plan.md, architecture.md, interfaces.md, adr.md, CLAUDE.md
```

`core` has **zero** dependencies on async, I/O, or networking. This is the most important boundary: it makes the matching logic unit-testable, deterministic, and `criterion`-benchmarkable in isolation.

---

## 4. Phased build plan

Each phase ends with something runnable and demoable. Do not start a phase before the previous one is green and tested.

### Phase 1 — Core matching, single-threaded, in-memory (the heart)
**Goal:** Feed orders in via a function call, get trades out. No networking, no persistence.
- Define `Order`, `OrderId`, `Side`, `OrderType`, `OrderStatus`.
- Implement `OrderBook` with `BTreeMap<Price, PriceLevel>` for asks (ascending) and a reversed key for bids (descending), plus `FxHashMap<OrderId, Location>` for O(1) cancel.
- Implement `PriceLevel` as a FIFO queue (`VecDeque<OrderId>` to start; intrusive linked list later).
- Implement **limit order** matching with price-time priority and partial fills.
- Emit `OutputEvent`s (`Accepted`, `Trade`, `Filled`, `PartiallyFilled`).
- Implement `cancel`.
- **Tests:** unit tests for cross/no-cross, partial fill, FIFO ordering, cancel; property tests for invariants (§7).
- **Demo:** a CLI or test that submits a sequence and prints the trade tape + final book.

✅ *Milestone: "I have a working matching engine."* Everything else is plumbing around this.

### Phase 2 — Full order type matrix
- **Market** orders (walk the book, no price limit; reject/cancel remainder if book empty).
- **IOC** (immediate-or-cancel): match what you can now, cancel the rest.
- **FOK** (fill-or-kill): pre-check full fill possible; if not, reject entirely (atomic).
- **Stop / stop-limit**: held in a separate trigger structure, activated when last-trade price crosses the stop; then injected as market/limit.
- Full **order lifecycle** state machine: `New → Accepted → (PartiallyFilled)* → Filled | Cancelled | Rejected`.
- **Tests:** one suite per order type; FOK atomicity is a great property test.

### Phase 3 — Persistence & recovery (WAL)
- Append-only **write-ahead log**: every inbound command (`Submit`, `Cancel`) gets a sequence number and is written *before* it mutates the book (or: log the resulting events — pick command-sourcing, see ADR).
- **Segmented** log files (roll at N MB) with CRC per record.
- **Replay**: on startup, read all segments in order, re-apply, rebuild identical book.
- Periodic **snapshots** to bound replay time (snapshot book state + last-applied seq; replay only the tail).
- **Tests:** kill mid-stream → replay → assert book identical to a no-crash run. This is a headline test.

### Phase 4 — Engine runtime: sharding + ring buffer
- One **matching thread per symbol shard**, pinned (`core_affinity`).
- Inbound commands cross into the matching thread via a **bounded SPSC/MPSC ring buffer** (`crossbeam` or a custom disruptor-style buffer). No locks held during matching.
- Sequencer assigns a global, monotonic seq number + nanosecond timestamp at ingress.
- Output events flow out on an outbound queue to the market-data publisher and WAL.
- **Tests:** concurrency soak test — N producer threads, assert no lost/duplicated orders, determinism preserved per shard.

### Phase 5 — Wire protocol + gateway
- Define a compact **binary protocol** (fixed-size headers, little-endian, see interfaces.md): `NewOrder`, `Cancel`, `Replace` inbound; `Ack`, `Fill`, `Reject`, `Cancelled` outbound.
- Tokio TCP **gateway**: accept connections, decode frames, hand commands to the right shard's ring buffer, write responses back.
- Provide a **JSON/line gateway** too (same logical messages) so the project is trivial to demo with `nc`/a script without a custom client.
- Optional: implement a **FIX 4.2 subset** (NewOrderSingle 35=D, ExecutionReport 35=8) to say "I speak FIX."
- **Tests:** round-trip encode/decode; golden-bytes tests; fuzz the decoder (`cargo fuzz`).

### Phase 6 — Market data feeds
- Internal **broadcast bus**: trades + book deltas published to all subscribers.
- **WebSocket** publisher (`tokio-tungstenite`): clients subscribe per symbol; receive trade tape + L2 depth deltas + periodic snapshots.
- Define a **book snapshot** and **incremental delta** format (interfaces.md).
- Optional **UDP multicast** feed to mirror how real venues broadcast (ITCH-style) — great talking point.
- **Demo:** the web depth chart updates live as orders flow.

### Phase 7 — Observability
- Per-stage latency with **`hdrhistogram`**: ingress→match, match→ack, end-to-end. Export p50/p99/p999/max.
- Throughput counters (orders/sec, trades/sec, rejects).
- **Prometheus** exporter + **Grafana** dashboard (docker-compose in `deploy/`).
- **Live order book depth** visualization in the web UI.
- **Demo:** run loadgen, watch Grafana light up; show the latency histogram.

### Phase 8 — ML signal sidecar (the optional differentiator)
- Python process subscribes to the WS market-data feed.
- Pick **one** to start (mid-price prediction is the cleanest):
  - **Mid-price / micro-price prediction**: features = order-book imbalance, spread, recent trade direction; model = logistic/GBM predicting next mid move up/down.
  - **VWAP tracker**: rolling volume-weighted avg price per symbol, published back.
  - **Order-flow anomaly detection**: flag spoofing-like patterns (rapid place/cancel), unusual size, quote stuffing — simple z-score / isolation forest.
- A mock **algo strategy** can resubmit orders to the gateway based on the signal (closes the loop: data out → signal → orders in).
- **Keep it small.** It reads the public feed and (optionally) submits orders through the *same* gateway as any client. No special hooks into the engine.

### Phase 9 — Benchmark, harden, document
- `loadgen` macro benchmark: sustained throughput + latency under load; publish numbers in README.
- `criterion` micro benchmarks for `match()`, insert, cancel.
- Honest **performance report** (numbers + flame graph + what limits you).
- Polish README with the architecture diagram, the demo GIF, and the pitch from §0.

---

## 5. Data structures & algorithms — the decisions that matter

| Component | Structure | Reasoning | Alternatives considered |
|---|---|---|---|
| Price → orders | `BTreeMap<Price, PriceLevel>` (asks); bids keyed by `Reverse<Price>` | Sorted, O(log n) insert/remove, **best price is `.first()/.iter().next()`** in O(log n) (or O(1) with a cached best). Matching walks levels in price order naturally. | Skip list (same complexity, more code, no win in Rust); red-black tree (that's what BTreeMap is, roughly); flat array of ticks (below). |
| Orders at one price | FIFO queue — start `VecDeque<OrderId>`, upgrade to **intrusive doubly-linked list** | Time priority = FIFO. Intrusive list gives O(1) cancel-from-middle without scanning, and the node lives in the order arena (cache-friendly, no per-order alloc). | `VecDeque` is O(n) to remove from middle — fine early, fix in Phase 4+. |
| Order lookup (cancel/replace) | `FxHashMap<OrderId, OrderLocation>` | Cancel must be O(1): jump straight to the order's node + price level. | Linear scan (unacceptable). |
| Order storage | **Arena / slab** (`Vec<OrderSlot>` + freelist), orders referenced by index | Stable indices, no allocator churn on the hot path, cache locality. Index doubles as the intrusive-list pointer. | `Box<Order>` per order = allocation per order = latency + fragmentation. |
| Stop orders | Separate `BTreeMap<TriggerPrice, ...>` per side | Checked against last-trade price after each match; triggered stops re-enter as market/limit orders. Keeps the resting book clean. | Mixing stops into the main book breaks price-time semantics. |
| Sequencing | `AtomicU64` seq + monotonic `Instant`/TSC timestamp at ingress | Total order across all inputs → determinism → replayable. | Wall-clock time is non-monotonic; never use it for ordering. |

### The flat-array optimization (mention as the "I know how real HFT does it" upgrade)
When the price range is bounded and tick size is fixed, replace the `BTreeMap` with a **direct-indexed array of price levels** (`level[price_in_ticks]`) plus a bitmap / cached best-price pointer. This turns best-price access and level lookup into **O(1)** array indexing — what the fastest real engines do. Trade-off: memory proportional to price range, and you must handle the best-price pointer carefully. Ship `BTreeMap` first (simple, correct), benchmark, then offer the flat array as the optimization with measured numbers.

### The matching algorithm (price-time priority), in words
```
on incoming aggressive order O (say a buy):
  while O has remaining qty AND best ask exists AND best ask price <= O.limit (or O is market):
      level = best ask level
      while O.remaining > 0 AND level not empty:
          resting = head of level            # oldest = time priority
          traded = min(O.remaining, resting.remaining)
          emit Trade(price = resting.price, qty = traded)   # resting order sets the price
          O.remaining   -= traded
          resting.remaining -= traded
          if resting.remaining == 0: remove resting (pop head), free slot, drop from id-index
      if level empty: remove level from BTreeMap
  if O.remaining > 0 AND O is a resting type (limit, GTC):
      insert O into its side at O.limit (append to that level's FIFO tail)
  else if O.remaining > 0:  # market/IOC leftover
      cancel remainder
  after matching: check stop triggers against new last-trade price
```
The resting order always sets the trade price (price improvement goes to the order that was there first).

---

## 6. Concurrency model — the headline systems story

**Single-writer principle (LMAX Disruptor):** each symbol's book is owned and mutated by exactly **one thread**. There are **no locks on the matching path**. This is counterintuitive and is the thing to lead with: you don't make matching fast by parallelizing it, you make it fast by *not sharing it*.

- **Ingress**: gateway (Tokio threads) parse orders, stamp seq+timestamp, push into the target shard's **bounded ring buffer**. Producers may be many; the consumer (matcher) is one.
- **Matching**: the shard thread spins/blocks on its ring buffer, pulls commands in order, mutates its book, emits events. Deterministic given the input sequence.
- **Egress**: events go to (a) the WAL writer and (b) the market-data broadcast bus via output queues.
- **Scaling**: more symbols → more shards → more cores. Throughput scales horizontally across symbols, not within a single symbol (which is inherently serial — you cannot match the same book from two threads and stay correct).

Why not fine-grained locks / lock-free book? You *can*, and you should be able to discuss it, but the single-writer design is simpler, faster in practice (no contention, no cache-line ping-pong), and trivially correct. Lead with it; mention lock-free as the thing you deliberately *didn't* need.

**Timestamps:** monotonic source (`Instant`, or rdtsc/TSC for sub-microsecond) at ingress. Sequence number is the real tiebreaker; timestamp is for latency measurement and reporting.

---

## 7. Correctness: invariants & property tests

State these as invariants and test them with `proptest` over random valid order streams:
1. **No crossed book**: after every operation, best_bid < best_ask (or one side empty).
2. **Conservation**: total filled buy qty == total filled sell qty at every trade.
3. **Price-time priority**: for any two resting orders at the same price, the earlier-arriving one fills first.
4. **FOK atomicity**: a FOK order either fully fills or causes zero trades.
5. **No phantom liquidity**: sum of qty in id-index == sum of qty across all price levels.
6. **Replay determinism**: replay(WAL) produces byte-identical book state and trade tape vs. the live run.
7. **Cancel correctness**: a cancelled order never trades afterward; cancel of unknown/filled id is a clean reject.

---

## 8. What the finished demo looks like

1. `docker compose up` brings up Prometheus + Grafana.
2. `cargo run -p engine` starts the exchange (gateway on TCP, WS feed, metrics endpoint).
3. Open `web/index.html` → live **order book depth chart** + **trade tape** for `AAPL`.
4. Run `cargo run -p loadgen -- --symbol AAPL --rate 1000000` → orders flood in.
5. Grafana dashboard shows **orders/sec**, **trades/sec**, and **p50/p99/p999 latency** climbing.
6. Start the **ML sidecar** → a "predicted next mid-move" arrow appears on the chart; the anomaly detector flags a deliberately-injected spoofing burst.
7. **Kill the engine mid-load, restart** → it replays the WAL and the book is exactly where it was. Show the determinism test passing.
8. Show the **latency histogram** and the README perf table.

That sequence — correctness, speed, recovery, live data, and a dash of ML — is the whole story in ~3 minutes.

---

## 9. Suggested timeline (calibrate to your pace)

| Weeks | Phases | Output |
|---|---|---|
| 1–2 | 1–2 | Working matcher, all order types, full test suite |
| 3 | 3 | WAL + crash-recovery + determinism test |
| 4 | 4–5 | Sharded runtime + binary/JSON gateway |
| 5 | 6–7 | WS market data + Grafana + depth UI |
| 6 | 8–9 | ML sidecar + benchmarks + README/docs |

Ship Phase 1–3 even if you stop early — that alone (a correct, recoverable matching engine in Rust with tests) is a strong project. Phases 4–9 take it from "strong" to "stand-out."

---

## 10. Reading / reference (know these names)
- **LMAX Disruptor** (single-writer, ring buffer) — the concurrency thesis.
- **Nasdaq ITCH / OUCH** — real binary market-data / order-entry protocols (model your protocol on these).
- **FIX protocol** — NewOrderSingle (35=D), ExecutionReport (35=8).
- **"How to build a fast limit order book"** (WK Selph blog) — the array-of-price-levels design.
- **Price-time priority / pro-rata** matching — know the difference.

See `architecture.md` for the component diagram, `interfaces.md` for concrete message/struct definitions, and `adr.md` for the recorded decisions and their trade-offs.
