use crate::backend::RuntimeEvent;
use std::collections::VecDeque;
use std::time::{Duration, Instant, SystemTime};
use tracing::trace;

const NORMAL_ACTIVITY_IDLE_THRESHOLD: Duration = Duration::from_secs(10);

#[derive(Debug)]
pub(crate) struct ActivityPoller {
    poll_interval: Duration,
    last_sampled_at: Option<SystemTime>,
    events: VecDeque<RuntimeEvent>,
}

impl ActivityPoller {
    pub(crate) fn new(poll_interval: Duration) -> Self {
        Self {
            poll_interval,
            last_sampled_at: None,
            events: VecDeque::new(),
        }
    }

    pub(crate) const fn poll_interval(&self) -> Duration {
        self.poll_interval
    }

    pub(crate) fn queue_sample(&mut self, sample: ActivitySample) -> ActivityState {
        self.queue_sample_at(sample, SystemTime::now())
    }

    pub(crate) fn queue_sample_at(
        &mut self,
        sample: ActivitySample,
        sampled_at: SystemTime,
    ) -> ActivityState {
        let state = sample.state_for(NORMAL_ACTIVITY_IDLE_THRESHOLD);
        trace!(
            target: "rusteyes::activity",
            idle_for = ?sample.idle_for(),
            ?state,
            poll_interval = ?self.poll_interval,
            idle_threshold = ?NORMAL_ACTIVITY_IDLE_THRESHOLD,
            "sampled activity"
        );

        let unobserved_idle = self.unobserved_idle_since_last_sample(sampled_at);
        self.last_sampled_at = Some(sampled_at);

        self.queue_event(RuntimeEvent::WallClockElapsed(self.poll_interval));

        if let Some(unobserved_idle) = unobserved_idle {
            self.queue_event(RuntimeEvent::IdleTimeElapsed(unobserved_idle));
        }

        if state == ActivityState::Active {
            self.queue_event(RuntimeEvent::ActiveTimeElapsed(self.poll_interval));
        } else {
            self.queue_event(RuntimeEvent::IdleTimeElapsed(self.poll_interval));
        }

        state
    }

    fn unobserved_idle_since_last_sample(&self, sampled_at: SystemTime) -> Option<Duration> {
        let last_sampled_at = self.last_sampled_at?;
        let elapsed = sampled_at.duration_since(last_sampled_at).ok()?;
        if elapsed <= self.poll_interval {
            return None;
        }

        Some(elapsed.saturating_sub(self.poll_interval))
    }

    pub(crate) fn queue_event(&mut self, event: RuntimeEvent) {
        trace!(target: "rusteyes::activity", ?event, "queued runtime event");
        self.events.push_back(event);
    }

    pub(crate) fn next_event(&mut self) -> Option<RuntimeEvent> {
        self.events.pop_front()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct ActivitySample {
    idle_for: Duration,
}

impl ActivitySample {
    pub(crate) const fn new(idle_for: Duration) -> Self {
        Self { idle_for }
    }

    pub(crate) const fn idle_for(self) -> Duration {
        self.idle_for
    }

    pub(crate) fn state_for(self, idle_threshold: Duration) -> ActivityState {
        if self.idle_for <= idle_threshold {
            ActivityState::Active
        } else {
            ActivityState::Idle
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ActivityState {
    Active,
    Idle,
}

/// A monotonic and a wall-clock reading taken at the same moment. Break
/// deadlines follow the wall clock so time spent asleep counts toward the
/// break, while per-tick elapsed reporting stays monotonic.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct ObservedTime {
    pub(crate) monotonic: Instant,
    pub(crate) wall: SystemTime,
}

impl ObservedTime {
    pub(crate) fn now() -> Self {
        Self {
            monotonic: Instant::now(),
            wall: SystemTime::now(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct BreakDeadline {
    ends_at: SystemTime,
    duration: Duration,
}

impl BreakDeadline {
    pub(crate) fn starting_at(started_at: SystemTime, duration: Duration) -> Self {
        Self {
            ends_at: started_at.checked_add(duration).unwrap_or(started_at),
            duration,
        }
    }

    pub(crate) fn remaining_at(self, now: SystemTime) -> Duration {
        // The clamp keeps a backwards system-clock jump from inflating the
        // countdown past the break's full duration.
        self.ends_at
            .duration_since(now)
            .unwrap_or(Duration::ZERO)
            .min(self.duration)
    }

    pub(crate) fn is_finished_at(self, now: SystemTime) -> bool {
        self.remaining_at(now).is_zero()
    }
}

#[cfg(test)]
mod tests;
