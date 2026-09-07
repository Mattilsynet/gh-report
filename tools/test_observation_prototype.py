"""Independent assertions for the offline model, never production health checks."""

import unittest
from datetime import timedelta as D

from observation_prototype import (
    BeyondDeclaredWait, FakeClock, IdleUntil, Instant, ProgressAge, Reason,
    Running, Stopped, Unknown, WaitingUntil, WithinDeclaredWait,
    observe, scheduler_step, worker_progress,
)


class ObservationTests(unittest.TestCase):
    def test_instant_after_rejects_negative_before_construction(self):
        instant = Instant(D(hours=1))
        with self.assertRaisesRegex(ValueError, "backward"):
            instant.after(D(microseconds=-1))
        self.assertEqual(instant.after(D()), instant)
        self.assertEqual(instant.after(D(seconds=1)), Instant(D(hours=1, seconds=1)))

    def test_observation_retains_idle_quota_and_sweep_reason(self):
        deadline = Instant(D(hours=1))
        for state, expected in [(IdleUntil(deadline), "IDLE"),
                                (WaitingUntil(deadline, Reason.QUOTA), "QUOTA"),
                                (WaitingUntil(deadline, Reason.SWEEP_BATCH), "SWEEP_BATCH")]:
            for now in [Instant(D()), deadline, deadline.after(D(seconds=1))]:
                with self.subTest(expected=expected, now=now):
                    self.assertEqual(observe(state, now).reason.name, expected)

    def test_hour_idle_observation_never_reschedules(self):
        clock = FakeClock()
        idle = IdleUntil(clock.now.after(D(hours=1)))
        for _ in range(60):
            clock.advance(D(minutes=1))
            self.assertIsInstance(observe(idle, clock.now), WithinDeclaredWait)
        self.assertEqual(idle.deadline, Instant(D(hours=1)))
        self.assertEqual(scheduler_step(idle, clock.now), Running(clock.now))

    def test_declared_wait_boundaries_not_progress_age_thresholds(self):
        for reason, duration in [(Reason.QUOTA, D(hours=1)),
                                 (Reason.QUOTA, D(seconds=3605)),
                                 (Reason.QUOTA, D(seconds=86405)),
                                 (Reason.RETRY_AFTER, D(hours=3)),
                                 (Reason.SWEEP_BATCH, D(hours=2)),
                                 (Reason.FENCE_BACKOFF, D(seconds=2))]:
            with self.subTest(reason=reason, duration=duration):
                clock = FakeClock()
                state = WaitingUntil(clock.now.after(duration), reason)
                clock.advance(duration)
                self.assertEqual(observe(state, clock.now), WithinDeclaredWait(D(), reason))
                clock.advance(D(microseconds=1))
                self.assertEqual(observe(state, clock.now),
                                 BeyondDeclaredWait(D(microseconds=1), reason))
                self.assertEqual(state.deadline, Instant(duration))

    def test_pending_two_hour_batch_does_not_prove_worker_progress(self):
        clock = FakeClock()
        worker = Running(clock.now)
        supervisor = WaitingUntil(clock.now.after(D(hours=2)), Reason.SWEEP_BATCH)
        clock.advance(D(hours=2))
        self.assertEqual(observe(supervisor, clock.now), WithinDeclaredWait(D(), Reason.SWEEP_BATCH))
        self.assertEqual(observe(worker, clock.now), ProgressAge(D(hours=2)))
        self.assertEqual(worker.last_progress, Instant(D()))

    def test_sibling_and_reads_cannot_advance_worker(self):
        clock = FakeClock()
        worker = Running(clock.now)
        sibling = Running(clock.now)
        for _ in range(120):
            clock.advance(D(minutes=1))
            sibling = worker_progress(sibling, clock.now)
            observe(worker, clock.now)
            observe(sibling, clock.now)
        self.assertEqual(worker.last_progress, Instant(D()))
        self.assertEqual(observe(worker, clock.now), ProgressAge(D(hours=2)))
        self.assertEqual(sibling.last_progress, clock.now)
        self.assertEqual(worker_progress(worker, clock.now), Running(clock.now))

    def test_cancellation_wins_at_due_time_and_stays_stopped(self):
        clock = FakeClock()
        idle = IdleUntil(clock.now.after(D(hours=1)))
        self.assertEqual(scheduler_step(idle, clock.now, True), Stopped())
        clock.advance(D(hours=1))
        stopped = scheduler_step(idle, clock.now, True)
        self.assertEqual(stopped, Stopped())
        clock.advance(D(hours=2))
        self.assertEqual(scheduler_step(stopped, clock.now), Stopped())
        self.assertEqual(worker_progress(stopped, clock.now), Stopped())
        self.assertEqual(observe(stopped, clock.now), Stopped())

    def test_unknown_remains_unknown_including_new_process(self):
        clock = FakeClock()
        clock.advance(D(days=1))
        self.assertEqual(observe(Unknown(), clock.now), Unknown())
        self.assertNotEqual(observe(Unknown(), clock.now), WithinDeclaredWait(D(), Reason.QUOTA))

    def test_monotonic_clock_rejects_reversal(self):
        with self.assertRaises(ValueError):
            FakeClock().advance(D(seconds=-1))


if __name__ == "__main__":
    unittest.main()
