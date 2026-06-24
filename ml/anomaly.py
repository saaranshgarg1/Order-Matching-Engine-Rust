"""
Order-flow anomaly detector.
Flags: quote stuffing (rapid place/cancel bursts), unusual size, momentum ignition.
Uses z-score on a rolling window — no ML dep required, interpretable.
"""

import logging
from collections import deque
from dataclasses import dataclass, field
from typing import Deque, List, Optional

import numpy as np

log = logging.getLogger(__name__)


@dataclass
class AnomalySignal:
    kind:   str    # "quote_stuffing" | "size_spike" | "momentum_ignition"
    score:  float  # sigma above rolling mean
    detail: str


class AnomalyDetector:
    """
    Rolling-window z-score detector. Each call to update() returns a list
    of AnomalySignal (empty if nothing flagged).
    """

    def __init__(self, window: int = 100, threshold_sigma: float = 4.0):
        self.window    = window
        self.threshold = threshold_sigma

        # rolling trade rates (trades per N-trade window)
        self._trade_sizes:   Deque[float] = deque(maxlen=window)
        self._trade_intervals: Deque[float] = deque(maxlen=window)  # ms between trades
        self._last_ts:       Optional[int]  = None

        # for momentum ignition: price move per unit volume
        self._price_moves:   Deque[float] = deque(maxlen=window)
        self._last_mid:      Optional[float] = None

    def update(self, trade, book) -> List[AnomalySignal]:
        signals: List[AnomalySignal] = []

        # ── size spike ──────────────────────────────────────────────────
        self._trade_sizes.append(float(trade.qty))
        if len(self._trade_sizes) >= 20:
            arr   = np.array(self._trade_sizes)
            mu, sigma = arr.mean(), arr.std()
            if sigma > 0:
                z = (trade.qty - mu) / sigma
                if z > self.threshold:
                    signals.append(AnomalySignal(
                        kind="size_spike",
                        score=round(z, 2),
                        detail=f"qty={trade.qty} is {z:.1f}σ above rolling mean {mu:.1f}",
                    ))

        # ── quote stuffing: very high trade frequency ───────────────────
        if self._last_ts is not None and trade.ts > 0:
            gap_ms = (trade.ts - self._last_ts) / 1_000_000.0  # ns → ms
            if gap_ms >= 0:
                self._trade_intervals.append(gap_ms)
                if len(self._trade_intervals) >= 20:
                    arr  = np.array(self._trade_intervals)
                    mu, sigma = arr.mean(), arr.std()
                    if sigma > 0 and gap_ms < 1:  # sub-ms interval
                        z = (mu - gap_ms) / sigma  # inverted: smaller = more anomalous
                        if z > self.threshold:
                            signals.append(AnomalySignal(
                                kind="quote_stuffing",
                                score=round(z, 2),
                                detail=f"inter-trade gap={gap_ms:.3f}ms, rolling mean={mu:.1f}ms",
                            ))
        self._last_ts = trade.ts if trade.ts > 0 else self._last_ts

        # ── momentum ignition: price moves faster than volume justifies ─
        if book is not None and book.bids and book.asks:
            mid = (book.bids[0].price + book.asks[0].price) / 2.0
            if self._last_mid is not None:
                move_per_vol = abs(mid - self._last_mid) / max(trade.qty, 1)
                self._price_moves.append(move_per_vol)
                if len(self._price_moves) >= 20:
                    arr  = np.array(self._price_moves)
                    mu, sigma = arr.mean(), arr.std()
                    if sigma > 0:
                        z = (move_per_vol - mu) / sigma
                        if z > self.threshold:
                            signals.append(AnomalySignal(
                                kind="momentum_ignition",
                                score=round(z, 2),
                                detail=f"price_move_per_vol={move_per_vol:.6f}, {z:.1f}σ above mean",
                            ))
            self._last_mid = mid

        return signals
