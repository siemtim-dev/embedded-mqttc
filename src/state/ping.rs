
use crate::time::{ self, Duration, Instant };

use super::KEEP_ALIVE;

const PING_RETRY_DURATION: Duration = Duration::from_secs(5);
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
    pub(crate) fn should_send_ping(&self) -> bool {
        let now = time::now();
        match self {
            PingState::PingSuccess(instant) => {
                let diff = now - *instant;
                diff > KEEP_ALIVE_DURATION / 2
            },
            PingState::AwaitingResponse { last_success: _, ping_request_sent } => {
                let diff = now - *ping_request_sent;
                diff > PING_RETRY_DURATION
            },
        }
    }

    pub(crate) fn is_critical_delay(&self) -> bool {
        let now = time::now();
        let diff = now - *self.last_success();
        diff >= Duration::from_secs(KEEP_ALIVE as u64 - 5)
    }

    fn last_success(&self) -> &Instant {
        match self {
            PingState::PingSuccess(instant) => instant,
            PingState::AwaitingResponse { last_success, ping_request_sent: _ } => last_success,
        }
    }

    pub(crate) fn ping_sent(&mut self) {
        let now = time::now();
        info!("ping sent at {}", now);
        *self = PingState::AwaitingResponse { 
            last_success: self.last_success().clone(), 
            ping_request_sent: now.clone()
        };
    }

    pub(crate) fn on_ping_response(&mut self) {
        let now = time::now();
        *self = PingState::PingSuccess(now.clone())
    }

    /// Returns the duration until the next 
    pub(crate) fn ping_pause(&self) -> Option<Duration> {
        let now = time::now();
        match self {
            PingState::PingSuccess(instant) => {
                let diff = now - *instant;
                let half_keep_alive = KEEP_ALIVE_DURATION / 2;
                if diff > half_keep_alive {
                    debug!("send ping now!");
                    None
                } else {
                    let d = half_keep_alive - diff;
                    trace!("send ping in {}", d);
                    Some(d)
                }
            },
            PingState::AwaitingResponse { last_success: _, ping_request_sent } => {
                let diff = now - *ping_request_sent;
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
    use crate::time::{self, Duration};

    use crate::state::KEEP_ALIVE;

    use super::PingState;


    #[test]
    fn test_should_send_ping_after_success() {
        time::test_time::set_static_now();

        let start = time::now();
        let ping_state = PingState::PingSuccess(start.clone());

        let a_bit_later = start + Duration::from_secs((KEEP_ALIVE / 2 - 3) as u64);
        time::test_time::set_time(a_bit_later);
        assert_eq!(ping_state.should_send_ping(), false);
        assert_eq!(ping_state.is_critical_delay(), false);

        let later = start + Duration::from_secs((KEEP_ALIVE / 2 + 5) as u64);
        time::test_time::set_time(later);
        assert_eq!(ping_state.should_send_ping(), true);
        assert_eq!(ping_state.is_critical_delay(), false);

        let too_late = start + Duration::from_secs(KEEP_ALIVE as u64);
        time::test_time::set_time(too_late);
        assert_eq!(ping_state.is_critical_delay(), true);
    }

    #[test]
    fn test_sould_send_ping_waiting() {
        time::test_time::set_static_now();

        let start = time::now();
        let ping_state = PingState::AwaitingResponse { 
            last_success: start - Duration::from_secs((KEEP_ALIVE / 2 + 4) as u64), 
            ping_request_sent: start
        };

        let a_bit_later = start + Duration::from_secs(5);
        time::test_time::set_time(a_bit_later);
        assert_eq!(ping_state.should_send_ping(), false);
        assert_eq!(ping_state.is_critical_delay(), false);

        let later = start + Duration::from_secs(11);
        time::test_time::set_time(later);
        assert_eq!(ping_state.should_send_ping(), true);
        assert_eq!(ping_state.is_critical_delay(), false);

        let too_late = start + Duration::from_secs(KEEP_ALIVE as u64);
        time::test_time::set_time(too_late);
        assert_eq!(ping_state.is_critical_delay(), true);
    }

    #[test]
    fn test_on_ping_sent () {
        time::test_time::set_static_now();
        let start = time::now();
        
        let mut ping_state = PingState::PingSuccess(start.clone());

        let ping_sent = start + Duration::from_secs(20);
        time::test_time::set_time(ping_sent);
        ping_state.ping_sent();

        assert_eq!(ping_state, PingState::AwaitingResponse { 
            last_success: start, 
            ping_request_sent: ping_sent 
        });
    }

    #[test]
    fn test_ping_pause() {
        time::test_time::set_static_now();
        let start = time::now();
        let ping_state = PingState::PingSuccess(start.clone());

        let pause = ping_state.ping_pause().expect("there must be a ping pause");

        assert_eq!(ping_state.should_send_ping(), false);

        time::test_time::advance_time(pause - Duration::from_millis(10));

        assert_eq!(ping_state.should_send_ping(), false);

        time::test_time::advance_time(Duration::from_millis(20));

        assert_eq!(ping_state.ping_pause(), None);
        assert_eq!(ping_state.should_send_ping(), true);
        
    }

}