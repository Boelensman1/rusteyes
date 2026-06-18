use crate::config::{BreakTypeConfig, Breaks};
use std::time::Duration;

#[allow(clippy::module_name_repetitions)]
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct BreakScheduler {
    breaks: Breaks,
    active_elapsed: Duration,
    slot: usize,
    pending_break: Option<ScheduledBreak>,
}

impl BreakScheduler {
    #[must_use]
    pub(crate) const fn new(breaks: Breaks) -> Self {
        Self {
            breaks,
            active_elapsed: Duration::ZERO,
            slot: 0,
            pending_break: None,
        }
    }

    pub(crate) fn advance_active(&mut self, elapsed: Duration) -> SchedulerAction {
        if self.pending_break.is_some() {
            return SchedulerAction::None;
        }

        self.active_elapsed = self.active_elapsed.saturating_add(elapsed);

        while self.active_elapsed >= self.breaks.after_active {
            self.active_elapsed -= self.breaks.after_active;
            self.slot += 1;

            if let Some(scheduled_break) = self.due_break() {
                self.active_elapsed = Duration::ZERO;
                self.pending_break = Some(scheduled_break.clone());
                return SchedulerAction::StartBreak(scheduled_break);
            }
        }

        SchedulerAction::None
    }

    pub(crate) fn finish_break(&mut self) {
        self.pending_break = None;
        self.active_elapsed = Duration::ZERO;
    }

    #[must_use]
    pub(crate) const fn pending_break(&self) -> Option<&ScheduledBreak> {
        self.pending_break.as_ref()
    }

    fn due_break(&self) -> Option<ScheduledBreak> {
        self.breaks
            .types
            .iter()
            .filter(|(_, break_type)| self.slot % break_type.interval == 0)
            .max_by_key(|(_, break_type)| break_type.interval)
            .map(|(name, break_type)| ScheduledBreak::new(name, self.slot, break_type))
    }
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

impl ScheduledBreak {
    fn new(name: &str, slot: usize, break_type: &BreakTypeConfig) -> Self {
        Self {
            name: name.to_owned(),
            slot,
            duration: break_type.duration,
            messages: break_type.messages.clone(),
            autolock: break_type.autolock,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{BreakScheduler, ScheduledBreak, SchedulerAction};
    use crate::config::{BreakTypeConfig, Breaks, Config, DEFAULT_BREAK_AFTER_ACTIVE};
    use std::collections::BTreeMap;
    use std::time::Duration;

    #[test]
    fn default_config_schedules_short_and_long_break_slots() {
        let mut scheduler = BreakScheduler::new(Config::default().breaks);

        let first = started_break(scheduler.advance_active(DEFAULT_BREAK_AFTER_ACTIVE));
        assert_eq!(first.name, "short");
        assert_eq!(first.slot, 1);
        assert_eq!(first.duration, Duration::from_secs(20));
        assert_eq!(first.messages, vec![String::from("Rest your eyes")]);
        assert!(!first.autolock);

        scheduler.finish_break();

        let second = started_break(scheduler.advance_active(DEFAULT_BREAK_AFTER_ACTIVE));
        assert_eq!(second.name, "long");
        assert_eq!(second.slot, 2);
        assert_eq!(second.duration, Duration::from_secs(5 * 60));
        assert_eq!(second.messages, vec![String::from("Take a longer break")]);
        assert!(second.autolock);

        scheduler.finish_break();

        let third = started_break(scheduler.advance_active(DEFAULT_BREAK_AFTER_ACTIVE));
        assert_eq!(third.name, "short");
        assert_eq!(third.slot, 3);
    }

    #[test]
    fn largest_due_interval_wins_for_the_slot() {
        let mut scheduler = BreakScheduler::new(custom_breaks(
            10,
            &[("blink", 1, 1), ("short", 2, 20), ("long", 4, 300)],
        ));

        assert_eq!(
            started_break(scheduler.advance_active(Duration::from_secs(10))).name,
            "blink"
        );
        scheduler.finish_break();

        assert_eq!(
            started_break(scheduler.advance_active(Duration::from_secs(10))).name,
            "short"
        );
        scheduler.finish_break();

        assert_eq!(
            started_break(scheduler.advance_active(Duration::from_secs(10))).name,
            "blink"
        );
        scheduler.finish_break();

        let fourth = started_break(scheduler.advance_active(Duration::from_secs(10)));
        assert_eq!(fourth.name, "long");
        assert_eq!(fourth.slot, 4);
        assert_eq!(fourth.duration, Duration::from_secs(300));
    }

    #[test]
    fn empty_slots_are_skipped_until_a_break_type_is_due() {
        let mut scheduler =
            BreakScheduler::new(custom_breaks(10, &[("short", 3, 20), ("long", 5, 300)]));

        assert_eq!(
            scheduler.advance_active(Duration::from_secs(20)),
            SchedulerAction::None
        );
        assert_eq!(scheduler.pending_break(), None);

        let third = started_break(scheduler.advance_active(Duration::from_secs(10)));
        assert_eq!(third.name, "short");
        assert_eq!(third.slot, 3);
    }

    #[test]
    fn large_active_delta_starts_only_the_first_due_break() {
        let mut scheduler = BreakScheduler::new(Config::default().breaks);

        let first = started_break(scheduler.advance_active(DEFAULT_BREAK_AFTER_ACTIVE * 3));
        assert_eq!(first.name, "short");
        assert_eq!(first.slot, 1);

        assert_eq!(
            scheduler.advance_active(DEFAULT_BREAK_AFTER_ACTIVE),
            SchedulerAction::None
        );

        scheduler.finish_break();

        let second = started_break(scheduler.advance_active(DEFAULT_BREAK_AFTER_ACTIVE));
        assert_eq!(second.name, "long");
        assert_eq!(second.slot, 2);
    }

    #[test]
    fn pending_break_blocks_active_time_accumulation() {
        let mut scheduler = BreakScheduler::new(Config::default().breaks);

        let first = started_break(scheduler.advance_active(DEFAULT_BREAK_AFTER_ACTIVE));
        assert_eq!(first.name, "short");
        assert_eq!(scheduler.pending_break(), Some(&first));

        assert_eq!(
            scheduler.advance_active(DEFAULT_BREAK_AFTER_ACTIVE * 10),
            SchedulerAction::None
        );
        assert_eq!(scheduler.pending_break(), Some(&first));

        scheduler.finish_break();

        let second = started_break(scheduler.advance_active(DEFAULT_BREAK_AFTER_ACTIVE));
        assert_eq!(second.name, "long");
        assert_eq!(second.slot, 2);
    }

    #[test]
    fn finish_break_restarts_active_accumulation_from_zero() {
        let mut scheduler = BreakScheduler::new(custom_breaks(10, &[("short", 1, 20)]));

        let first = started_break(scheduler.advance_active(Duration::from_secs(10)));
        assert_eq!(first.slot, 1);

        scheduler.finish_break();

        assert_eq!(
            scheduler.advance_active(Duration::from_secs(9)),
            SchedulerAction::None
        );

        let second = started_break(scheduler.advance_active(Duration::from_secs(1)));
        assert_eq!(second.slot, 2);
    }

    fn started_break(action: SchedulerAction) -> ScheduledBreak {
        match action {
            SchedulerAction::StartBreak(scheduled_break) => scheduled_break,
            SchedulerAction::None => panic!("expected break to start"),
        }
    }

    fn custom_breaks(after_active_secs: u64, types: &[(&str, usize, u64)]) -> Breaks {
        Breaks {
            after_active: Duration::from_secs(after_active_secs),
            types: types
                .iter()
                .map(|(name, interval, duration_secs)| {
                    (
                        (*name).to_owned(),
                        BreakTypeConfig {
                            interval: *interval,
                            duration: Duration::from_secs(*duration_secs),
                            messages: vec![format!("Take a {name} break")],
                            autolock: false,
                        },
                    )
                })
                .collect::<BTreeMap<_, _>>(),
        }
    }
}
