use embassy_time::Duration;

use embassy_time::Instant;

use super::KEEP_ALIVE;

const PING_RETRY_DURATION: Duration = Duration::from_secs(10);
const KEEP_ALIVE_DURATION: Duration = Duration::from_secs(KEEP_ALIVE as u64);

pub(crate) enum PingState {
    PingSuccess(Instant),

    AwaitingResponse {
        last_success: Instant,
        ping_request_sent: Instant
    }
}

impl PingState {
    pub(crate) fn should_send_ping(&self) -> bool {
        match self {
            PingState::PingSuccess(instant) => {
                let diff = ((Instant::now() - *instant).as_secs()) as usize;
                diff > KEEP_ALIVE / 2
            },
            PingState::AwaitingResponse { last_success: _, ping_request_sent } => {
                let diff = Instant::now() - *ping_request_sent;
                diff > PING_RETRY_DURATION
            },
        }
    }

    pub(crate) fn is_critical_delay(&self) -> bool {
        let diff = Instant::now() - *self.last_success();
        diff >= Duration::from_secs(KEEP_ALIVE as u64 - 5)
    }

    fn last_success(&self) -> &Instant {
        match self {
            PingState::PingSuccess(instant) => instant,
            PingState::AwaitingResponse { last_success, ping_request_sent: _ } => last_success,
        }
    }

    pub(crate) fn ping_sent(&mut self) {
        *self = PingState::AwaitingResponse { 
            last_success: self.last_success().clone(), 
            ping_request_sent: Instant::now() 
        };
    }

    pub(crate) fn on_ping_response(&mut self) {
        *self = PingState::PingSuccess(Instant::now())
    }

    /// Returns the duration until the next 
    pub(crate) fn ping_pause(&self) -> Option<Duration> {
        match self {
            PingState::PingSuccess(instant) => {
                let diff = Instant::now() - *instant;
                let half_keep_alive = KEEP_ALIVE_DURATION / 2;
                if diff > half_keep_alive {
                    None
                } else {
                    Some(half_keep_alive - diff)
                }
            },
            PingState::AwaitingResponse { last_success: _, ping_request_sent } => {
                let diff = Instant::now() - *ping_request_sent;
                if diff > PING_RETRY_DURATION {
                    None
                } else {
                    Some(PING_RETRY_DURATION - diff)
                }
            },
        }
    }
}