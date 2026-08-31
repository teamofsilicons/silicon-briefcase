//! Pure retry and topic-classification policy.

use std::time::Duration;

use uuid::Uuid;

pub(super) const DOMAIN_EVENTS_TOPIC: &str = "briefcase.domain-events.v1";

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum DeliveryFailure {
    ExternalDispatcherUnconfigured,
    UnsupportedTopic,
}

impl DeliveryFailure {
    #[must_use]
    pub(super) const fn code(self) -> &'static str {
        match self {
            Self::ExternalDispatcherUnconfigured => "external_dispatch_unconfigured",
            Self::UnsupportedTopic => "unsupported_outbox_topic",
        }
    }
}

#[must_use]
pub(super) fn classify_topic(topic: &str) -> DeliveryFailure {
    if topic == DOMAIN_EVENTS_TOPIC {
        DeliveryFailure::ExternalDispatcherUnconfigured
    } else {
        DeliveryFailure::UnsupportedTopic
    }
}

#[must_use]
pub(super) const fn should_dead_letter(attempt: u16, maximum_attempts: u16) -> bool {
    attempt >= maximum_attempts
}

/// Calculates half-to-full deterministic jitter over capped exponential delay.
#[must_use]
pub(super) fn retry_delay(
    base: Duration,
    maximum: Duration,
    attempt: u16,
    event_id: Uuid,
) -> Duration {
    let capped = capped_exponential(base, maximum, attempt);
    let floor = capped / 2;
    let window = capped.saturating_sub(floor);
    floor
        .saturating_add(jitter(window, event_id, attempt))
        .min(maximum)
}

fn capped_exponential(base: Duration, maximum: Duration, attempt: u16) -> Duration {
    let mut delay = base.min(maximum);
    for _ in 1..attempt {
        if delay == maximum {
            break;
        }
        delay = delay.saturating_mul(2).min(maximum);
    }
    delay
}

fn jitter(window: Duration, event_id: Uuid, attempt: u16) -> Duration {
    let window_millis = window.as_millis();
    if window_millis == 0 {
        return Duration::ZERO;
    }

    let attempt_seed = u128::from(attempt).wrapping_mul(0x9e37_79b9_7f4a_7c15);
    let seed = event_id.as_u128() ^ attempt_seed;
    let offset = seed % window_millis.saturating_add(1);
    match u64::try_from(offset) {
        Ok(milliseconds) => Duration::from_millis(milliseconds),
        Err(_) => window,
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use uuid::Uuid;

    use super::{
        DOMAIN_EVENTS_TOPIC, DeliveryFailure, classify_topic, retry_delay, should_dead_letter,
    };

    #[test]
    fn known_external_and_unknown_topics_fail_honestly() {
        assert_eq!(
            classify_topic(DOMAIN_EVENTS_TOPIC),
            DeliveryFailure::ExternalDispatcherUnconfigured
        );
        assert_eq!(
            classify_topic("vendor.notification.v1"),
            DeliveryFailure::UnsupportedTopic
        );
        assert_eq!(
            classify_topic(DOMAIN_EVENTS_TOPIC).code(),
            "external_dispatch_unconfigured"
        );
        assert_eq!(
            classify_topic("vendor.notification.v1").code(),
            "unsupported_outbox_topic"
        );
    }

    #[test]
    fn retry_delay_is_deterministic_exponential_and_capped() {
        let event_id = Uuid::from_u128(42);
        let base = Duration::from_secs(2);
        let maximum = Duration::from_secs(10);
        let expected_caps = [2, 4, 8, 10, 10, 10];

        for (attempt, expected_cap_seconds) in (1_u16..).zip(expected_caps) {
            let delay = retry_delay(base, maximum, attempt, event_id);
            let cap = Duration::from_secs(expected_cap_seconds);
            assert!(delay >= cap / 2);
            assert!(delay <= cap);
            assert_eq!(delay, retry_delay(base, maximum, attempt, event_id));
        }
    }

    #[test]
    fn terminal_attempt_is_inclusive() {
        assert!(!should_dead_letter(19, 20));
        assert!(should_dead_letter(20, 20));
        assert!(should_dead_letter(21, 20));
    }
}
