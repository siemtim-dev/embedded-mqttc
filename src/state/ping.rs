use embassy_time::Duration;

use embassy_time::Instant;

use super::KEEP_ALIVE;

const PING_RETRY_DURATION: Duration = Duration::from_secs(10);
const KEEP_ALIVE_DURATION: Duration = Duration::from_secs(KEEP_ALIVE as u64);

#[derive(Debug, Clone, PartialEq)]
#[cfg_attr(feature = "defmt", derive(defmt::Format))]
pub(crate) enum PingState {
    PingSuccess(Instant),

    AwaitingResponse {
        last_success: Instant,
        ping_request_sent: Instant
    }
}

impl PingState {
    pub(crate) fn should_send_ping(&self, now: &Instant) -> bool {
        match self {
            PingState::PingSuccess(instant) => {
                let diff = ((*now - *instant).as_secs()) as usize;
                diff > KEEP_ALIVE / 2
            },
            PingState::AwaitingResponse { last_success: _, ping_request_sent } => {
                let diff = *now - *ping_request_sent;
                diff > PING_RETRY_DURATION
            },
        }
    }

    pub(crate) fn is_critical_delay(&self, now: &Instant) -> bool {
        let diff = *now - *self.last_success();
        diff >= Duration::from_secs(KEEP_ALIVE as u64 - 5)
    }

    fn last_success(&self) -> &Instant {
        match self {
            PingState::PingSuccess(instant) => instant,
            PingState::AwaitingResponse { last_success, ping_request_sent: _ } => last_success,
        }
    }

    pub(crate) fn ping_sent(&mut self, now: &Instant) {
        *self = PingState::AwaitingResponse { 
            last_success: self.last_success().clone(), 
            ping_request_sent: now.clone()
        };
    }

    pub(crate) fn on_ping_response(&mut self, now: &Instant) {
        *self = PingState::PingSuccess(now.clone())
    }

    /// Returns the duration until the next 
    pub(crate) fn ping_pause(&self, now: &Instant) -> Option<Duration> {
        match self {
            PingState::PingSuccess(instant) => {
                let diff = *now - *instant;
                let half_keep_alive = KEEP_ALIVE_DURATION / 2;
                if diff > half_keep_alive {
                    None
                } else {
                    Some(half_keep_alive - diff)
                }
            },
            PingState::AwaitingResponse { last_success: _, ping_request_sent } => {
                let diff = *now - *ping_request_sent;
                if diff > PING_RETRY_DURATION {
                    None
                } else {
                    Some(PING_RETRY_DURATION - diff)
                }
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use embassy_time::{Duration, Instant};

    use crate::state::KEEP_ALIVE;

    use super::PingState;


    #[test]
    fn test_should_send_ping_after_success() {
        let start = Instant::from_secs(1000);
        let ping_state = PingState::PingSuccess(start.clone());

        let a_bit_later = start + Duration::from_secs((KEEP_ALIVE / 2 - 3) as u64);
        assert_eq!(ping_state.should_send_ping(&a_bit_later), false);
        assert_eq!(ping_state.is_critical_delay(&a_bit_later), false);

        let later = start + Duration::from_secs((KEEP_ALIVE / 2 + 5) as u64);
        assert_eq!(ping_state.should_send_ping(&later), true);
        assert_eq!(ping_state.is_critical_delay(&later), false);

        let too_late = start + Duration::from_secs(KEEP_ALIVE as u64);
        assert_eq!(ping_state.is_critical_delay(&too_late), true);
    }

    #[test]
    fn test_sould_send_ping_waiting() {
        let start = Instant::from_secs(1000);
        let ping_state = PingState::AwaitingResponse { 
            last_success: start - Duration::from_secs((KEEP_ALIVE / 2 + 4) as u64), 
            ping_request_sent: start
        };

        let a_bit_later = start + Duration::from_secs(5);
        assert_eq!(ping_state.should_send_ping(&a_bit_later), false);
        assert_eq!(ping_state.is_critical_delay(&a_bit_later), false);

        let later = start + Duration::from_secs(11);
        assert_eq!(ping_state.should_send_ping(&later), true);
        assert_eq!(ping_state.is_critical_delay(&later), false);

        let too_late = start + Duration::from_secs(KEEP_ALIVE as u64);
        assert_eq!(ping_state.is_critical_delay(&too_late), true);
    }

    #[test]
    fn test_on_ping_sent () {
        let start = Instant::from_secs(1000);
        let mut ping_state = PingState::PingSuccess(start.clone());

        let ping_sent = start + Duration::from_secs(20);
        ping_state.ping_sent(&ping_sent);

        assert_eq!(ping_state, PingState::AwaitingResponse { 
            last_success: start, 
            ping_request_sent: ping_sent 
        });
    }

}