# Interfaces — Types, Wire Protocol, and APIs

Concrete contracts for the matching engine. Rust signatures are illustrative (final code may
differ slightly) but the **shapes and invariants are binding**. See `architecture.md` for how
these connect and `adr.md` for why.

---

## 1. Core domain types (`core` crate)

```rust
/// Fixed-point price in ticks. NEVER use f64 for price — rounding breaks priority & matching.
/// e.g. price_ticks = dollars * 10_000 (4 dp). Tick size is a market parameter.
pub type Price = i64;          // in ticks
pub type Qty   = u64;          // in lots/shares
pub type OrderId = u64;        // client-or-exchange assigned, unique per session
pub type Seq   = u64;          // global monotonic ingress sequence
pub type Ts    = u64;          // monotonic nanoseconds at ingress

#[derive(Copy, Clone, PartialEq, Eq)]
pub enum Side { Buy, Sell }

#[derive(Copy, Clone, PartialEq, Eq)]
pub enum OrderType {
    Limit,        // rest at price if not fully matched
    Market,       // match at any price; cancel remainder
    Ioc,          // immediate-or-cancel: match now, cancel remainder (never rests)
    Fok,          // fill-or-kill: fully fill atomically or zero trades
    StopMarket,   // becomes Market when trigger crosses
    StopLimit,    // becomes Limit  when trigger crosses
}

#[derive(Copy, Clone, PartialEq, Eq)]
pub enum TimeInForce { Gtc, Day, Ioc, Fok }   // GTC default for Limit

#[derive(Copy, Clone, PartialEq, Eq)]
pub enum OrderStatus {
    New, Accepted, PartiallyFilled, Filled, Cancelled, Rejected,
}

pub struct Order {
    pub id: OrderId,
    pub side: Side,
    pub kind: OrderType,
    pub tif: TimeInForce,
    pub price: Price,          // limit price; ignored for Market
    pub stop_price: Price,     // trigger; only for Stop* types
    pub qty: Qty,              // original
    pub remaining: Qty,        // decremented on fills
    pub seq: Seq,              // ordering / time-priority key
    pub ts: Ts,                // ingress timestamp
    pub status: OrderStatus,
}
```

**Why fixed-point price:** floating point makes `price_a == price_b` unreliable and breaks
deterministic priority and replay. All prices are integers in ticks.
---

## 2. Commands (engine input)

```rust
pub enum Command {
    New(NewOrder),
    Cancel { id: OrderId, symbol: SymbolId },
    Replace { id: OrderId, new_price: Price, new_qty: Qty, symbol: SymbolId },
}

pub struct NewOrder {
    pub id: OrderId,
    pub symbol: SymbolId,
    pub side: Side,
    pub kind: OrderType,
    pub tif: TimeInForce,
    pub price: Price,
    pub stop_price: Price,
    pub qty: Qty,
}

/// Stamped at ingress by the sequencer; this is what the shard actually consumes.
pub struct Sequenced {
    pub seq: Seq,
    pub ts: Ts,
    pub cmd: Command,
}
```

---

## 3. Output events (engine output / audit / market data)

```rust
pub enum OutputEvent {
    Accepted   { id: OrderId, seq: Seq },
    Rejected   { id: OrderId, reason: RejectReason, seq: Seq },
    Trade {                       // one per matched pair
        seq: Seq,
        taker: OrderId,
        maker: OrderId,
        price: Price,             // maker (resting) order sets the price
        qty: Qty,
        side: Side,               // aggressor side
        ts: Ts,
    },
    PartiallyFilled { id: OrderId, filled: Qty, remaining: Qty, seq: Seq },
    Filled          { id: OrderId, seq: Seq },
    Cancelled       { id: OrderId, seq: Seq },
    Replaced        { id: OrderId, seq: Seq },
}

pub enum RejectReason {
    UnknownSymbol, DuplicateId, UnknownId, ZeroQty, BadPrice,
    FokUnfillable, MarketNoLiquidity, SelfTrade, RateLimited,
}
```

Every event carries the originating `seq` so the WAL, market-data, and client-response
streams can be correlated and ordered.

---

## 4. The OrderBook API (`core`)

```rust
pub struct OrderBook { /* bids, asks, id_index, arena, stops, last_trade */ }

impl OrderBook {
    pub fn new(symbol: SymbolId, tick: Price) -> Self;

    /// THE hot path. Pure: same (state, cmd) → same (events, new state).
    /// Mutates self; returns events. No I/O, no clock reads, no logging.
    pub fn apply(&mut self, cmd: &Sequenced, out: &mut Vec<OutputEvent>);

    // read-only accessors for market-data / metrics (no mutation)
    pub fn best_bid(&self) -> Option<(Price, Qty)>;
    pub fn best_ask(&self) -> Option<(Price, Qty)>;
    pub fn depth(&self, levels: usize) -> BookSnapshot;   // top-N each side
    pub fn order(&self, id: OrderId) -> Option<&Order>;
}
```

Internal layout (illustrative):
```rust
struct OrderBook {
    asks: BTreeMap<Price, PriceLevel>,            // ascending; best = first
    bids: BTreeMap<Reverse<Price>, PriceLevel>,   // descending; best = first
    id_index: FxHashMap<OrderId, Location>,       // O(1) cancel/replace
    arena: Slab<OrderSlot>,                        // stable indices, no per-order alloc
    stops: BTreeMap<Price, Vec<OrderId>>,         // resting stop triggers
    last_trade: Option<Price>,
}
struct PriceLevel { head: SlotIdx, tail: SlotIdx, total_qty: Qty }  // intrusive FIFO
struct Location { price: Price, side: Side, slot: SlotIdx }
```

---

## 5. Binary wire protocol (order entry)

Fixed-size little-endian frames, ITCH/OUCH-inspired. All messages start with a 1-byte type
tag. Lengths are implicit per type (fixed) for zero-copy decode.

### Inbound (client → exchange)

```
NewOrder  (type = 0x4E 'N'), 42 bytes:
  off  size  field
  0    1     msg_type = 'N'
  1    8     order_id        u64
  2    4     symbol_id       u32
  3    1     side            u8   (0=Buy,1=Sell)
  4    1     order_type      u8   (0=Limit,1=Market,2=IOC,3=FOK,4=StopMkt,5=StopLmt)
  5    1     tif             u8
  6    8     price           i64  (ticks; 0 if market)
  7    8     stop_price      i64  (ticks; 0 if not stop)
  8    8     qty             u64
  ─ total 1+8+4+1+1+1+8+8+8 = 40 bytes (+2 pad → 42, 8-byte align)

Cancel    (type = 0x43 'C'), 16 bytes:
  msg_type='C' | order_id u64 | symbol_id u32 | pad u24

Replace   (type = 0x52 'R'), 32 bytes:
  msg_type='R' | order_id u64 | symbol_id u32 | new_price i64 | new_qty u64
```

### Outbound (exchange → client)

```
Ack       (type = 0x41 'A'): order_id u64 | seq u64 | status u8
Reject    (type = 0x4A 'J'): order_id u64 | seq u64 | reason u8
Fill      (type = 0x46 'F'): order_id u64 | seq u64 | price i64 | qty u64 | leaves u64 | maker_or_taker u8
Cancelled (type = 0x58 'X'): order_id u64 | seq u64
```

**Design rules:** fixed offsets → branch-free, zero-copy decode (`rkyv`/manual). Little-endian
matches x86. Reserve a `version` byte in a session-hello if you want forward compat.

### JSON gateway (same logical messages, for easy demo)
```json
// inbound
{"t":"new","id":1001,"symbol":"AAPL","side":"buy","type":"limit","tif":"gtc","price":150.25,"qty":100}
{"t":"cancel","id":1001,"symbol":"AAPL"}
// outbound
{"t":"ack","id":1001,"seq":55012}
{"t":"trade","taker":1001,"maker":900,"price":150.25,"qty":50,"seq":55013}
{"t":"fill","id":1001,"price":150.25,"qty":50,"leaves":50,"seq":55013}
```
JSON prices are human dollars; the gateway converts to/from ticks at the boundary. Binary
protocol carries ticks directly.

### FIX 4.2 subset (optional, talking point)
- `35=D` NewOrderSingle → `Command::New`. Tags: 11(ClOrdID), 55(Symbol), 54(Side),
  38(OrderQty), 40(OrdType), 44(Price), 59(TimeInForce).
- `35=8` ExecutionReport ← `OutputEvent`. Tags: 37(OrderID), 39(OrdStatus), 150(ExecType),
  32(LastQty), 31(LastPx), 151(LeavesQty).
- `35=F` OrderCancelRequest → `Command::Cancel`.

---

## 6. Market-data feed (WebSocket)

### Subscribe
```json
{"op":"subscribe","channels":["trades:AAPL","book:AAPL"]}
```

### Book snapshot (sent on subscribe + periodically)
```json
{"t":"snapshot","symbol":"AAPL","seq":55013,
 "bids":[[150.20,300],[150.15,1200]],   // [price, qty], best first
 "asks":[[150.25,150],[150.30,800]]}
```

### Book delta (incremental, ordered by seq)
```json
{"t":"delta","symbol":"AAPL","seq":55014,
 "bids":[[150.20,250]],     // new qty at price; 0 qty = level removed
 "asks":[[150.25,0]]}       // ask 150.25 fully consumed/removed
```

### Trade tape
```json
{"t":"trade","symbol":"AAPL","price":150.25,"qty":150,"side":"buy","seq":55013,"ts":172...}
```

**Gap recovery:** every message carries `seq`. A client that detects a gap (`seq` jump)
requests/awaits a fresh snapshot. UDP-multicast variant relies on this for lossy transport.

---

## 7. WAL record format

```
record:  len u32 | crc32 u32 | seq u64 | ts u64 | rec_type u8 | payload[...]
rec_type: 0 = Command (command-sourcing)  OR  1 = Event (event-sourcing)  // pick one, ADR-003
segment file: 0000000001.wal, rolls at SEGMENT_BYTES; replay reads in filename order
snapshot file: snapshot-<seq>.bin  = serialized full book state + last_applied_seq
```
`len`+`crc` frame each record so a torn final write (crash mid-append) is detected and
truncated on recovery rather than corrupting replay.

---

## 8. Engine / runtime API (`engine` crate)

```rust
pub struct Engine { /* shards, sequencer, egress */ }

impl Engine {
    pub fn start(cfg: EngineConfig) -> Engine;          // spawns + pins shard threads, WAL writer
    pub fn submit(&self, cmd: Command) -> Result<(), Backpressure>;  // routes to shard ring buffer
    pub fn subscribe_events(&self) -> broadcast::Receiver<OutputEvent>;
    pub fn shutdown(self);                              // drain, fsync WAL, join threads
}

pub struct EngineConfig {
    pub num_shards: usize,
    pub symbols: Vec<(SymbolId, /*tick*/ Price)>,
    pub ring_capacity: usize,        // bounded → back-pressure
    pub wal_dir: PathBuf,
    pub fsync: FsyncPolicy,          // PerRecord | GroupCommit{ window } | Off(test only)
    pub pin_cores: bool,
}
```

`shard_of(symbol) = stable_hash(symbol) % num_shards`. A symbol is owned by exactly one shard
for the process lifetime (so ordering/determinism per symbol holds).

---

## 9. ML sidecar interface (external, decoupled)

The sidecar is *just another market-data client* and *just another order-entry client*.

```
INPUT : WS market-data feed  (snapshots + deltas + trade tape)  → features
OUTPUT: (optional) orders via the same gateway as any client
```

Example published signal (sidecar → its own WS topic / file, NOT into the engine):
```json
{"t":"signal","symbol":"AAPL","mid":150.225,"micro_price":150.231,
 "pred_next_move":"up","prob":0.61,"vwap":150.19,"ts":172...}
{"t":"anomaly","symbol":"AAPL","kind":"quote_stuffing","score":7.4,"window_ms":50}
```
No privileged engine hooks — this keeps the engine pure and the ML layer honestly evaluable
on the public feed.

---

## 10. Metrics surface (Prometheus)

```
exchange_orders_total{symbol,type}            counter
exchange_trades_total{symbol}                 counter
exchange_rejects_total{reason}                counter
exchange_match_latency_seconds{stage}         histogram (hdr-backed) → p50/p99/p999
exchange_book_depth{symbol,side}              gauge
exchange_spread_ticks{symbol}                 gauge
exchange_ring_occupancy{shard}                gauge  (back-pressure visibility)
```

---

## 11. Invariants the interfaces must uphold (cross-ref `plan.md` §7)
- `Price` is integer ticks everywhere inside the engine; dollars only at the JSON edge.
- Every `OutputEvent` carries the originating `seq`.
- `OrderBook::apply` is pure (no clock/IO) → replay determinism.
- Bounded queues everywhere on the control plane → overload = back-pressure, not OOM.
- Maker order sets the trade price; aggressor crosses the spread.
