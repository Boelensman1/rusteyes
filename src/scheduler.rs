use crate::config::{BreakTypeConfig, Breaks, ConfigError};
use std::collections::BTreeMap;
use std::time::Duration;

pub(crate) const DEFAULT_BREAK_MESSAGE: &str = "Take a break";

#[allow(clippy::module_name_repetitions)]
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct BreakSchedule {
    after_active: Duration,
    reset_after_idle: Option<Duration>,
    reset_count_after_idle: Option<Duration>,
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

    pub(crate) const fn reset_count_after_idle(&self) -> Option<Duration> {
        self.reset_count_after_idle
    }

    fn rule(&self, name: &str) -> Option<&BreakRule> {
        self.rules.iter().find(|rule| rule.name == name)
    }

    fn initial_last_satisfied_slots(&self) -> BTreeMap<String, usize> {
        self.rules
            .iter()
            .map(|rule| (rule.name.clone(), 0))
            .collect()
    }
}

impl TryFrom<Breaks> for BreakSchedule {
    type Error = ConfigError;

    fn try_from(breaks: Breaks) -> Result<Self, Self::Error> {
        breaks.validate()?;

        let Breaks {
            after_active,
            reset_after_idle,
            reset_count_after_idle,
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
            reset_count_after_idle,
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
    last_satisfied_slots: BTreeMap<String, usize>,
    state: SchedulerState,
}

impl BreakScheduler {
    #[must_use]
    pub(crate) fn new(schedule: BreakSchedule) -> Self {
        let last_satisfied_slots = schedule.initial_last_satisfied_slots();

        Self {
            schedule,
            active_elapsed: Duration::ZERO,
            slot: 0,
            last_satisfied_slots,
            state: SchedulerState::Ready(SchedulerMode::Active),
        }
    }

    pub(crate) const fn after_active(&self) -> Duration {
        self.schedule.after_active()
    }

    pub(crate) const fn active_elapsed(&self) -> Duration {
        self.active_elapsed
    }

    pub(crate) fn position(&self) -> SchedulerPosition {
        SchedulerPosition {
            slot: self.slot,
            active_elapsed: self.active_elapsed,
            last_satisfied_slots: self.last_satisfied_slots.clone(),
        }
    }

    pub(crate) fn upcoming_scheduled_break(&self) -> Option<UpcomingScheduledBreak> {
        if self.state != SchedulerState::Ready(SchedulerMode::Active) {
            return None;
        }

        let scheduled_break = self.next_due_break_after(self.slot)?;
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

    pub(crate) fn manual_break_availability(&self) -> BTreeMap<String, bool> {
        let next_interval = self
            .upcoming_scheduled_break()
            .and_then(|upcoming| self.schedule.rule(&upcoming.scheduled_break.name))
            .map(|rule| rule.interval);

        self.schedule
            .rules
            .iter()
            .map(|rule| {
                let available = next_interval.is_none_or(|interval| rule.interval >= interval);
                (rule.name.clone(), available)
            })
            .collect()
    }

    fn manual_break_is_available(&self, name: &str) -> bool {
        let Some(rule) = self.schedule.rule(name) else {
            return false;
        };
        let Some(upcoming) = self.upcoming_scheduled_break() else {
            return true;
        };
        let Some(upcoming_rule) = self.schedule.rule(&upcoming.scheduled_break.name) else {
            return true;
        };

        rule.interval >= upcoming_rule.interval
    }

    pub(crate) fn advance_active(&mut self, elapsed: Duration) -> Option<ScheduledBreak> {
        if self.state != SchedulerState::Ready(SchedulerMode::Active) {
            return None;
        }

        self.active_elapsed = self.active_elapsed.saturating_add(elapsed);

        while self.active_elapsed >= self.schedule.after_active() {
            self.active_elapsed -= self.schedule.after_active();
            self.slot += 1;

            if let Some(scheduled_break) = self.due_break_at(self.slot) {
                self.satisfy_scheduled_breaks(self.slot, &scheduled_break.name);
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

    pub(crate) fn reset_position(&mut self) -> bool {
        let changed = self.slot != 0
            || !self.active_elapsed.is_zero()
            || self.last_satisfied_slots.values().any(|slot| *slot != 0);

        self.slot = 0;
        self.active_elapsed = Duration::ZERO;
        for slot in self.last_satisfied_slots.values_mut() {
            *slot = 0;
        }

        changed
    }

    pub(crate) fn start_manual_break(&mut self, name: &str) -> Option<ScheduledBreak> {
        let resume = match self.state {
            SchedulerState::Ready(mode) => mode,
            SchedulerState::Pending { .. } => return None,
        };
        if !self.manual_break_is_available(name) {
            return None;
        }
        let rule = self.schedule.rule(name)?;
        let scheduled_break = rule.to_break(BreakOrigin::Manual);
        let interval = rule.interval;

        self.active_elapsed = Duration::ZERO;
        self.satisfy_manual_breaks(interval);
        self.state = SchedulerState::Pending { resume };
        Some(scheduled_break)
    }

    pub(crate) fn has_break(&self, name: &str) -> bool {
        self.schedule.rule(name).is_some()
    }

    pub(crate) fn start_synced_break(
        &mut self,
        name: &str,
        origin: BreakOrigin,
    ) -> Option<ScheduledBreak> {
        let scheduled_break = self.break_for_origin(name, origin)?;
        self.begin_synced_break(name, origin)?;
        Some(scheduled_break)
    }

    pub(crate) fn replacement_synced_break(
        &mut self,
        name: &str,
        origin: BreakOrigin,
    ) -> Option<ScheduledBreak> {
        let scheduled_break = self.break_for_origin(name, origin)?;
        self.adopt_synced_origin(name, origin)?;
        Some(scheduled_break)
    }

    pub(crate) fn merge_synced_position(&mut self, position: SchedulerPosition) -> bool {
        let SchedulerPosition {
            slot,
            active_elapsed,
            last_satisfied_slots,
        } = position;
        let mut changed = false;

        if slot > self.slot {
            self.slot = slot;
            self.active_elapsed = active_elapsed;
            changed = true;
        }

        if slot == self.slot && active_elapsed > self.active_elapsed {
            self.active_elapsed = active_elapsed;
            changed = true;
        }

        for rule in &self.schedule.rules {
            let Some(position_slot) = last_satisfied_slots.get(&rule.name) else {
                continue;
            };
            let Some(current_slot) = self.last_satisfied_slots.get_mut(&rule.name) else {
                continue;
            };
            let position_slot = (*position_slot).min(self.slot);
            if position_slot > *current_slot {
                *current_slot = position_slot;
                changed = true;
            }
        }

        changed
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

    fn begin_synced_break(&mut self, name: &str, origin: BreakOrigin) -> Option<()> {
        let resume = match self.state {
            SchedulerState::Ready(mode) => mode,
            SchedulerState::Pending { .. } => return None,
        };

        if let BreakOrigin::Scheduled { slot } = origin
            && slot <= self.slot
        {
            return None;
        }

        self.adopt_synced_origin(name, origin)?;
        self.active_elapsed = Duration::ZERO;
        self.state = SchedulerState::Pending { resume };
        Some(())
    }

    fn adopt_synced_origin(&mut self, name: &str, origin: BreakOrigin) -> Option<()> {
        match origin {
            BreakOrigin::Manual => {
                let interval = self.schedule.rule(name)?.interval;
                self.satisfy_manual_breaks(interval);
            }
            BreakOrigin::Scheduled { slot } => {
                if slot < self.slot {
                    return None;
                }
                self.slot = slot;
                self.active_elapsed = Duration::ZERO;
                self.satisfy_scheduled_breaks(slot, name);
            }
        }

        Some(())
    }

    fn break_for_origin(&self, name: &str, origin: BreakOrigin) -> Option<ScheduledBreak> {
        let rule = self.schedule.rule(name)?;

        match origin {
            BreakOrigin::Manual => Some(rule.to_break(BreakOrigin::Manual)),
            BreakOrigin::Scheduled { slot } if slot > 0 => {
                Some(rule.to_break(BreakOrigin::Scheduled { slot }))
            }
            BreakOrigin::Scheduled { .. } => None,
        }
    }

    fn next_due_break_after(&self, slot: usize) -> Option<ScheduledBreak> {
        let next_slot = self
            .schedule
            .rules
            .iter()
            .filter_map(|rule| self.next_due_slot_for_rule(rule, slot))
            .min()?;

        self.due_break_at(next_slot)
    }

    fn next_due_slot_for_rule(&self, rule: &BreakRule, slot: usize) -> Option<usize> {
        let next_slot = slot.checked_add(1)?;
        let due_slot = self.last_satisfied_slot(rule).saturating_add(rule.interval);

        Some(std::cmp::max(next_slot, due_slot))
    }

    fn due_break_at(&self, slot: usize) -> Option<ScheduledBreak> {
        self.schedule
            .rules
            .iter()
            .find(|rule| self.rule_due_at(rule, slot))
            .map(|rule| rule.to_break(BreakOrigin::Scheduled { slot }))
    }

    fn rule_due_at(&self, rule: &BreakRule, slot: usize) -> bool {
        slot.saturating_sub(self.last_satisfied_slot(rule)) >= rule.interval
    }

    fn last_satisfied_slot(&self, rule: &BreakRule) -> usize {
        self.last_satisfied_slots
            .get(&rule.name)
            .copied()
            .unwrap_or(0)
    }

    fn satisfy_scheduled_breaks(&mut self, slot: usize, selected_name: &str) {
        let Some(selected_interval) = self.schedule.rule(selected_name).map(|rule| rule.interval)
        else {
            return;
        };
        let names = self
            .schedule
            .rules
            .iter()
            .filter(|rule| rule.interval <= selected_interval && self.rule_due_at(rule, slot))
            .map(|rule| rule.name.clone())
            .collect::<Vec<_>>();

        for name in names {
            self.last_satisfied_slots.insert(name, slot);
        }
    }

    fn satisfy_manual_breaks(&mut self, selected_interval: usize) {
        let names = self
            .schedule
            .rules
            .iter()
            .filter(|rule| rule.interval <= selected_interval)
            .map(|rule| rule.name.clone())
            .collect::<Vec<_>>();

        for name in names {
            self.last_satisfied_slots.insert(name, self.slot);
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

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SchedulerPosition {
    pub(crate) slot: usize,
    pub(crate) active_elapsed: Duration,
    pub(crate) last_satisfied_slots: BTreeMap<String, usize>,
}

#[cfg(test)]
mod tests;
