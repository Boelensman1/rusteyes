use crate::config::{BreakTypeConfig, Breaks, ConfigError};
use std::time::Duration;

#[allow(clippy::module_name_repetitions)]
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct BreakSchedule {
    after_active: Duration,
    rules: Vec<BreakRule>,
}

impl BreakSchedule {
    #[must_use]
    const fn after_active(&self) -> Duration {
        self.after_active
    }

    fn due_break(&self, slot: usize) -> Option<ScheduledBreak> {
        self.rules
            .iter()
            .find(|rule| slot % rule.interval == 0)
            .map(|rule| rule.scheduled_break(slot))
    }
}

impl TryFrom<Breaks> for BreakSchedule {
    type Error = ConfigError;

    fn try_from(breaks: Breaks) -> Result<Self, Self::Error> {
        breaks.validate()?;

        let Breaks {
            after_active,
            types,
        } = breaks;
        let mut rules = types
            .into_iter()
            .map(|(name, break_type)| BreakRule::from_config(name, break_type))
            .collect::<Vec<_>>();

        rules.sort_by(|left, right| {
            right
                .interval
                .cmp(&left.interval)
                .then_with(|| left.name.cmp(&right.name))
        });

        Ok(Self {
            after_active,
            rules,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct BreakRule {
    name: String,
    interval: usize,
    duration: Duration,
    messages: Vec<String>,
    autolock: bool,
}

impl BreakRule {
    fn from_config(name: String, break_type: BreakTypeConfig) -> Self {
        Self {
            name,
            interval: break_type.interval,
            duration: break_type.duration,
            messages: break_type.messages,
            autolock: break_type.autolock,
        }
    }

    fn scheduled_break(&self, slot: usize) -> ScheduledBreak {
        ScheduledBreak {
            name: self.name.clone(),
            slot,
            duration: self.duration,
            messages: self.messages.clone(),
            autolock: self.autolock,
        }
    }
}

#[allow(clippy::module_name_repetitions)]
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct BreakScheduler {
    schedule: BreakSchedule,
    active_elapsed: Duration,
    slot: usize,
    state: SchedulerState,
}

impl BreakScheduler {
    #[must_use]
    pub(crate) fn new(schedule: BreakSchedule) -> Self {
        Self {
            schedule,
            active_elapsed: Duration::ZERO,
            slot: 0,
            state: SchedulerState::Active,
        }
    }

    pub(crate) fn advance_active(&mut self, elapsed: Duration) -> SchedulerAction {
        if self.state != SchedulerState::Active {
            return SchedulerAction::None;
        }

        self.active_elapsed = self.active_elapsed.saturating_add(elapsed);

        while self.active_elapsed >= self.schedule.after_active() {
            self.active_elapsed -= self.schedule.after_active();
            self.slot += 1;

            if let Some(scheduled_break) = self.schedule.due_break(self.slot) {
                self.active_elapsed = Duration::ZERO;
                self.state = SchedulerState::Pending(scheduled_break.clone());
                return SchedulerAction::StartBreak(scheduled_break);
            }
        }

        SchedulerAction::None
    }

    pub(crate) fn finish_break(&mut self) {
        if self.state != SchedulerState::Disabled {
            self.state = SchedulerState::Active;
        }
        self.active_elapsed = Duration::ZERO;
    }

    pub(crate) fn disable(&mut self) {
        self.state = SchedulerState::Disabled;
        self.active_elapsed = Duration::ZERO;
    }

    pub(crate) fn enable(&mut self) {
        if self.state == SchedulerState::Disabled {
            self.state = SchedulerState::Active;
        }
    }

    #[must_use]
    #[allow(dead_code)]
    pub(crate) fn is_disabled(&self) -> bool {
        self.state == SchedulerState::Disabled
    }

    #[must_use]
    #[allow(dead_code)]
    pub(crate) fn pending_break(&self) -> Option<&ScheduledBreak> {
        match &self.state {
            SchedulerState::Pending(scheduled_break) => Some(scheduled_break),
            SchedulerState::Active | SchedulerState::Disabled => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum SchedulerState {
    Active,
    Pending(ScheduledBreak),
    Disabled,
}

#[allow(clippy::module_name_repetitions)]
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum SchedulerAction {
    None,
    StartBreak(ScheduledBreak),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ScheduledBreak {
    pub(crate) name: String,
    pub(crate) slot: usize,
    pub(crate) duration: Duration,
    pub(crate) messages: Vec<String>,
    pub(crate) autolock: bool,
}

#[cfg(test)]
mod tests;
