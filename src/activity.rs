use crate::backend::RuntimeEvent;
use std::collections::VecDeque;
use std::time::{Duration, Instant};
use tracing::trace;

const NORMAL_ACTIVITY_IDLE_THRESHOLD: Duration = Duration::from_secs(10);

#[derive(Debug)]
pub(crate) struct ActivityPoller {
    poll_interval: Duration,
    events: VecDeque<RuntimeEvent>,
}

impl ActivityPoller {
    pub(crate) fn new(poll_interval: Duration) -> Self {
        Self {
            poll_interval,
            events: VecDeque::new(),
        }
    }

    pub(crate) const fn poll_interval(&self) -> Duration {
        self.poll_interval
    }

    pub(crate) fn queue_sample(&mut self, sample: ActivitySample) -> ActivityState {
        let state = sample.state_for(NORMAL_ACTIVITY_IDLE_THRESHOLD);
        trace!(
            target: "rusteyes::activity",
            idle_for = ?sample.idle_for(),
            ?state,
            poll_interval = ?self.poll_interval,
            idle_threshold = ?NORMAL_ACTIVITY_IDLE_THRESHOLD,
            "sampled activity"
        );

        self.queue_event(RuntimeEvent::WallClockElapsed(self.poll_interval));

        if state == ActivityState::Active {
            self.queue_event(RuntimeEvent::ActiveTimeElapsed(self.poll_interval));
        } else {
            self.queue_event(RuntimeEvent::IdleTimeElapsed(self.poll_interval));
        }

        state
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct BreakDeadline {
    ends_at: Instant,
}

impl BreakDeadline {
    pub(crate) fn starting_at(started_at: Instant, duration: Duration) -> Self {
        Self {
            ends_at: started_at.checked_add(duration).unwrap_or(started_at),
        }
    }

    pub(crate) fn remaining_at(self, now: Instant) -> Duration {
        self.ends_at.saturating_duration_since(now)
    }

    pub(crate) fn is_finished_at(self, now: Instant) -> bool {
        self.remaining_at(now).is_zero()
    }
}

#[cfg(test)]
mod tests;
