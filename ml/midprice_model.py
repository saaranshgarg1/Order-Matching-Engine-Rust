"""
Mid-price direction predictor.
Features: book imbalance, spread, recent trade direction, micro-price.
Label: next mid-price move (up=1, down=0).
Model: LightGBM (or logistic regression as fallback).
"""

import logging
from collections import deque
from typing import Deque, List, Optional

import numpy as np

log = logging.getLogger(__name__)

try:
    import lightgbm as lgb
    _HAS_LGB = True
except ImportError:
    _HAS_LGB = False
    from sklearn.linear_model import LogisticRegression


def micro_price(bid_px: float, ask_px: float, bid_qty: float, ask_qty: float) -> float:
    """Qty-weighted mid: micro = ask*(bid_qty/(bid+ask)) + bid*(ask_qty/(bid+ask))."""
    total = bid_qty + ask_qty
    if total == 0:
        return (bid_px + ask_px) / 2.0
    return ask_px * (bid_qty / total) + bid_px * (ask_qty / total)


def extract_features(book, trades: Deque) -> Optional[np.ndarray]:
    """Return feature vector or None if not enough data."""
    if book is None or not book.bids or not book.asks:
        return None

    bid0 = book.bids[0]
    ask0 = book.asks[0]
    mid  = (bid0.price + ask0.price) / 2.0
    sprd = ask0.price - bid0.price

    imb  = book.imbalance() or 0.0
    mp   = micro_price(bid0.price, ask0.price, bid0.qty, ask0.qty)
    mp_offset = (mp - mid) / sprd if sprd > 0 else 0.0

    # recent trade direction: fraction of last 20 trades that were buys
    recent = list(trades)[-20:]
    buy_frac = (sum(1 for t in recent if t.side == "buy") / len(recent)) if recent else 0.5

    # trade size imbalance (buy_vol - sell_vol) / total_vol in last 20
    buy_vol  = sum(t.qty for t in recent if t.side == "buy")
    sell_vol = sum(t.qty for t in recent if t.side == "sell")
    vol_total = buy_vol + sell_vol
    vol_imb  = (buy_vol - sell_vol) / vol_total if vol_total > 0 else 0.0

    return np.array([imb, sprd, mp_offset, buy_frac, vol_imb], dtype=np.float32)


class MidPricePredictor:
    """
    Online predictor. Collects (features, label) pairs from the live feed,
    retrains every `retrain_every` samples, then predicts on incoming data.
    """

    def __init__(self, retrain_every: int = 500, min_samples: int = 200):
        self.retrain_every = retrain_every
        self.min_samples   = min_samples
        self._X: List[np.ndarray] = []
        self._y: List[int]        = []
        self._last_mid: Optional[float] = None
        self._last_feat: Optional[np.ndarray] = None
        self._model = None
        self._n_updates = 0

    def update(self, book, trades: Deque) -> Optional[dict]:
        """
        Called on every book event. Returns a signal dict or None.
        Signal: {"pred": "up"|"down", "prob": float, "micro_price": float, "mid": float}
        """
        feat = extract_features(book, trades)
        if feat is None:
            return None

        mid = (book.bids[0].price + book.asks[0].price) / 2.0
        mp  = micro_price(
            book.bids[0].price, book.asks[0].price,
            book.bids[0].qty,   book.asks[0].qty,
        )

        # Label the *previous* observation with the direction mid moved since then.
        if self._last_mid is not None and self._last_feat is not None:
            label = 1 if mid > self._last_mid else 0
            self._X.append(self._last_feat)
            self._y.append(label)
            self._n_updates += 1

            if (self._n_updates % self.retrain_every == 0
                    and len(self._X) >= self.min_samples):
                self._retrain()

        self._last_mid  = mid
        self._last_feat = feat

        if self._model is None or len(self._X) < self.min_samples:
            return None

        prob = self._predict_prob(feat)
        return {
            "pred":        "up" if prob >= 0.5 else "down",
            "prob":        round(float(prob), 4),
            "micro_price": round(mp, 4),
            "mid":         round(mid, 4),
        }

    def _retrain(self) -> None:
        X = np.array(self._X[-5000:])   # rolling window cap
        y = np.array(self._y[-5000:])
        if len(np.unique(y)) < 2:
            return  # can't fit with one class
        try:
            if _HAS_LGB:
                ds = lgb.Dataset(X, label=y)
                params = {
                    "objective":  "binary",
                    "metric":     "binary_logloss",
                    "num_leaves": 15,
                    "learning_rate": 0.05,
                    "verbose":    -1,
                    "n_jobs":     1,
                }
                self._model = lgb.train(params, ds, num_boost_round=50,
                                        valid_sets=[ds], callbacks=[lgb.log_evaluation(-1)])
            else:
                m = LogisticRegression(max_iter=200, C=1.0)
                m.fit(X, y)
                self._model = m
            log.info("Model retrained on %d samples", len(X))
        except Exception as exc:
            log.warning("Retrain failed: %s", exc)

    def _predict_prob(self, feat: np.ndarray) -> float:
        if self._model is None:
            return 0.5
        try:
            x = feat.reshape(1, -1)
            if _HAS_LGB:
                return float(self._model.predict(x)[0])
            else:
                return float(self._model.predict_proba(x)[0, 1])
        except Exception:
            return 0.5
