
use core::cell::RefCell;

use buffer::BufferWriter;
use embassy_sync::blocking_mutex::{raw::CriticalSectionRawMutex, Mutex};
use crate::{time::{Duration, Instant}, AutoSubscribe};
use heapless::{String, Vec};
use mqttrs::{encode_slice, Packet, Pid, QoS, Suback, Subscribe, SubscribeReturnCodes, SubscribeTopic, Unsubscribe};
use queue_vec::QueuedVec;

use crate::{time, MqttError, MqttEvent, Topic, UniqueID};

const RESUBSCRIBE_DURATION: Duration = Duration::from_secs(5);
pub const MAX_CONCURRENT_REQUESTS: usize = 4;

#[derive(Debug, Eq, PartialEq, Clone, Copy)]
#[cfg_attr(feature = "defmt", derive(defmt::Format))]
pub enum RequestType {
    Subscribe(QoS),
    Unsubscribe
}

impl RequestType {
    fn is_subscribe(&self) -> bool {
        if let Self::Subscribe(_) = self {
            true
        } else {
            false
        }
    }
}

#[derive(Debug, PartialEq, Clone, Copy)]
#[cfg_attr(feature = "defmt", derive(defmt::Format))]
pub enum RequestState {
    Initial,
    AwaitAck(Instant),
    Done
}

impl RequestState {
    fn should_publish(&self, now: Instant) -> bool {
        match self {
            Self::Initial => true,
            Self::AwaitAck(instant) => (now - *instant) > RESUBSCRIBE_DURATION,
            Self::Done => false
        }
    }

    fn is_await_ack(&self) -> bool{
        if let Self::AwaitAck(_) = self {
            true
        } else {
            false
        }
    }
}

#[derive(Debug)]
#[cfg_attr(feature = "defmt", derive(defmt::Format))]
pub struct Request {
    request_type: RequestType,
    topic: Topic,
    pid: Pid,
    external_id: UniqueID,
    state: RequestState,
    initial: bool
}

impl Request {
    fn subscribe(topic: Topic, pid: Pid, external_id: UniqueID, qos: QoS, initial: bool) -> Self {
        Self {
            topic, pid, external_id,
            request_type: RequestType::Subscribe(qos),
            state: RequestState::Initial,
            initial
        }
    }

    fn unsubscribe(topic: Topic, pid: Pid, external_id: UniqueID) -> Self {
        Self {
            topic, pid, external_id,
            request_type: RequestType::Unsubscribe,
            state: RequestState::Initial,
            initial: false // There is no initial unsubscribe
        }
    }

    fn on_send_success(&mut self) {  
        match self.state {
             RequestState::Initial => {
                self.state = RequestState::AwaitAck(time::now());
             },
             _ => {}
         }
    }

    fn send(&mut self, send_buffer: &mut impl BufferWriter) -> Result<(), MqttError>{
        let packet = match self.request_type {
            RequestType::Subscribe(qos) => {
                let mut topics = Vec::<SubscribeTopic, 5>::new();
                let mut topic = String::<256>::new();
                topic.push_str(&self.topic).unwrap();

                topics.push(SubscribeTopic {
                    topic_path: topic, 
                    qos
                }).unwrap();

                Packet::Subscribe(Subscribe{
                    pid: self.pid.clone(),
                    topics
                })
            },
            RequestType::Unsubscribe => {
                let mut topics = Vec::<String<256>, 5>::new();
                let mut topic = String::<256>::new();
                topic.push_str(&self.topic).unwrap();
                topics.push(topic).unwrap();

                Packet::Unsubscribe(Unsubscribe{
                    pid: self.pid.clone(),
                    topics
                })
            },
        };

        let result = encode_slice(&packet, send_buffer);
        match result {
            Ok(n) => {
                send_buffer.commit(n).unwrap();
                self.on_send_success();
                debug!("{} packet {} written to send buffer; len = {}", self.request_type, self.pid, n);
                Ok(())
            },
            Err(mqttrs::Error::WriteZero) => {
                warn!("cannot write {} packet so send buffer: no capacity ({} bytes left)", 
                    self.request_type, send_buffer.remaining_capacity());
                Ok(())
            },
            Err(e) => {
                error!("error encoding subscribe / unsubscribe packet: {}", e);
                Err(MqttError::CodecError)
            }
        }

    }
}

pub(crate) struct SubQueue {
    requests: QueuedVec<CriticalSectionRawMutex, Request, MAX_CONCURRENT_REQUESTS>,
    initial_subscriptions_pending: Mutex<CriticalSectionRawMutex, RefCell<Vec<Pid, MAX_CONCURRENT_REQUESTS>>>
}

impl SubQueue {
    pub(crate) fn new() -> Self {
        Self {
            requests: QueuedVec::new(),
            initial_subscriptions_pending: Mutex::new(RefCell::new(Vec::new()))
        }
    }

    pub(crate) async fn push_subscribe(&self, topic: Topic, pid: Pid, external_id: UniqueID, qos: QoS) {
        let req = Request::subscribe(topic, pid, external_id, qos, false);
        self.requests.push(req).await;
    }

    pub(crate) async fn push_unsubscribe(&self, topic: Topic, pid: Pid, external_id: UniqueID) {
        let req = Request::unsubscribe(topic, pid, external_id);
        self.requests.push(req).await;
    }

    /**
     * Adds the subscription requests from the auto subscribe client option. 
     * Current design decision: current requests are removed!
     */
    pub(super) fn add_auto_subscribes<F: FnMut() -> Pid>(&self, auto_subscribes: &[AutoSubscribe], mut pid_source: F) {
        self.requests.operate(move |requests| {
            if auto_subscribes.len() > requests.capacity() {
                panic!("Internal logic error: number of auto subscribes must be <= subscribe request capacity.");
            }

            for auto_subscribe in auto_subscribes {
                if requests.is_full() {
                    requests.remove(0);
                }

                let pid = pid_source();
                let id = UniqueID::new();
                let request = Request::subscribe(auto_subscribe.topic.clone(), pid, id, auto_subscribe.qos, true);

                requests.push(request)
                    .map_err(|_| "unexpected error: could not add auto subscribe request to queue")
                    .unwrap();

                info!("added auto subscribe request to {}", &auto_subscribe.topic);
            }
        })
    } 

    /// Sends subscribe and unsubscribe
    pub(crate) fn process(&self, send_buffer: &mut impl BufferWriter) -> Result<(), MqttError> {
        self.requests.operate(|requests|{

            for request in requests.iter_mut() {
                // TODO answer quetsion:
                //   Should the loop `break;` if a publish cannot be written to buffer 
                //   beause of insufficient space?
                if request.state.should_publish(time::now()) {
                    request.send(send_buffer)?;
                }
            }
            Ok(())
        })
    }

    fn on_initial_suback(&self, pid: Pid, result: &mut Vec<MqttEvent, 2>) {
        self.initial_subscriptions_pending.lock(|inner|{
            let mut inner = inner.borrow_mut();
            inner.retain(|el| *el != pid);

            if inner.is_empty() {
                info!("initial subscribes done");
                result.push(MqttEvent::InitialSubscribesDone).unwrap();
            } else {
                debug!("initial subscribe {} done, but {} remaining", pid, inner.len());
            }
        })
    }

    pub(crate) fn process_suback(&self, suback: &Suback) -> Vec<MqttEvent, 2> {
        self.requests.operate(|requests|{
            let mut result = Vec::new();

            let op = requests.iter_mut().find(|el| el.pid == suback.pid);

            if let Some(request) = op {
                if request.request_type.is_subscribe() && request.state.is_await_ack() {
                    debug!("suback processed for packet {}", request.pid);

                    request.state = RequestState::Done;

                    const FAIL: SubscribeReturnCodes = SubscribeReturnCodes::Failure;
                    let code = suback.return_codes.first().unwrap_or(&FAIL);

                    if  let SubscribeReturnCodes::Success(qos) = code {
                        if request.initial {
                            self.on_initial_suback(request.pid, &mut result);
                        }

                        result.push(MqttEvent::SubscribeResult(request.external_id, Ok(qos.clone()))).unwrap();
                    } else {
                        result.push(MqttEvent::SubscribeResult(request.external_id, Err(MqttError::SubscribeOrUnsubscribeFailed))).unwrap();
                    }
                } else {
                    warn!("illegal state: received suback for packet {} but packet has state {}", request.pid, request.state);
                    // Add nothing to result vec
                }
            } else {
                warn!("received suback for packet {} but packet is unknown", suback.pid);
                // Add nothing to result vec
            };

            requests.retain(|el| el.state != RequestState::Done);
            result
        })
    }

    pub(crate) fn process_unsuback(&self, pid: &Pid) -> Option<MqttEvent> {
        self.requests.operate(|requests|{

            let op = requests.iter_mut().find(|el| el.pid == *pid);

            let result = if let Some(request) = op {
                if request.request_type == RequestType::Unsubscribe && request.state.is_await_ack() {
                    debug!("suback processed for packet {}", request.pid);

                    request.state = RequestState::Done;

                    Some(MqttEvent::UnsubscribeResult(request.external_id, Ok(())))
                } else {
                    warn!("illegal state: received unsuback for packet {} but packet has state {}", request.pid, request.state);
                    None
                }
            } else {
                warn!("received unsuback for packet {} but packet is unknown", pid);
                None
            };

            requests.retain(|el| el.state != RequestState::Done);
            result
        })
    }
}

#[cfg(test)]
mod tests {
    
}