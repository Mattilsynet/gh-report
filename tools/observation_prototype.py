"""Offline decision prototype only: no monitor, IO, heartbeat, or alert policy."""

from dataclasses import dataclass
from datetime import timedelta
from enum import Enum, auto


@dataclass(frozen=True, order=True)
class Instant:
    elapsed: timedelta

    def __post_init__(self):
        if self.elapsed < timedelta():
            raise ValueError("elapsed time must be nonnegative")

    def after(self, duration):
        if duration < timedelta():
            raise ValueError("monotonic instant cannot move backward")
        return Instant(self.elapsed + duration)


@dataclass
class FakeClock:
    now: Instant = Instant(timedelta())

    def advance(self, duration):
        if duration < timedelta():
            raise ValueError("monotonic clock cannot move backward")
        self.now = self.now.after(duration)


class Reason(Enum):
    IDLE = auto()
    QUOTA = auto()
    RETRY_AFTER = auto()
    FENCE_BACKOFF = auto()
    SWEEP_BATCH = auto()


@dataclass(frozen=True)
class Unknown:
    pass


@dataclass(frozen=True)
class Stopped:
    pass


@dataclass(frozen=True)
class IdleUntil:
    deadline: Instant


@dataclass(frozen=True)
class WaitingUntil:
    deadline: Instant
    reason: Reason


@dataclass(frozen=True)
class Running:
    last_progress: Instant


@dataclass(frozen=True)
class WithinDeclaredWait:
    remaining: timedelta
    reason: Reason


@dataclass(frozen=True)
class BeyondDeclaredWait:
    overdue: timedelta
    reason: Reason


@dataclass(frozen=True)
class ProgressAge:
    age: timedelta


def observe(state, now):
    match state:
        case Unknown() | Stopped():
            return state
        case IdleUntil(deadline):
            return observe(WaitingUntil(deadline, Reason.IDLE), now)
        case WaitingUntil(deadline, reason):
            if now <= deadline:
                return WithinDeclaredWait(deadline.elapsed - now.elapsed, reason)
            return BeyondDeclaredWait(now.elapsed - deadline.elapsed, reason)
        case Running(last_progress):
            if now < last_progress:
                raise ValueError("observer clock is before last progress")
            return ProgressAge(now.elapsed - last_progress.elapsed)
        case _:
            raise TypeError("unsupported observation state")


def scheduler_step(state, now, cancelled=False):
    if cancelled or isinstance(state, Stopped):
        return Stopped()
    if isinstance(state, IdleUntil) and now >= state.deadline:
        return Running(now)
    return state


def worker_progress(state, now):
    if isinstance(state, Running):
        return Running(now)
    return state
