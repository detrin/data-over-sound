"""Link-adaptation strategies: choose per-tile levels for the next frame from receiver feedback.

Feedback per frame: the levels used, per-tile block success, and per-tile SNR measured on the
DBPSK header symbol (available for every tile, including tiles the map switched off).

Strategies
- Fixed:           one level everywhere (baseline)
- SnrThreshold:    per tile, the highest level whose calibrated 90%-success SNR (+ margin) is met (ADSL-like)
- Olla:            SnrThreshold + per-tile outer-loop margin driven by block outcomes (LTE OLLA, target BLER)
- Minstrel:        EWMA success probability per tile and level, exploit the best expected throughput,
                   spend ~10% of tiles on random "lookaround" levels (Linux Wi-Fi rate control)
- ThompsonSW:      Thompson sampling over levels per tile with discounted Beta counts (non-stationary
                   bandit), optional SNR-informed prior from the calibrated success model
"""
from abc import ABC, abstractmethod

import numpy as np

from .calibrate import load as load_snr_model
from .frame import block_layout
from .phy import PhyConfig, MAX_LEVEL

LEVELS = np.arange(MAX_LEVEL + 1)


class Controller(ABC):
    name = "base"

    def __init__(self, cfg: PhyConfig, seed: int = 0):
        self.cfg = cfg
        self.rng = np.random.default_rng(seed)
        self.n = cfg.n_tiles
        # payload bytes a tile carries at each level (the reward scale)
        self.cap = np.array([block_layout(cfg, int(l))[1] for l in LEVELS], float)

    @abstractmethod
    def choose(self) -> np.ndarray:
        """Levels for the next frame."""

    @abstractmethod
    def observe(self, levels: np.ndarray, ok: np.ndarray, snr_db: np.ndarray):
        """Feedback for the last frame (snr_db may be NaN if the frame was lost)."""


class Fixed(Controller):
    def __init__(self, cfg, level: int, **kw):
        super().__init__(cfg, **kw)
        self.level, self.name = level, f"fixed-L{level}"

    def choose(self):
        return np.full(self.n, self.level)

    def observe(self, levels, ok, snr_db):
        pass


class _SnrBased(Controller):
    """Shared SNR tracking: EWMA of the header SNR per tile; a lost frame counts as very low SNR."""

    def __init__(self, cfg, alpha: float = 0.6, lost_snr_db: float = 0.0, online_cal: bool = False,
                 cal_lr: float = 0.5, **kw):
        super().__init__(cfg, **kw)
        self.model = load_snr_model()
        self.alpha, self.lost = alpha, lost_snr_db
        self.snr = np.full(self.n, np.nan)
        # online re-calibration: per-level SNR offset learnt from block outcomes (contextual bandit).
        # The offline model comes from the simulator; the real direction may be a few dB off.
        self.online_cal, self.cal_lr = online_cal, cal_lr
        self.offset = np.zeros(len(LEVELS))

    def _track(self, snr_db):
        s = np.where(np.isfinite(snr_db), snr_db, self.lost)
        self.snr = np.where(np.isfinite(self.snr), self.alpha * s + (1 - self.alpha) * self.snr, s)

    def p_success(self, snr_db, margin=0.0):
        """Calibrated success probability per tile (rows) and level (cols, level 0 = 1)."""
        p = np.ones((len(snr_db), len(LEVELS)))
        for lev in LEVELS[1:]:
            m = self.model[int(lev)]
            z = np.clip((np.nan_to_num(snr_db, nan=-10) + self.offset[lev] - margin - m["mid"]) / m["scale"], -30, 30)
            p[:, lev] = (1 - m["lapse"]) / (1 + np.exp(-z))
        return p

    def _calibrate(self, snr_before, levels, ok):
        """One Newton-like logistic-regression step per level on this frame's tile outcomes:
        d logL / d offset = (y - p) / scale, var(p) <= 1/4  =>  step = lr * 4 * scale * mean(y - p)."""
        if not self.online_cal or not np.isfinite(snr_before).any():
            return
        p = self.p_success(snr_before)
        for lev in LEVELS[1:]:
            used = levels == lev
            if used.sum() >= 4:
                err = np.mean(ok[used] - p[used, lev])
                self.offset[lev] = np.clip(self.offset[lev] + self.cal_lr * 4 * self.model[int(lev)]["scale"] * err, -8, 8)

    def threshold_levels(self, margin):
        """Highest level per tile whose 90%-success SNR (+ margin) is met."""
        lv = np.zeros(self.n, int)
        s = np.nan_to_num(self.snr, nan=-10)
        for lev in LEVELS[1:]:
            lv = np.where(s - margin >= self.model[int(lev)]["thr90"], lev, lv)
        return lv


class SnrThreshold(_SnrBased):
    def __init__(self, cfg, margin_db: float = 1.0, **kw):
        super().__init__(cfg, **kw)
        self.margin, self.name = margin_db, f"snr-thr+{margin_db:g}dB"

    def choose(self):
        return self.threshold_levels(self.margin) if np.isfinite(self.snr).any() else np.full(self.n, 1)

    def observe(self, levels, ok, snr_db):
        self._track(snr_db)


class SnrExpected(_SnrBased):
    """Greedy: level with the best expected throughput under the calibrated SNR model (no outcome learning)."""
    name = "snr-expected"

    def __init__(self, cfg, **kw):
        super().__init__(cfg, **kw)
        self.name = "snr-expected" + ("+cal" if self.online_cal else "")

    def choose(self):
        if not np.isfinite(self.snr).any():
            return np.full(self.n, 1)
        return np.argmax(self.p_success(self.snr) * self.cap, axis=1)

    def observe(self, levels, ok, snr_db):
        self._calibrate(self.snr, levels, ok)
        self._track(snr_db)


class Olla(_SnrBased):
    """Expected-throughput choice with a per-tile outer-loop margin: +up on a block error,
    -up*BLER/(1-BLER) on success, so each tile converges to the target block error rate."""

    def __init__(self, cfg, target_bler: float = 0.1, step_db: float = 2.0, **kw):
        super().__init__(cfg, **kw)
        self.name = f"olla-{int(target_bler * 100)}%"
        self.up, self.down = step_db, step_db * target_bler / (1 - target_bler)
        self.margin = np.zeros(self.n)

    def choose(self):
        if not np.isfinite(self.snr).any():
            return np.full(self.n, 1)
        p = self.p_success(self.snr - self.margin)
        return np.argmax(p * self.cap, axis=1)

    def observe(self, levels, ok, snr_db):
        self._track(snr_db)
        used = levels > 0
        self.margin[used & ~ok] += self.up
        self.margin[used & ok] -= self.down
        self.margin = np.clip(self.margin, -3, 15)


class Minstrel(Controller):
    """Per tile and level: EWMA success probability; exploit argmax(cap * p); ~10% lookaround."""

    def __init__(self, cfg, ewma: float = 0.75, lookaround: float = 0.1, **kw):
        super().__init__(cfg, **kw)
        self.name = "minstrel"
        self.w, self.look = ewma, lookaround
        self.p = np.tile(np.array([1.0, 0.9, 0.7, 0.5]), (self.n, 1))   # optimistic start

    def choose(self):
        lv = np.argmax(self.p * self.cap, axis=1)
        explore = self.rng.random(self.n) < self.look
        lv[explore] = self.rng.integers(1, MAX_LEVEL + 1, explore.sum())
        return lv

    def observe(self, levels, ok, snr_db):
        idx = np.arange(self.n)
        used = levels > 0
        self.p[idx[used], levels[used]] = self.w * self.p[idx[used], levels[used]] + (1 - self.w) * ok[used]


class ThompsonSW(_SnrBased):
    """Thompson sampling per tile over levels with discounted Beta(success, failure) counts.

    snr_prior: add the calibrated SNR model as `prior_weight` pseudo-observations per frame, so the
    fresh header SNR (free exploration of every tile) and the outcome statistics are combined.
    Sampled success probabilities are made non-increasing in the level (higher order never more
    reliable), then the level with the best sampled throughput is used."""

    def __init__(self, cfg, gamma: float = 0.7, snr_prior: bool = True, prior_weight: float = 3.0, **kw):
        super().__init__(cfg, **kw)
        self.gamma, self.use_prior, self.pw = gamma, snr_prior, prior_weight
        self.name = f"thompson{'+snr' if snr_prior else ''}{'+cal' if self.online_cal else ''}-g{gamma:g}"
        self.s = np.zeros((self.n, len(LEVELS)))
        self.f = np.zeros((self.n, len(LEVELS)))

    def choose(self):
        a, b = 1 + self.s, 1 + self.f
        if self.use_prior and np.isfinite(self.snr).any():
            p0 = self.p_success(self.snr)
            a, b = a + self.pw * p0, b + self.pw * (1 - p0)
        theta = self.rng.beta(a, b)
        theta[:, 0] = 1.0
        theta = np.minimum.accumulate(theta, axis=1)
        return np.argmax(theta * self.cap, axis=1)

    def observe(self, levels, ok, snr_db):
        self._calibrate(self.snr, levels, ok)
        self._track(snr_db)
        self.s *= self.gamma
        self.f *= self.gamma
        idx = np.arange(self.n)
        used = levels > 0
        self.s[idx[used], levels[used]] += ok[used]
        self.f[idx[used], levels[used]] += ~ok[used]


def finalists(cfg: PhyConfig, seed: int = 0):
    """Round 2 (after the first over-the-air comparison): with and without online calibration."""
    return [
        Fixed(cfg, 3, seed=seed),
        ThompsonSW(cfg, gamma=0.7, snr_prior=True, seed=seed),
        ThompsonSW(cfg, gamma=0.7, snr_prior=True, online_cal=True, seed=seed),
        SnrExpected(cfg, online_cal=True, seed=seed),
    ]


def all_strategies(cfg: PhyConfig, seed: int = 0):
    return [
        Fixed(cfg, 2, seed=seed),
        Fixed(cfg, 3, seed=seed),
        SnrThreshold(cfg, 0.0, seed=seed),
        SnrThreshold(cfg, 3.0, seed=seed),
        Olla(cfg, 0.1, seed=seed),
        Minstrel(cfg, seed=seed),
        SnrExpected(cfg, seed=seed),
        ThompsonSW(cfg, snr_prior=False, seed=seed),
        ThompsonSW(cfg, snr_prior=True, seed=seed),
        ThompsonSW(cfg, gamma=0.5, snr_prior=True, seed=seed),
        ThompsonSW(cfg, gamma=0.85, snr_prior=True, seed=seed),
    ]
