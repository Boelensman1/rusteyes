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

    fn next_due_break_after(&self, slot: usize) -> Option<ScheduledBreak> {
        let max_interval = self.rules.iter().map(|rule| rule.interval).max()?;

        for offset in 1..=max_interval {
            let next_slot = slot.checked_add(offset)?;
            if let Some(scheduled_break) = self.due_break(next_slot) {
                return Some(scheduled_break);
            }
        }

        None
    }

    fn due_break(&self, slot: usize) -> Option<ScheduledBreak> {
        self.rules
            .iter()
            .find(|rule| slot % rule.interval == 0)
            .map(|rule| rule.to_break(BreakOrigin::Scheduled { slot }))
    }

    fn manual_break(&self, name: &str) -> Option<ScheduledBreak> {
        self.rules
            .iter()
            .find(|rule| rule.name == name)
            .map(|rule| rule.to_break(BreakOrigin::Manual))
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

        rules.sort_by_key(|rule| std::cmp::Reverse(rule.interval));

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

    fn to_break(&self, origin: BreakOrigin) -> ScheduledBreak {
        ScheduledBreak {
            name: self.name.clone(),
            origin,
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
            state: SchedulerState::Ready(SchedulerMode::Active),
        }
    }

    pub(crate) const fn after_active(&self) -> Duration {
        self.schedule.after_active()
    }

    pub(crate) const fn active_elapsed(&self) -> Duration {
        self.active_elapsed
    }

    pub(crate) fn upcoming_scheduled_break(&self) -> Option<UpcomingScheduledBreak> {
        if self.state != SchedulerState::Ready(SchedulerMode::Active) {
            return None;
        }

        let scheduled_break = self.schedule.next_due_break_after(self.slot)?;
        let BreakOrigin::Scheduled { slot: next_slot } = scheduled_break.origin else {
            return None;
        };

        let mut starts_after = self
            .schedule
            .after_active()
            .saturating_sub(self.active_elapsed);
        for _ in self.slot + 1..next_slot {
            starts_after = starts_after.saturating_add(self.schedule.after_active());
        }

        Some(UpcomingScheduledBreak {
            scheduled_break,
            starts_after,
        })
    }

    pub(crate) fn advance_active(&mut self, elapsed: Duration) -> Option<ScheduledBreak> {
        if self.state != SchedulerState::Ready(SchedulerMode::Active) {
            return None;
        }

        self.active_elapsed = self.active_elapsed.saturating_add(elapsed);

        while self.active_elapsed >= self.schedule.after_active() {
            self.active_elapsed -= self.schedule.after_active();
            self.slot += 1;

            if let Some(scheduled_break) = self.schedule.due_break(self.slot) {
                self.active_elapsed = Duration::ZERO;
                self.state = SchedulerState::Pending {
                    resume: SchedulerMode::Active,
                };
                return Some(scheduled_break);
            }
        }

        None
    }

    pub(crate) fn start_manual_break(&mut self, name: &str) -> Option<ScheduledBreak> {
        let resume = match self.state {
            SchedulerState::Ready(mode) => mode,
            SchedulerState::Pending { .. } => return None,
        };
        let scheduled_break = self.schedule.manual_break(name)?;

        self.active_elapsed = Duration::ZERO;
        self.state = SchedulerState::Pending { resume };
        Some(scheduled_break)
    }

    pub(crate) fn finish_break(&mut self) -> bool {
        let previous = std::mem::replace(
            &mut self.state,
            SchedulerState::Ready(SchedulerMode::Active),
        );
        self.active_elapsed = Duration::ZERO;

        match previous {
            SchedulerState::Pending { resume } => {
                self.state = SchedulerState::Ready(resume);
                true
            }
            SchedulerState::Ready(SchedulerMode::Disabled) => {
                self.state = SchedulerState::Ready(SchedulerMode::Disabled);
                false
            }
            SchedulerState::Ready(SchedulerMode::Active) => false,
        }
    }

    pub(crate) fn disable(&mut self) -> bool {
        let previous = std::mem::replace(
            &mut self.state,
            SchedulerState::Ready(SchedulerMode::Disabled),
        );
        self.active_elapsed = Duration::ZERO;

        match previous {
            SchedulerState::Pending { .. } => true,
            SchedulerState::Ready(SchedulerMode::Active | SchedulerMode::Disabled) => false,
        }
    }

    pub(crate) fn enable(&mut self) {
        match &mut self.state {
            SchedulerState::Ready(mode) => *mode = SchedulerMode::Active,
            SchedulerState::Pending { resume, .. } => *resume = SchedulerMode::Active,
        }
    }

    #[cfg(test)]
    #[must_use]
    pub(crate) fn is_disabled(&self) -> bool {
        matches!(
            self.state,
            SchedulerState::Ready(SchedulerMode::Disabled)
                | SchedulerState::Pending {
                    resume: SchedulerMode::Disabled,
                }
        )
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum SchedulerState {
    Ready(SchedulerMode),
    Pending { resume: SchedulerMode },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SchedulerMode {
    Active,
    Disabled,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ScheduledBreak {
    pub(crate) name: String,
    pub(crate) origin: BreakOrigin,
    pub(crate) duration: Duration,
    pub(crate) messages: Vec<String>,
    pub(crate) autolock: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum BreakOrigin {
    Scheduled { slot: usize },
    Manual,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct UpcomingScheduledBreak {
    pub(crate) scheduled_break: ScheduledBreak,
    pub(crate) starts_after: Duration,
}

#[cfg(test)]
mod tests;
