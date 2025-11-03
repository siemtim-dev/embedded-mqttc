
use embassy_sync::{blocking_mutex::raw::RawMutex, pubsub::DynPublisher};
use heapless::{Deque, String, Vec};
use mqttrs2::{Packet, Pid, QoS, Suback, Subscribe, SubscribeTopic, Unsubscribe};
use crate::{MqttError, MqttEvent, UniqueID, mutex::Mutex, state::{SendResult, connection::ConnectionState, pid::{free_pid, next_pid}}, time::{self, Duration, Instant}};

pub const RESEND_DURATION: Duration = Duration::from_secs(5);

enum RequestType {
    Subscribe(QoS),
    Unsubscribe
}

impl RequestType {

    fn is_unsub(&self) -> bool {
        match self {
            RequestType::Subscribe(_) => false,
            RequestType::Unsubscribe => true,
        }
    }
}

fn create_subscribe_topic(qos: QoS, topic: &str) -> Result<SubscribeTopic, MqttError> {
    Ok(SubscribeTopic { 
        topic_path: String::try_from(topic)
            .map_err(|_| MqttError::TopicSizeError)?, 
        qos 
    })
}

struct SubRequest <const TOPIC: usize>{
    topics: Vec<String<TOPIC>, 5>,
    request_type: RequestType,
    pid: Pid,
    unique_id: UniqueID,
}

impl<const TOPIC: usize> SubRequest<TOPIC> {

    fn create_packet(&self) -> Result<Packet<'_>, MqttError> {
        let p = match self.request_type {
            RequestType::Subscribe(qos) => Packet::Subscribe(Subscribe{
                pid: self.pid,
                topics: self.topics.iter()
                    .map(|t| create_subscribe_topic(qos, t))
                    .collect::<Result<_, _>>()?
            }),
            RequestType::Unsubscribe => Packet::Unsubscribe(Unsubscribe{
                pid: self.pid,
                topics: self.topics.iter()
                .map(|el| String::try_from(&el[..]))
                .collect::<Result<Vec<_, _>, _>>()
                .map_err(|_| MqttError::TopicSizeError)?
            }),
        };

        Ok(p)
    }

}

struct AwaitAck <const TOPIC: usize> {
    request: SubRequest<TOPIC>,
    request_sent: Instant,
}

impl<const TOPIC: usize> AwaitAck<TOPIC> {
    fn new(request: SubRequest<TOPIC>) -> Self {
        Self {
            request,
            request_sent: time::now(),
        }
    } 
}



struct SubsInner <const TOPIC: usize, const PARALLEL_REQUESTS: usize> {
    send_request: Deque<SubRequest<TOPIC>, PARALLEL_REQUESTS>,
    await_ack: Vec<AwaitAck<TOPIC>, PARALLEL_REQUESTS>,
}

impl<const TOPIC: usize, const PARALLEL_REQUESTS: usize> SubsInner<TOPIC, PARALLEL_REQUESTS> {

    fn new() -> Self {
        Self {
            send_request: Deque::new(),
            await_ack: Vec::new()
        }
    }

    fn send_requests(&mut self, connection: &impl ConnectionState) -> Result<SendResult, MqttError> {
        while let Some(request) = self.send_request.front(){
            if self.await_ack.is_full() {
                warn!("cannot send subscribe request: await_ack queue full");
                return Ok(SendResult::PartiallySent);
            }

            let packet = request.create_packet()?;
            let sent = connection.try_write_packet(&packet)?;
            
            if sent {
                let _ = self.await_ack.push(AwaitAck::new(self.send_request.pop_front().unwrap()));
            } else {
                debug!("abort sending sub/unsub request due to try_send failing");
                return Ok(SendResult::PartiallySent);
            }
        }

        Ok(SendResult::SentAll)
    }

    fn resend_requests(&mut self, connection: &impl ConnectionState) -> Result<SendResult, MqttError> {
        let now = time::now();

        for request in &mut self.await_ack {
            if now - request.request_sent > RESEND_DURATION {
                info!("resending sub/unsub {}", request.request.pid);

                let packet = request.request.create_packet()?;
                let sent = connection.try_write_packet(&packet)?;
                if sent {
                    request.request_sent = time::now();
                } else {
                    debug!("abort resending sub/unsub request due to try_send failing");
                    return Ok(SendResult::PartiallySent);
                }
            }
        }
        
        Ok(SendResult::SentAll)
    }

    fn on_suback(&mut self, suback: &Suback, publisher: DynPublisher<'_, MqttEvent>) -> Result<(), MqttError> {
        let p = self.await_ack.iter()
            .enumerate()
            .filter(|(_, await_ack)| await_ack.request.pid == suback.pid)
            .filter_map(|(index, await_ack)| match await_ack.request.request_type {
                RequestType::Subscribe(qos) => Some((index, qos)),
                RequestType::Unsubscribe => None,
            })
            .next();

        if let Some((index, qos)) = p {
            free_pid(suback.pid);
            let request = self.await_ack.remove(index);
            Self::check_suback(&suback, qos, &request, publisher);
        }

        Ok(())
    }

    fn check_suback(suback: &Suback, expected_qos: QoS, request: &AwaitAck<TOPIC>, publisher: DynPublisher<'_, MqttEvent>) {
        for (result, expected) in suback.return_codes.iter().zip(request.request.topics.iter()) {
            match result {
                mqttrs2::SubscribeReturnCodes::Success(qos) if *qos == expected_qos => {
                    debug!("subscribed to {} with {}", expected, qos);
                },
                mqttrs2::SubscribeReturnCodes::Success(qos) => {
                    warn!("subscribed to {} with {} but expected {}", expected, qos, expected_qos);
                },
                mqttrs2::SubscribeReturnCodes::Failure => {
                    error!("subscribe to {} failed", expected);
                    publisher.publish_immediate(MqttEvent::SubscribeDone(request.request.unique_id, Err(MqttError::SubscribeOrUnsubscribeFailed)));
                    return;
                },
            }
        }

        // TODO return not the expectd but the acual qos
        publisher.publish_immediate(MqttEvent::SubscribeDone(request.request.unique_id, Ok(expected_qos)));
    }

    fn on_unsuback(&mut self, pid: Pid, publisher: DynPublisher<'_, MqttEvent>) {
        let p = self.await_ack.iter().position(|await_ack|{
            await_ack.request.pid == pid && await_ack.request.request_type.is_unsub()
        });

        if let Some(p) = p {
            let request = self.await_ack.remove(p);
            free_pid(request.request.pid);
            publisher.publish_immediate(MqttEvent::UnsubscribeDone(request.request.unique_id));
        }
    }

    pub fn send(&mut self, connection: &impl ConnectionState) -> Result<SendResult, MqttError> {
        let result = self.send_requests(connection)?
            .next_sync(|| self.resend_requests(connection))?;
        Ok(result)
    }
    
}



pub struct Subs<M: RawMutex, const TOPIC: usize, const PARALLEL_REQUESTS: usize> {
    inner: Mutex<M, SubsInner<TOPIC, PARALLEL_REQUESTS>, PARALLEL_REQUESTS>
}

impl<M: RawMutex, const TOPIC: usize, const PARALLEL_REQUESTS: usize> Subs<M, TOPIC, PARALLEL_REQUESTS> {

    pub fn new() -> Self {
        Self {
            inner: Mutex::new(SubsInner::new())
        }
    }

    pub async fn add_unsubscribe_request(&self, topics: &[&str], unique_id: UniqueID) {
        self.inner.try_with_lock(|inner|  {
            if inner.send_request.is_full() {
                Result::<bool, MqttError>::Ok(false)
            } else {
                let topics = topics.iter()
                    .map(|topic| String::try_from(*topic).unwrap())
                    .collect();

                let request = SubRequest {
                    topics,
                    request_type: RequestType::Unsubscribe,
                    pid: next_pid(),
                    unique_id
                };

                let _ = inner.send_request.push_back(request);
                Ok(true)
            }
        }).await.unwrap();
    }

    pub async fn add_subscribe_request(&self, topics: &[&str], qos: QoS, unique_id: UniqueID) {
        self.inner.try_with_lock(|inner|  {
            if inner.send_request.is_full() {
                Result::<bool, MqttError>::Ok(false)
            } else {
                let topics = topics.iter()
                    .map(|topic| String::try_from(*topic).unwrap())
                    .collect();

                let request = SubRequest {
                    topics,
                    request_type: RequestType::Subscribe(qos),
                    pid: next_pid(),
                    unique_id
                };

                let _ = inner.send_request.push_back(request);
                Ok(true)
            }
        }).await.unwrap();
    }

    pub async fn send(&self, connection: &impl ConnectionState) -> Result<SendResult, MqttError> {
        let mut inner = self.inner.lock().await;
        inner.send(connection)
    }

    pub async fn on_suback(&self, suback: &Suback, publisher: DynPublisher<'_, MqttEvent>) -> Result<(), MqttError> {
        let mut inner = self.inner.lock().await;
        inner.on_suback(suback, publisher)
    }

    pub async fn on_unsuback(&self, pid: Pid, publisher: DynPublisher<'_, MqttEvent>) {
        let mut inner = self.inner.lock().await;
        inner.on_unsuback(pid, publisher)
    }
}



#[cfg(test)]
mod tests {
    use core::pin::Pin;

    use embassy_sync::{blocking_mutex::raw::CriticalSectionRawMutex, pubsub::PubSubChannel};
    use mqttrs2::{QoS, Suback, SubscribeReturnCodes};

    use crate::{MqttEvent, state::{SendResult, connection::test::DummyConnectionState}};

    use super::Subs;

    use crate::testutils::*;


    #[test]
    fn test_subscribe() {

        let connection_state = DummyConnectionState::new();

        let subs = Subs::<CriticalSectionRawMutex, 128, 4>::new();

        let req_fut = subs.add_subscribe_request(&["test/topic"], QoS::AtMostOnce, 6.into());
        assert_ready_pin(req_fut);

        let events = PubSubChannel::<CriticalSectionRawMutex, MqttEvent, 1, 1, 1>::new();
        let mut event_subscriber = events.dyn_subscriber().unwrap();

        let send_fut = subs.send(&connection_state);
        let send_result = assert_ready_pin(send_fut).unwrap();

        assert_eq!(send_result, SendResult::SentAll);

        let pid = connection_state.assert_packet_written(|p|{
            match p {
                mqttrs2::Packet::Subscribe(subscribe) => subscribe.pid,
                p => panic!("sent unexpected packet {:?}", p),
            }
        });

        let mut suback = Suback{
            pid,
            return_codes: heapless::Vec::new()
        };
        let _ = suback.return_codes.push(SubscribeReturnCodes::Success(QoS::AtMostOnce));
        
        let on_suback_fut = subs.on_suback(&suback, events.dyn_publisher().unwrap());
        assert_ready_pin(on_suback_fut).unwrap();

        let event = event_subscriber.try_next_message_pure().expect("expect a sub success event");
        assert_eq!(event, MqttEvent::SubscribeDone(6.into(), Ok(QoS::AtMostOnce)))
    }

    #[test]
    fn test_parallel_subscribe() {

        let connection_state = DummyConnectionState::new();

        let subs = Subs::<CriticalSectionRawMutex, 128, 4>::new();

        let req_fut = subs.add_subscribe_request(&["test/topic/0"], QoS::AtMostOnce, 0.into());
        assert_ready_pin(req_fut);

        let req_fut = subs.add_subscribe_request(&["test/topic/1"], QoS::AtMostOnce, 1.into());
        assert_ready_pin(req_fut);

        let events = PubSubChannel::<CriticalSectionRawMutex, MqttEvent, 1, 1, 1>::new();
        let mut event_subscriber = events.dyn_subscriber().unwrap();

        // "Send" pubscribe packets

        for i in 0..2 {
            let mut send_fut = subs.send(&connection_state);
            let send_fut = unsafe { Pin::new_unchecked(&mut send_fut) };
            let send_result = assert_ready(send_fut).unwrap();

            let expected_send_result = if i + 1 < 2 { SendResult::PartiallySent } else { SendResult::SentAll };
            assert_eq!(send_result, expected_send_result);

            let pid = connection_state.assert_packet_written(|p|{
                match p {
                    mqttrs2::Packet::Subscribe(subscribe) => subscribe.pid,
                    p => panic!("sent unexpected packet {:?}", p),
                }
            });

            let mut suback = Suback{
                pid,
                return_codes: heapless::Vec::new()
            };
            let _ = suback.return_codes.push(SubscribeReturnCodes::Success(QoS::AtMostOnce));
            
            let on_suback_fut = subs.on_suback(&suback, events.dyn_publisher().unwrap());
            assert_ready_pin(on_suback_fut).unwrap();

            let event = event_subscriber.try_next_message_pure().expect("expect a sub success event");
            assert_eq!(event, MqttEvent::SubscribeDone(i.into(), Ok(QoS::AtMostOnce)))
        }

        
    }

    #[test]
    fn test_unsubscribe() {
        let connection_state = DummyConnectionState::new();

        let subs = Subs::<CriticalSectionRawMutex, 128, 4>::new();

        let req_fut = subs.add_unsubscribe_request(&["test/topic"], 6.into());
        assert_ready_pin(req_fut);

        let events = PubSubChannel::<CriticalSectionRawMutex, MqttEvent, 1, 1, 1>::new();
        let mut event_subscriber = events.dyn_subscriber().unwrap();

        let send_fut = subs.send(&connection_state);
        let send_result = assert_ready_pin(send_fut).unwrap();

        assert_eq!(send_result, SendResult::SentAll);

        let pid = connection_state.assert_packet_written(|p|{
            match p {
                mqttrs2::Packet::Unsubscribe(unsubscribe) => unsubscribe.pid,
                p => panic!("sent unexpected packet {:?}", p),
            }
        });
        
        let on_unsuback_fut = subs.on_unsuback(pid, events.dyn_publisher().unwrap());
        assert_ready_pin(on_unsuback_fut);

        let event = event_subscriber.try_next_message_pure().expect("expect a sub success event");
        assert_eq!(event, MqttEvent::UnsubscribeDone(6.into()))
    }

}



