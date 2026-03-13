"""IBKR fee model — extracted from the overnight/intraday drift strategies."""

from __future__ import annotations

# IBKR Tiered fee model (US equities)
IBKR_PER_SHARE = 0.0035  # commission
IBKR_EXCHANGE_REG = 0.0030  # exchange + regulatory
IBKR_TOTAL_PER_SHARE = IBKR_PER_SHARE + IBKR_EXCHANGE_REG
IBKR_MIN_ORDER = 0.35
IBKR_MAX_PCT = 0.01  # 1% of trade value


def ibkr_roundtrip_cost(equity: float, price: float) -> float:
    """IBKR tiered round-trip cost for a fully-invested position.

    Returns total dollar cost for buy + sell.
    """
    shares = int(equity / price)
    if shares <= 0:
        return 0.0

    def one_side(n_shares: float, trade_value: float) -> float:
        raw = n_shares * IBKR_TOTAL_PER_SHARE
        raw = max(raw, IBKR_MIN_ORDER)
        raw = min(raw, IBKR_MAX_PCT * trade_value)
        return raw

    trade_value = shares * price
    return one_side(shares, trade_value) + one_side(shares, trade_value)
