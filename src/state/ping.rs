
use core::cell::RefCell;

use embassy_sync::blocking_mutex::raw::RawMutex;
use embassy_sync::blocking_mutex::Mutex;
use mqttrs2::Packet;

use crate::{state::{connection::ConnectionState, SendResult}, time::{ self, Duration, Instant }, MqttError};

use super::KEEP_ALIVE;

const PING_RETRY_DURATION: Duration = Duration::from_secs(5);
const KEEP_ALIVE_DURATION: Duration = Duration::from_secs(KEEP_ALIVE as u64);
const ERROR_CORRECTING_DURATION: Duration = Duration::from_millis(10);

#[derive(Debug, Clone, PartialEq)]
#[cfg_attr(feature = "defmt", derive(defmt::Format))]
enum PingStateInner {
    PingSuccess(Instant),

    AwaitingResponse {
        last_success: Instant,
        ping_request_sent: Instant
    }
}

impl PingStateInner {

    fn on_ping_response(&mut self) {
        let now = time::now();
        *self = Self::PingSuccess(now)
    }

    // /// Returns the duration until the next 
    // pub(crate) fn ping_pause(&self) -> Option<Duration> {
    //     let now = time::now();
    //     match self {
    //         Self::PingSuccess(instant) => {
    //             let diff = now - *instant;
    //             let half_keep_alive = KEEP_ALIVE_DURATION / 2;
    //             if diff > half_keep_alive {
    //                 debug!("send ping now!");
    //                 None
    //             } else {
    //                 let d = half_keep_alive - diff + ERROR_CORRECTING_DURATION;
    //                 trace!("send ping in {}", d);
    //                 Some(d)
    //             }
    //         },
    //         Self::AwaitingResponse { last_success: _, ping_request_sent } => {
    //             let diff = now - *ping_request_sent;
    //             if diff > PING_RETRY_DURATION {
    //                 None
    //             } else {
    //                 Some(PING_RETRY_DURATION - diff + ERROR_CORRECTING_DURATION)
    //             }
    //         },
    //     }
    // }

    fn ping_pause(&self) -> time::SleepFuture {
        let now = time::now();

        let duration = match self {
            Self::PingSuccess(instant) => {
                if now - *instant > KEEP_ALIVE_DURATION / 2 {
                    Duration::from_secs(0)
                } else {
                    (KEEP_ALIVE_DURATION / 2) - (now - *instant) + ERROR_CORRECTING_DURATION
                }
            },
            Self::AwaitingResponse { last_success: _, ping_request_sent } => {
                let diff = now - *ping_request_sent;
                if diff > PING_RETRY_DURATION {
                    Duration::from_secs(0)
                } else {
                    PING_RETRY_DURATION - diff + ERROR_CORRECTING_DURATION
                }
            },
        };

        info!("next ping in {} s", duration.as_secs());

        time::sleep(duration)
    }


    fn send(&mut self, connection: &impl ConnectionState) -> Result<SendResult, MqttError> {
        let now = time::now();

        match self {
            Self::PingSuccess(instant) if now - *instant > KEEP_ALIVE_DURATION / 2 => {
                info!("time to send ping");

                let sent = connection.try_write_packet(&Packet::Pingreq)?;
                if sent {
                    *self = Self::AwaitingResponse { 
                        last_success: *instant, 
                        ping_request_sent: now
                    };
                    Ok(SendResult::SentAll)
                } else {
                    Ok(SendResult::PartiallySent)
                }
            },
            Self::AwaitingResponse { last_success: _, ping_request_sent } 
                if now - *ping_request_sent > PING_RETRY_DURATION => {
                warn!("resend ping, no response from broker");

                let sent = connection.try_write_packet(&Packet::Pingreq)?;
                if sent {
                    *ping_request_sent = now;
                    Ok(SendResult::SentAll)
                } else {
                    Ok(SendResult::PartiallySent)
                }
            },
            _ => {
                debug!("no ping required");
                Ok(SendResult::SentAll)
            }
        }
    }
}

pub struct PingState<M: RawMutex> {
    inner: Mutex<M, RefCell<PingStateInner>>
}

impl<M: RawMutex> PingState<M> {

    pub fn new() -> Self {
        Self {
            inner: Mutex::new(RefCell::new(PingStateInner::PingSuccess(time::now())))
        }
    }

    pub fn ping_pause(&self) -> time::SleepFuture {
        self.inner.lock(|inner| {
            inner.borrow().ping_pause()

        })
    }

    pub fn send(&self, connection: &impl ConnectionState) -> Result<SendResult, MqttError> {
        self.inner.lock(|inner| {
            inner.borrow_mut().send(connection)

        })
    }

    pub fn on_ping_response(&self) {
        self.inner.lock(|inner| {
            inner.borrow_mut().on_ping_response();

        })
    }

}

// #[cfg(all(test, feature = "std"))]
// mod tests {
//     use crate::time::{self, Duration};

//     use crate::state::KEEP_ALIVE;

//     use super::PingStateInner;


//     #[test]
//     fn test_should_send_ping_after_success() {
//         time::test_time::set_static_now();

//         let start = time::now();
//         let ping_state = PingStateInner::PingSuccess(start.clone());

//         let a_bit_later = start + Duration::from_secs((KEEP_ALIVE / 2 - 3) as u64);
//         time::test_time::set_time(a_bit_later);
//         assert_eq!(ping_state.should_send_ping(), false);
//         assert_eq!(ping_state.is_critical_delay(), false);

//         let later = start + Duration::from_secs((KEEP_ALIVE / 2 + 5) as u64);
//         time::test_time::set_time(later);
//         assert_eq!(ping_state.should_send_ping(), true);
//         assert_eq!(ping_state.is_critical_delay(), false);

//         let too_late = start + Duration::from_secs(KEEP_ALIVE as u64);
//         time::test_time::set_time(too_late);
//         assert_eq!(ping_state.is_critical_delay(), true);
//     }

//     #[test]
//     fn test_sould_send_ping_waiting() {
//         time::test_time::set_static_now();

//         let start = time::now();
//         let ping_state = PingStateInner::AwaitingResponse { 
//             last_success: start - Duration::from_secs((KEEP_ALIVE / 2 + 4) as u64), 
//             ping_request_sent: start
//         };

//         let a_bit_later = start + Duration::from_secs(5);
//         time::test_time::set_time(a_bit_later);
//         assert_eq!(ping_state.should_send_ping(), false);
//         assert_eq!(ping_state.is_critical_delay(), false);

//         let later = start + Duration::from_secs(11);
//         time::test_time::set_time(later);
//         assert_eq!(ping_state.should_send_ping(), true);
//         assert_eq!(ping_state.is_critical_delay(), false);

//         let too_late = start + Duration::from_secs(KEEP_ALIVE as u64);
//         time::test_time::set_time(too_late);
//         assert_eq!(ping_state.is_critical_delay(), true);
//     }

//     #[test]
//     fn test_on_ping_sent () {
//         time::test_time::set_static_now();
//         let start = time::now();
        
//         let mut ping_state = PingStateInner::PingSuccess(start.clone());

//         let ping_sent = start + Duration::from_secs(20);
//         time::test_time::set_time(ping_sent);
//         ping_state.ping_sent();

//         assert_eq!(ping_state, PingStateInner::AwaitingResponse { 
//             last_success: start, 
//             ping_request_sent: ping_sent 
//         });
//     }

//     #[test]
//     fn test_ping_pause() {
//         time::test_time::set_static_now();
//         let start = time::now();
//         let ping_state = PingStateInner::PingSuccess(start.clone());

//         let pause = ping_state.ping_pause().expect("there must be a ping pause");

//         assert_eq!(ping_state.should_send_ping(), false);

//         time::test_time::advance_time(pause - Duration::from_millis(10));

//         assert_eq!(ping_state.should_send_ping(), false);

//         time::test_time::advance_time(Duration::from_millis(10));
        
//         assert!(! ping_state.is_critical_delay());
//         assert_eq!(ping_state.ping_pause(), None);
//         assert_eq!(ping_state.should_send_ping(), true);
        
//     }

// }