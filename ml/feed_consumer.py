"""
Consumes the exchange WebSocket market-data feed.
Maintains a rolling feature window and calls registered handlers.
"""

import asyncio
import json
import logging
from collections import deque
from dataclasses import dataclass, field
from typing import Callable, Deque, List, Optional

import websockets

log = logging.getLogger(__name__)


@dataclass
class Trade:
    symbol: str
    price:  float
    qty:    int
    side:   str   # "buy" | "sell"
    seq:    int
    ts:     int


@dataclass
class BookLevel:
    price: float
    qty:   float


@dataclass
class BookState:
    symbol: str
    seq:    int
    bids:   List[BookLevel] = field(default_factory=list)
    asks:   List[BookLevel] = field(default_factory=list)

    def mid_price(self) -> Optional[float]:
        if self.bids and self.asks:
            return (self.bids[0].price + self.asks[0].price) / 2.0
        return None

    def spread(self) -> Optional[float]:
        if self.bids and self.asks:
            return self.asks[0].price - self.bids[0].price
        return None

    def imbalance(self) -> Optional[float]:
        """Order-book imbalance = (bid_qty - ask_qty) / (bid_qty + ask_qty)."""
        if not self.bids or not self.asks:
            return None
        bq = sum(l.qty for l in self.bids[:5])
        aq = sum(l.qty for l in self.asks[:5])
        total = bq + aq
        return (bq - aq) / total if total > 0 else 0.0


class FeedConsumer:
    """
    Connects to ws://host:port, dispatches events to registered handlers.
    Maintains rolling deque of last N trades and current book state.
    """

    def __init__(self, uri: str, symbols: List[str], window: int = 200):
        self.uri     = uri
        self.symbols = symbols
        self.window  = window

        self.trades:     Deque[Trade]     = deque(maxlen=window)
        self.book:       Optional[BookState] = None
        self._handlers:  List[Callable]   = []

    def register(self, handler: Callable) -> None:
        self._handlers.append(handler)

    def _dispatch(self, event: dict) -> None:
        for h in self._handlers:
            try:
                h(event, self.trades, self.book)
            except Exception as exc:
                log.warning("handler %s raised: %s", h.__name__, exc)

    def _handle_raw(self, msg: str) -> None:
        try:
            ev = json.loads(msg)
        except json.JSONDecodeError:
            return

        t = ev.get("t")

        if t == "trade":
            trade = Trade(
                symbol=ev.get("symbol", ""),
                price=float(ev.get("price", 0)),
                qty=int(ev.get("qty", 0)),
                side=ev.get("side", ""),
                seq=int(ev.get("seq", 0)),
                ts=int(ev.get("ts", 0)),
            )
            self.trades.append(trade)
            self._dispatch(ev)

        elif t in ("snapshot", "delta"):
            bids = [BookLevel(b[0], b[1]) for b in ev.get("bids", [])]
            asks = [BookLevel(a[0], a[1]) for a in ev.get("asks", [])]
            self.book = BookState(
                symbol=ev.get("symbol", ""),
                seq=int(ev.get("seq", 0)),
                bids=bids,
                asks=asks,
            )
            self._dispatch(ev)

    async def run(self) -> None:
        backoff = 1
        while True:
            try:
                async with websockets.connect(self.uri) as ws:
                    log.info("Connected to %s", self.uri)
                    backoff = 1
                    # Subscribe to all symbols
                    channels = []
                    for sym in self.symbols:
                        channels += [f"trades:{sym}", f"book:{sym}"]
                    await ws.send(json.dumps({"op": "subscribe", "channels": channels}))

                    async for msg in ws:
                        self._handle_raw(msg)

            except (websockets.ConnectionClosed, OSError) as exc:
                log.warning("Feed disconnected (%s), retrying in %ds", exc, backoff)
                await asyncio.sleep(backoff)
                backoff = min(backoff * 2, 30)
