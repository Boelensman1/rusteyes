use crate::config::{BreakTypeConfig, Breaks, ConfigError};
use std::time::Duration;

pub(crate) const DEFAULT_BREAK_MESSAGE: &str = "Take a break";

#[allow(clippy::module_name_repetitions)]
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct BreakSchedule {
    after_active: Duration,
    reset_after_idle: Option<Duration>,
    rules: Vec<BreakRule>,
}

impl BreakSchedule {
    #[must_use]
    const fn after_active(&self) -> Duration {
        self.after_active
    }

    pub(crate) const fn reset_after_idle(&self) -> Option<Duration> {
        self.reset_after_idle
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
            .find(|rule| slot.is_multiple_of(rule.interval))
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
            reset_after_idle,
            types,
        } = breaks;
        let mut rules = types
            .into_iter()
            .map(|(name, break_type)| BreakRule::from_config(name, break_type))
            .collect::<Vec<_>>();

        rules.sort_by_key(|rule| std::cmp::Reverse(rule.interval));

        Ok(Self {
            after_active,
            reset_after_idle,
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
            message: self.random_message(),
            autolock: self.autolock,
        }
    }

    /// Returns the configured message at `index`, wrapping around the list, or
    /// [`DEFAULT_BREAK_MESSAGE`] when no messages are configured.
    fn message_at(&self, index: usize) -> &str {
        match self.messages.len() {
            0 => DEFAULT_BREAK_MESSAGE,
            len => self.messages[index % len].as_str(),
        }
    }

    /// Picks a message to display for a break at random.
    ///
    /// Falls back to the first message (or [`DEFAULT_BREAK_MESSAGE`] when the
    /// list is empty) if the system random source is unavailable.
    fn random_message(&self) -> String {
        let mut bytes = [0u8; 8];
        let index = match getrandom::fill(&mut bytes) {
            Ok(()) => usize::try_from(u64::from_le_bytes(bytes)).unwrap_or(0),
            Err(_) => 0,
        };
        self.message_at(index).to_owned()
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

    pub(crate) fn reset_active_time(&mut self) {
        self.active_elapsed = Duration::ZERO;
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
    pub(crate) message: String,
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
