use std::fmt;

const WARNING_THROTTLE_REPEAT_LOG_INTERVAL: u64 = 60;

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
enum WarningThrottleAction {
    Log { suppressed_repetitions: u64 },
    Suppress,
}

#[derive(Debug, Default)]
pub(crate) struct WarningThrottle {
    last_fingerprint: Option<String>,
    suppressed_repetitions: u64,
}

impl WarningThrottle {
    pub(crate) fn record_success(&mut self) {
        self.last_fingerprint = None;
        self.suppressed_repetitions = 0;
    }

    pub(crate) fn record_error(&mut self, error: &(impl fmt::Display + ?Sized)) -> Option<u64> {
        match self.record(&error.to_string()) {
            WarningThrottleAction::Log {
                suppressed_repetitions,
            } => Some(suppressed_repetitions),
            WarningThrottleAction::Suppress => None,
        }
    }

    fn record(&mut self, fingerprint: &str) -> WarningThrottleAction {
        if self.last_fingerprint.as_deref() == Some(fingerprint) {
            self.suppressed_repetitions = self.suppressed_repetitions.saturating_add(1);
            if self
                .suppressed_repetitions
                .is_multiple_of(WARNING_THROTTLE_REPEAT_LOG_INTERVAL)
            {
                return WarningThrottleAction::Log {
                    suppressed_repetitions: self.suppressed_repetitions,
                };
            }
            return WarningThrottleAction::Suppress;
        }

        let suppressed_repetitions = self.suppressed_repetitions;
        self.last_fingerprint = Some(fingerprint.to_string());
        self.suppressed_repetitions = 0;
        WarningThrottleAction::Log {
            suppressed_repetitions,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{WarningThrottle, WarningThrottleAction};

    #[test]
    fn logs_first_error_and_suppresses_identical_repeats() {
        let mut throttle = WarningThrottle::default();

        assert_eq!(
            throttle.record("settings sync failed"),
            WarningThrottleAction::Log {
                suppressed_repetitions: 0
            }
        );
        assert_eq!(
            throttle.record("settings sync failed"),
            WarningThrottleAction::Suppress
        );
    }

    #[test]
    fn logs_when_error_changes_with_suppressed_count() {
        let mut throttle = WarningThrottle::default();

        assert_eq!(
            throttle.record("settings sync failed"),
            WarningThrottleAction::Log {
                suppressed_repetitions: 0
            }
        );
        assert_eq!(
            throttle.record("settings sync failed"),
            WarningThrottleAction::Suppress
        );
        assert_eq!(
            throttle.record("audit write failed"),
            WarningThrottleAction::Log {
                suppressed_repetitions: 1
            }
        );
    }

    #[test]
    fn logs_same_error_after_success() {
        let mut throttle = WarningThrottle::default();

        assert_eq!(
            throttle.record("settings sync failed"),
            WarningThrottleAction::Log {
                suppressed_repetitions: 0
            }
        );
        assert_eq!(
            throttle.record("settings sync failed"),
            WarningThrottleAction::Suppress
        );

        throttle.record_success();

        assert_eq!(
            throttle.record("settings sync failed"),
            WarningThrottleAction::Log {
                suppressed_repetitions: 0
            }
        );
    }
}
