use crate::backend::RuntimeEvent;
use std::collections::VecDeque;
use std::time::Duration;
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
pub(crate) struct BreakTimer {
    remaining: Duration,
}

impl BreakTimer {
    pub(crate) const fn new(duration: Duration) -> Self {
        Self {
            remaining: duration,
        }
    }

    pub(crate) fn advance(&mut self, elapsed: Duration) -> bool {
        if self.remaining.is_zero() {
            return false;
        }

        if elapsed >= self.remaining {
            self.remaining = Duration::ZERO;
            true
        } else {
            self.remaining -= elapsed;
            false
        }
    }

    pub(crate) const fn remaining(self) -> Duration {
        self.remaining
    }
}

pub(crate) fn break_elapsed_for_sample(
    sample: ActivitySample,
    poll_interval: Duration,
) -> Duration {
    match sample.state_for(poll_interval) {
        ActivityState::Active => Duration::ZERO,
        ActivityState::Idle => poll_interval,
    }
}

#[cfg(test)]
mod tests;
