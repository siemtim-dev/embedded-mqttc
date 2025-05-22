
use core::cell::RefCell;

use buffer::BufferWriter;
use embassy_sync::blocking_mutex::{raw::CriticalSectionRawMutex, Mutex};
use crate::{time::{Duration, Instant}, AutoSubscribe};
use heapless::{FnvIndexMap, String, Vec};
use mqttrs::{encode_slice, Packet, Pid, QoS, Suback, Subscribe, SubscribeReturnCodes, SubscribeTopic, Unsubscribe};
use crate::queue_vec::split::{QueuedVecInner, WithQueuedVecInner};

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

struct InitialSubscribes {
    initial_subscriptions_pending: FnvIndexMap<Pid, bool, MAX_CONCURRENT_REQUESTS>,
}

impl InitialSubscribes {

    fn new() -> Self {
        Self { 
            initial_subscriptions_pending: FnvIndexMap::new()
        }
    }

    fn on_initial_suback(&mut self, pid: Pid, result: &mut Vec<MqttEvent, 2>) {
            
        let initial_sub_op = self.initial_subscriptions_pending.get_mut(&pid);
        if let Some(initial_sub) = initial_sub_op{
            *initial_sub = true;
        } else {
            error!("on_initial_suback(): {} not in self.initial_subscriptions_pending", pid);
            return;
        }

        let remaining = self.initial_subscriptions_pending.iter()
            .filter(|el| *el.1 == false)
            .count();

        if remaining == 0 {
            info!("initial subscribes done");
            result.push(MqttEvent::InitialSubscribesDone).unwrap();
        } else {
            debug!("initial subscribe {} done, but {} remaining", pid, remaining);
        }
    }

}

pub(crate) struct SubQueue {
    inner: Mutex<CriticalSectionRawMutex, RefCell<QueuedVecInner<InitialSubscribes, Request, MAX_CONCURRENT_REQUESTS>>>
}

impl WithQueuedVecInner<InitialSubscribes, Request, MAX_CONCURRENT_REQUESTS> for SubQueue {
    fn with_queued_vec_inner<F, O>(&self, operation: F) -> O where F: FnOnce(&mut QueuedVecInner<InitialSubscribes, Request, MAX_CONCURRENT_REQUESTS>) -> O {
        self.inner.lock(|inner| {
            let mut inner = inner.borrow_mut();
            operation(&mut inner)
        })
    }
}

impl SubQueue {
    pub(crate) fn new() -> Self {
        Self {
            inner: Mutex::new(RefCell::new(QueuedVecInner::new(InitialSubscribes::new())))
        }
    }

    pub(crate) async fn push_subscribe(&self, topic: Topic, pid: Pid, external_id: UniqueID, qos: QoS) {
        let req = Request::subscribe(topic, pid, external_id, qos, false);
        self.push(req).await;
    }

    pub(crate) async fn push_unsubscribe(&self, topic: Topic, pid: Pid, external_id: UniqueID) {
        let req = Request::unsubscribe(topic, pid, external_id);
        self.push(req).await;
    }

    /**
     * Adds the subscription requests from the auto subscribe client option. 
     * Current design decision: current requests are removed!
     */
    pub(super) fn add_auto_subscribes<F: FnMut() -> Pid>(&self, auto_subscribes: &[AutoSubscribe], mut pid_source: F) {

        self.inner.lock(|inner| {
            let mut inner = inner.borrow_mut();
            let (requests, initial_subscribes) = inner.working_copy();

            if auto_subscribes.len() > requests.data.capacity() {
                panic!("Internal logic error: number of auto subscribes must be <= subscribe request capacity.");
            }

            for auto_subscribe in auto_subscribes {
                if requests.data.is_full() {
                    requests.data.remove(0);
                }

                let pid = pid_source();
                let id = UniqueID::new();
                let request = Request::subscribe(auto_subscribe.topic.clone(), pid, id, auto_subscribe.qos, true);

                requests.data.push(request)
                    .map_err(|_| "unexpected error: could not add auto subscribe request to queue")
                    .unwrap();

                initial_subscribes.initial_subscriptions_pending.insert(pid, false).unwrap();

                info!("added auto subscribe request to {}", &auto_subscribe.topic);
            }
        })
    } 

    /// Sends subscribe and unsubscribe
    pub(crate) fn process(&self, send_buffer: &mut impl BufferWriter) -> Result<(), MqttError> {
        self.operate(|requests|{

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

    pub(crate) fn process_suback(&self, suback: &Suback) -> Vec<MqttEvent, 2> {
        self.inner.lock(|inner|{
            let mut inner = inner.borrow_mut();
            let (requests, initial_subscriptions) = inner.working_copy();

            let mut result = Vec::new();

            let op = requests.data.iter_mut().find(|el| el.pid == suback.pid);

            if let Some(request) = op {
                if request.request_type.is_subscribe() && request.state.is_await_ack() {
                    debug!("suback processed for packet {}", request.pid);

                    request.state = RequestState::Done;

                    const FAIL: SubscribeReturnCodes = SubscribeReturnCodes::Failure;
                    let code = suback.return_codes.first().unwrap_or(&FAIL);

                    if  let SubscribeReturnCodes::Success(qos) = code {
                        if request.initial {
                            initial_subscriptions.on_initial_suback(request.pid, &mut result);
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

            requests.data.retain(|el| el.state != RequestState::Done);
            result
        })
    }

    pub(crate) fn process_unsuback(&self, pid: &Pid) -> Option<MqttEvent> {
        self.operate(|requests|{

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
    use buffer::{new_stack_buffer, ReadWrite};
    use heapless::Vec;
    use mqttrs::{Packet, Pid, QoS, Suback, SubscribeReturnCodes};
    use network::mqtt::ReadMqttPacket;

    use crate::{state::{pid::PidSource, sub::SubQueue}, AutoSubscribe, MqttEvent, Topic, UniqueID};


    #[tokio::test]
    #[ntest::timeout(5000)]
    async fn test_subscribe () {

        let subs = SubQueue::new();

        let mut send_buffer = new_stack_buffer::<1024>();

        {
            let mut writer = send_buffer.create_writer();
            subs.process(&mut writer).unwrap();
        }

        assert_eq!(send_buffer.remaining_len(), 0);
        let pid = Pid::new() + 16;

        {
            let mut topic = Topic::new();
            topic.push_str("test/a/topic").unwrap();
            subs.push_subscribe(topic, pid, UniqueID(5), QoS::AtMostOnce).await;
        }

        {
            let mut writer = send_buffer.create_writer();
            subs.process(&mut writer).unwrap();
        }

        {
            let reader = send_buffer.create_reader();
            let p = reader.read_packet().unwrap()
                .expect("expected a subscribe packet but got none");
            if let Packet::Subscribe(s) = p {
                assert_eq!(&s.topics[0].topic_path[..], "test/a/topic");
                assert_eq!(s.pid, pid);
                assert_eq!(s.topics[0].qos, QoS::AtMostOnce);
            } else {
                panic!("expected subscribe packet");
            }
        }

    }

    #[tokio::test]
    #[ntest::timeout(5000)]
    async fn test_auto_subscribe () {

        let subs = SubQueue::new();
        let mut send_buffer = new_stack_buffer::<1024>();

        let pid = Pid::new() + 16;
        let auto_subscribes = [
            AutoSubscribe::new("some/default/topic", QoS::ExactlyOnce)
        ];
        subs.add_auto_subscribes(&auto_subscribes, || pid);

        {
            let mut writer = send_buffer.create_writer();
            subs.process(&mut writer).unwrap();
        }

        {
            let reader = send_buffer.create_reader();
            let p = reader.read_packet().unwrap()
                .expect("expected a subscribe packet but got none");
            if let Packet::Subscribe(s) = p {
                assert_eq!(&s.topics[0].topic_path[..], "some/default/topic");
                assert_eq!(s.pid, pid);
                assert_eq!(s.topics[0].qos, QoS::ExactlyOnce);
            } else {
                panic!("expected subscribe packet");
            }
        }

        {
            let mut suback = Suback{
                pid,
                return_codes: Vec::new()
            };
            suback.return_codes.push(SubscribeReturnCodes::Success(QoS::ExactlyOnce)).unwrap();
            let events = subs.process_suback(&suback);

            assert_eq!(events.len(), 2);
            let mut found_subscription_result = false;
            let mut found_initial_subscriptions_done = false;
            
            for event in events {
                if let MqttEvent::SubscribeResult(_, res) = event {
                    let qos = res.unwrap();
                    assert_eq!(qos, QoS::ExactlyOnce);
                    found_subscription_result = true;
                } else if let MqttEvent::InitialSubscribesDone = event {
                    found_initial_subscriptions_done = true;
                } else {
                    panic!("unexpected event: {:?}", event);
                }
            }

            assert!(found_subscription_result);
            assert!(found_initial_subscriptions_done);
        }
    }


    #[tokio::test]
    #[ntest::timeout(5000)]
    async fn test_multi_auto_subscribe () {

        let subs = SubQueue::new();
        let mut send_buffer = new_stack_buffer::<1024>();
        let pid_src = PidSource::new();

        let auto_subscribes = [
            AutoSubscribe::new("some/default/topic/1", QoS::ExactlyOnce),
            AutoSubscribe::new("some/default/topic/2", QoS::AtLeastOnce)
        ];
        subs.add_auto_subscribes(&auto_subscribes, || pid_src.next_pid());

        {
            let mut writer = send_buffer.create_writer();
            subs.process(&mut writer).unwrap();
        }

        let mut pid_1 = None;
        let mut pid_2 = None;

        for _ in 0..2 {
            let reader = send_buffer.create_reader();
            let p = reader.read_packet().unwrap()
                .expect("expected a subscribe packet but got none");
            if let Packet::Subscribe(s) = p {
                if &s.topics[0].topic_path[..] == "some/default/topic/1" {
                    pid_1 = Some(s.pid);
                    assert_eq!(s.topics[0].qos, QoS::ExactlyOnce);
                } else if &s.topics[0].topic_path[..] == "some/default/topic/2" {
                    pid_2 = Some(s.pid);
                    assert_eq!(s.topics[0].qos, QoS::AtLeastOnce);
                } else {
                    panic!("unexpected subscribe to topic {}", &s.topics[0].topic_path[..]);
                }
            } else {
                panic!("expected subscribe packet");
            }
        }

        assert!(pid_1.is_some());
        assert!(pid_2.is_some());

        {
            let mut suback = Suback{
                pid: pid_1.unwrap(),
                return_codes: Vec::new()
            };
            suback.return_codes.push(SubscribeReturnCodes::Success(QoS::ExactlyOnce)).unwrap();
            let events = subs.process_suback(&suback);

            assert_eq!(events.len(), 1);
            let event = events.first().unwrap();

            if let MqttEvent::SubscribeResult(_, res) = event {
                let qos = res.as_ref().unwrap();
                assert_eq!(*qos, QoS::ExactlyOnce);
            } else {
                panic!("expectes subscribe result");
            }
        }

        {
            let mut suback = Suback{
                pid: pid_2.unwrap(),
                return_codes: Vec::new()
            };
            suback.return_codes.push(SubscribeReturnCodes::Success(QoS::AtLeastOnce)).unwrap();
            let events = subs.process_suback(&suback);

            assert_eq!(events.len(), 2);
            let mut found_subscription_result = false;
            let mut found_initial_subscriptions_done = false;
            
            for event in events {
                if let MqttEvent::SubscribeResult(_, res) = event {
                    let qos = res.unwrap();
                    assert_eq!(qos, QoS::AtLeastOnce);
                    found_subscription_result = true;
                } else if let MqttEvent::InitialSubscribesDone = event {
                    found_initial_subscriptions_done = true;
                } else {
                    panic!("unexpected event: {:?}", event);
                }
            }

            assert!(found_subscription_result);
            assert!(found_initial_subscriptions_done);
        }
    }

    
}