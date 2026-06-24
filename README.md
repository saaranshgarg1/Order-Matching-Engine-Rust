# Stock Order Book Matching Engine

> High-throughput, low-latency limit order book and matching engine in Rust.
> Price-time priority · single-writer-per-symbol · WAL-recoverable · live market data · ML signal sidecar.

---

## Pitch (interview-ready, 3 sentences)

Built a stock exchange matching engine in Rust. It maintains a limit order book per symbol, matches buy/sell orders by strict **price-time priority**, and supports limit, market, IOC, FOK, and stop orders with partial fills. The hot path is **single-threaded per symbol** (LMAX-disruptor style) so there are no locks on the critical matching path; every order event is written to an append-only WAL for deterministic crash recovery, and trades + book deltas stream to clients over WebSocket.

---

## Architecture

```
clients ──TCP/JSON──▶ gateway (Tokio)
                           │  ring buffer (bounded, per shard)
                           ▼
                    matching shard  ◀── single OS thread, owns book
                     (core::apply)
                           │ OutputEvents
                    ┌──────┴───────┐
                    ▼              ▼
                  WAL writer    egress bus
                  (segments,    (broadcast)
                   fsync)           │
                                    ├──▶ market-data WS publisher
                                    └──▶ gateway response path

Prometheus ◀── exchange_metrics ◀── every shard

ML sidecar (Python) ──▶ WS feed (read-only) ──▶ signals.jsonl
```

### Key design choices

| Decision | Choice | Why |
|---|---|---|
| Language | Rust | C++-class latency, no GC pauses, memory safety |
| Concurrency | Single-writer per symbol | No locks on match path; deterministic replay |
| Order book | `BTreeMap<Price, PriceLevel>` + intrusive FIFO | O(log P) insert, O(1) cancel, FIFO within level |
| Prices | `i64` ticks (never `f64`) | Exact equality, deterministic match, replay-safe |
| Persistence | Append-only WAL, command-sourcing | `apply(WAL) == live` determinism guarantee |
| Latency metric | HdrHistogram p50/p99/p999/max | Averages lie about tails |

---

## Crate map

```
crates/
  exchange-core   — order book, matcher, types (pure, no I/O)
  protocol        — binary wire codec + JSON + FIX-subset
  wal             — segmented WAL, snapshots, replay
  exchange-metrics — hdrhistogram + Prometheus exporter
  engine          — shards, ring buffers, sequencer, egress
  gateway         — Tokio TCP accept, JSON-line sessions
  marketdata      — broadcast bus, WebSocket publisher
  loadgen         — CLI load generator

ml/               — Python sidecar: mid-price predictor + anomaly detector
web/              — Live depth-chart UI (single HTML file, no build step)
deploy/           — docker-compose: Prometheus + Grafana
```

---

## Quick start

### 1. Prerequisites
```bash
# Rust (stable)
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh

# Python 3.11+ (for ML sidecar)
pip install -r ml/requirements.txt

# Docker (for observability stack)
docker compose version
```

### 2. Build
```bash
cargo build --release
```

### 3. Run the engine + gateway
```bash
cargo run --release -p gateway
# Gateway  → tcp://0.0.0.0:9001  (JSON-line order entry)
# Metrics  → http://0.0.0.0:9090/metrics
# WS feed  → ws://0.0.0.0:9002   (market data)
```

### 4. Open depth chart
```
open web/index.html   # or just double-click
```

### 5. Run load generator
```bash
cargo run --release -p loadgen -- --symbol AAPL --rate 50000 --count 500000
```

### 6. Observability stack
```bash
cd deploy && docker compose up -d
# Grafana → http://localhost:3000  (admin/admin)
# Prometheus → http://localhost:9091
```

### 7. ML sidecar
```bash
cd ml && python sidecar.py
# streams {"type":"signal",...} and {"type":"anomaly",...} JSON lines to stdout + signals.jsonl
```

---

## Testing

```bash
cargo test --workspace          # 23 unit tests
cargo bench -p exchange-core    # criterion micro-benchmarks
```

### Invariants checked by tests (plan.md §7)
- No crossed book after any operation
- Buy filled qty == sell filled qty per trade
- Earlier order at same price fills first (FIFO)
- FOK fully fills or causes zero trades
- `id_index` qty sum == level qty sum (no phantom liquidity)
- Replay(WAL) == live run (determinism)
- Cancelled order never trades; bad cancel = clean reject

---

## Correctness checklist (pre-PR)
- [ ] Book never crossed after operations
- [ ] Qty conservation per trade
- [ ] Price-time priority (FIFO within level)
- [ ] FOK atomicity
- [ ] WAL replay == live state
- [ ] No `f64` prices in `exchange-core`
- [ ] No I/O in `exchange-core`
- [ ] No unbounded queues on control plane
- [ ] No lock on matching path

---

## Performance story

The matching engine is intentionally benchmarked honestly:

- **Micro**: `cargo bench -p exchange-core` — per-`apply()` latency for insert / full-cross / cancel
- **Macro**: `cargo run -p loadgen -- --rate 0` — sustained throughput, end-to-end latency histogram printed at exit
- Latency reported as **p50 / p99 / p999 / max** (HdrHistogram). Averages are banned.

Flat-array tick book (O(1) level access) is the documented next optimization once `BTreeMap` is the measured bottleneck.

---

## ADR highlights

| ADR | Decision |
|---|---|
| ADR-001 | Rust — latency + safety without GC |
| ADR-002 | Single-writer per symbol — no hot-path locks |
| ADR-003 | Command-sourcing WAL — deterministic replay |
| ADR-005 | BTreeMap first, flat-array as measured upgrade |
| ADR-006 | Integer-tick prices — exact correctness |
| ADR-010 | ML is an external sidecar on the public feed only |
| ADR-011 | HdrHistogram percentiles — never averages |

See `adr.md` for full rationale and rejected alternatives.

---

## Non-goals (know the boundary)
- No distributed consensus / multi-node replication
- No real clearing, settlement, fees, or auth
- No full FIX engine (FIX-subset only)
- ML sidecar is a *signal layer*, not a trading strategy
