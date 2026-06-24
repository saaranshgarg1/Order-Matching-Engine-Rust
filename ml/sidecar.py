#!/usr/bin/env python3
"""
ML sidecar entry point.
Consumes market-data feed → runs mid-price predictor + anomaly detector
→ prints signals (JSON lines) to stdout and optionally to a signal file.
"""

import asyncio
import json
import logging
import sys
from datetime import datetime

from feed_consumer import FeedConsumer, Trade
from midprice_model import MidPricePredictor
from anomaly import AnomalyDetector

logging.basicConfig(
    level=logging.INFO,
    format="%(asctime)s %(levelname)s %(name)s: %(message)s",
)
log = logging.getLogger("sidecar")

WS_URI   = "ws://127.0.0.1:9002"
SYMBOLS  = ["AAPL", "MSFT", "TSLA", "GOOG"]
OUT_FILE = "signals.jsonl"


def emit(signal: dict) -> None:
    signal["ts"] = datetime.utcnow().isoformat()
    line = json.dumps(signal)
    print(line, flush=True)
    with open(OUT_FILE, "a") as f:
        f.write(line + "\n")


def make_handler(predictor: MidPricePredictor, detector: AnomalyDetector):
    def handler(event: dict, trades, book) -> None:
        t = event.get("t")

        # mid-price prediction on every book update
        if t in ("snapshot", "delta") and book is not None:
            sig = predictor.update(book, trades)
            if sig:
                emit({"type": "signal", "symbol": book.symbol, **sig})

        # anomaly detection on every trade
        if t == "trade" and trades:
            last_trade = trades[-1]
            alerts = detector.update(last_trade, book)
            for a in alerts:
                emit({
                    "type":   "anomaly",
                    "symbol": last_trade.symbol,
                    "kind":   a.kind,
                    "score":  a.score,
                    "detail": a.detail,
                })

    return handler


async def main() -> None:
    log.info("ML sidecar starting. Feed: %s  Symbols: %s", WS_URI, SYMBOLS)

    consumer  = FeedConsumer(uri=WS_URI, symbols=SYMBOLS)
    predictor = MidPricePredictor(retrain_every=500, min_samples=200)
    detector  = AnomalyDetector(window=100, threshold_sigma=4.0)

    consumer.register(make_handler(predictor, detector))

    try:
        await consumer.run()
    except KeyboardInterrupt:
        log.info("Sidecar stopped.")


if __name__ == "__main__":
    asyncio.run(main())
