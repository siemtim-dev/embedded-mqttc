
use buffer::BufferWriter;
use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use crate::time::{Duration, Instant};
use mqttrs::{encode_slice, Error, Packet, Pid};
use queue_vec::{split::WithQueuedVecInner, QueuedVec};

use crate::{io::AsyncSender, time, MqttError, MqttEvent, MqttPublish, UniqueID};

const MAX_CONCURRENT_PUBLISHES: usize = 8;

#[derive(Debug, PartialEq, Clone, Copy)]
#[cfg_attr(feature = "defmt", derive(defmt::Format))]
enum RequestState {
    
    /// Initial Puhlish State for all publishes: Nothing is done yet
    Initial,

    // QoS 1 At Least Once
    /// Wait for the broker to ack the publish
    /// The [`Instant`] specifies since when waiting
    AwaitPuback(Instant),

    // QoS 2 Exactly Once
    /// Wait for the broker to send a Pubrec
    /// The [`Instant`] specifies since when waiting
    AwaitPubrec(Instant),
    /// Wait for the broker to send Pubcomp
    /// The [`Instant`] specifies since when waiting
    AwaitPubcomp(Instant),

    /// The publish is done
    Done
}

const REPUBLISH_DURATION: Duration = Duration::from_secs(5);

impl RequestState {


    fn should_publish(&self, now: Instant) -> bool {
        match self {
            RequestState::Initial => true,
            RequestState::AwaitPuback(instant) | 
                RequestState::AwaitPubrec(instant) | 
                RequestState::AwaitPubcomp(instant) => (now - *instant) > REPUBLISH_DURATION,
            RequestState::Done => false,
        }
    }

    fn is_await_puback(&self) -> bool {
        if let Self::AwaitPuback(_) = self {
            true
        } else {
            false
        }
    }

    fn is_await_pubrec(&self) -> bool {
        if let Self::AwaitPubrec(_) = self {
            true
        } else {
            false
        }
    }

    fn is_await_pubcomp(&self) -> bool {
        if let Self::AwaitPubcomp(_) = self {
            true
        } else {
            false
        }
    }
}

#[cfg(test)]
mod request_state_test {

    use crate::{state::publish::RequestState, time};

    #[test]
    fn test_should_publish() {
        let now = time::now();
        
        assert_eq!(RequestState::Initial.should_publish(now.clone()), true);

        // TODO add more tests
    }

}

struct PublishRequest {
    request: MqttPublish,
    pid: Pid,
    state: RequestState,
    external_id: UniqueID
}

impl PublishRequest {
    fn new(request: MqttPublish, pid: Pid, external_id: UniqueID) -> Self {

        Self {
            request,
            pid,
            state: RequestState::Initial,
            external_id
        }
    }

    fn on_publish_success(&mut self) {
       // TODO How should the state change if republishing a Packet in QoS 2 (ExactlyOnce)?
       //   a. Go back to AwaitPubrec
       //   b. Keep State
       
       match self.state {
            RequestState::Initial => match self.request.qos {
                mqttrs::QoS::AtMostOnce => {
                    self.state = RequestState::Done
                },
                mqttrs::QoS::AtLeastOnce => {
                    self.state = RequestState::AwaitPuback(time::now())
                },
                mqttrs::QoS::ExactlyOnce => {
                    self.state = RequestState::AwaitPubrec(time::now())
                },
            },
            _ => {}
        }
    }
}

pub(crate) struct PublishQueue {
    publishes: QueuedVec<CriticalSectionRawMutex, PublishRequest, MAX_CONCURRENT_PUBLISHES>,
}

impl PublishQueue {

    pub(crate) fn new() -> Self {
        Self {
            publishes: QueuedVec::new(),
        }
    }

    /// Adds a `MqttPublish` to the publish queue
    pub(crate) async fn push_publish(&self, publish: MqttPublish, id: UniqueID, pid: Pid) {
        let request = PublishRequest::new(publish, pid, id);
        self.publishes.push(request).await;
    }

    /// Publish and republish packets
    pub(crate) fn process(&self, send_buffer: &mut impl BufferWriter, control_sender: &impl AsyncSender<MqttEvent>) -> Result<(), MqttError> {
        self.publishes.operate(|publishes|{

            for publish in publishes.iter_mut() {
                // TODO answer quetsion:
                //   Should the loop `break;` if a publish cannot be written to buffer 
                //   beause of insufficient space?
                if publish.state.should_publish(time::now()) {
                    self.publish(publish, send_buffer)?;
                }
            }
            Ok(())
        })?;

        self.cleanup(control_sender);

        Ok(())
    }
    /// Remove done requests and inform the sender 
    fn cleanup(&self, control_sender: &impl AsyncSender<MqttEvent>) {
        self.publishes.retain(|el| {
            if el.state == RequestState::Done {
                match control_sender.try_send(MqttEvent::PublishResult(el.external_id, Ok(()))) {
                    Ok(()) => false,
                    Err(_) => true,
                }
            } else {
                true
            }
        });


    }

    fn publish(&self, publish: &mut PublishRequest, send_buffer: &mut impl BufferWriter) -> Result<bool, MqttError> {
        let dup = publish.state != RequestState::Initial;
        let packet = publish.request.create_publish(publish.pid, dup);
        let packet = Packet::Publish(packet);
        
        let result = encode_slice(&packet, send_buffer);
        match result {
            Ok(n) => {
                send_buffer.commit(n).unwrap();
                publish.on_publish_success(); // Update state
                debug!("packet {} written to send buffer; len = {}", publish.pid, n);
                Ok(true)
            },
            Err(Error::WriteZero) => {
                warn!("send buffer to full to publish: send_buffer_available = {}", send_buffer.len());
                Ok(false)
            },
            Err(e) => {
                error!("error encoding packet: {}", e);
                Err(MqttError::CodecError)
            },
        }
    }

    pub(crate) fn process_puback(&self, puback_pid: &Pid) -> Option<MqttEvent> {
        self.publishes.operate(|publishes|{

            let op = publishes.iter_mut().find(|el| el.pid == *puback_pid);

            let result = if let Some(request) = op {
                if request.state.is_await_puback() {
                    debug!("puback processed for packet {}", request.pid);
                    request.state = RequestState::Done;
                    Some(MqttEvent::PublishResult(request.external_id, Ok(())))
                } else {
                    warn!("illegal state: received puback for packet {} but packet has state {}", request.pid, request.state);
                    None
                }
            } else {
                warn!("received puback for packet {} but packet is unknown", puback_pid);
                None
            };

            publishes.retain(|el| el.state != RequestState::Done);
            result
        })
    }

    fn send_pubrel(&self, request: &mut PublishRequest, send_buffer: &mut impl BufferWriter) -> Result<(), MqttError> {

        let packet = Packet::Pubrel(request.pid.clone());

        match encode_slice(&packet, send_buffer) {
            Ok(n) => {
                send_buffer.commit(n).unwrap();
                request.state = RequestState::AwaitPubcomp(time::now());
                Ok(())
            },
            Err(mqttrs::Error::WriteZero) => {
                warn!("cannot encode pubrel to buffer: not enaugh space");
                Ok(())
            },
            Err(e) => {
                error!("error encoding pubrel packet: {}", e);
                Err(MqttError::CodecError)
            }
        }

    }

    pub(crate) fn process_pubrec(&self, pubrec_pid: &Pid, send_buffer: &mut impl BufferWriter) -> Result<(), MqttError> {
        self.publishes.operate(|publishes|{

            let op = publishes.iter_mut().find(|el| el.pid == *pubrec_pid);

            if let Some(request) = op {
                if request.state.is_await_pubrec() || request.state.is_await_pubcomp() {
                    self.send_pubrel(request, send_buffer)?;
                    debug!("pubrec processed for packet {}", request.pid);
                } else {
                    warn!("illegal state: received pubrec for packet {} but packet has state {}", pubrec_pid, request.state);
                }
            } else {
                warn!("received pubrec for packet {} but packet is unknown", pubrec_pid);
            }

            Ok(())
        })
    }

    pub(crate) fn process_pubcomp(&self, pubcomp_pid: &Pid) -> Option<MqttEvent> {
        self.publishes.operate(|publishes|{

            let op = publishes.iter_mut().find(|el| el.pid == *pubcomp_pid);

            let result = if let Some(request) = op {
                if request.state.is_await_pubcomp() {
                    debug!("pubcomp processed for packet {}", request.pid);
                    request.state = RequestState::Done;
                    Some(MqttEvent::PublishResult(request.external_id, Ok(())))
                } else {
                    warn!("illegal state: received pubcomp for packet {} but packet has state {}", request.pid, request.state);
                    None
                }
            } else {
                warn!("received pubcomp for packet {} but packet is unknown", pubcomp_pid);
                None
            };

            publishes.retain(|el| el.state != RequestState::Done);
            result
        })
    }

}

#[cfg(test)]
mod tests {
    extern crate std;

    use buffer::{new_stack_buffer, Buffer, BufferReader, ReadWrite};
    use embassy_sync::{blocking_mutex::raw::CriticalSectionRawMutex, channel::Channel};
    use mqttrs::{decode_slice_with_len, Packet, Pid, Publish, QoS};

    use crate::{time, MqttEvent, MqttPublish, UniqueID};
    use crate::time::Duration;

    use super::PublishQueue;

    struct Test<const N: usize> {
        send_buffer: Buffer<[u8; N]>,
        control_ch: Channel<CriticalSectionRawMutex, MqttEvent, 16>,
        queue: PublishQueue
    }

    impl <const N: usize> Test<N> {
        fn new() -> Self {
            Self {
                send_buffer: new_stack_buffer(),
                control_ch: Channel::new(),
                queue: PublishQueue::new()
            }
        }

        async fn process(&mut self) {
            let mut writer = self.send_buffer.create_writer();
            self.queue.process(&mut writer, &self.control_ch).unwrap();
        }

        async fn send_publish(&self, topic: &str, payload: &str, qos: QoS, retain: bool) -> UniqueID {
            let mut payload_buffer = new_stack_buffer::<1024>();
            payload_buffer.push(payload.as_bytes()).unwrap();
            
            let publish = MqttPublish::new(
                topic, &[1, 2, 3, 4, 5], qos, retain);

            let id = UniqueID(43234);
            let pid = 34u16.try_into().unwrap();

            self.queue.push_publish(publish, id, pid).await;

            id
        }

        fn read_publish<F: FnOnce(Publish<'_>) -> R, R>(&mut self, assertions: F) -> R {

            let reader = self.send_buffer.create_reader();

            let packet = decode_slice_with_len(&reader).unwrap();
            if let Some((n, packet)) = packet {
                if let Packet::Publish(p) = packet {
                    reader.add_bytes_read(n);
                    assertions(p)
                } else {
                    panic!("expected publish");
                }
            } else {
                panic!("no publish read")
            }
        }

        fn read_pubrel(&mut self) -> Pid {
            let reader = self.send_buffer.create_reader();
            let packet = decode_slice_with_len(&reader).unwrap();
            if let Some((n, packet)) = packet {
                if let Packet::Pubrel(pid) = packet {
                    reader.add_bytes_read(n);
                    pid
                } else {
                    panic!("expected pubrel");
                }
            } else {
                panic!("no pubrel read")
            }
        }
    }

    #[tokio::test]
    async fn test_packet_publish() {
        let mut test = Test::<1024>::new();

        const RETAIN: bool = false;
        const QOS: QoS = QoS::AtMostOnce;

        test.send_publish("hello/world", "hello world", QOS, RETAIN).await;
        test.process().await;

        test.read_publish(|p|{
            assert_eq!(p.dup, false);
            assert_eq!(p.payload, &[1, 2, 3, 4, 5]);
            assert_eq!(p.qospid.qos(), QOS);
            assert_eq!(p.retain, RETAIN);
            assert_eq!(p.topic_name, "hello/world");
        });
    }


    #[tokio::test]
    async fn test_packet_republish() {
        let mut test = Test::<1024>::new();

        // Set time to be mocked
        time::test_time::set_static_now();

        const RETAIN: bool = false;
        const QOS: QoS = QoS::AtLeastOnce;

        // add a publish request & process its
        test.send_publish("hello/world", "hello world", QOS, RETAIN).await;
        test.process().await;

        // Advance the time to break timeout
        time::test_time::advance_time(Duration::from_secs(60));

        // Reset send buffer and process again
        test.send_buffer.reset();
        test.process().await;

        test.read_publish(|p|{
            assert_eq!(p.dup, true);
            assert_eq!(p.payload, &[1, 2, 3, 4, 5]);
            assert_eq!(p.qospid.qos(), QOS);
            assert_eq!(p.retain, RETAIN);
            assert_eq!(p.topic_name, "hello/world");
        });
    }

    #[tokio::test]
    async fn test_packet_publish_success_qos1() {
        let mut test = Test::<1024>::new();

        const RETAIN: bool = false;
        const QOS: QoS = QoS::AtLeastOnce;

        // add a publish request & process its
        let id_sent = test.send_publish("hello/world", "hello world", QOS, RETAIN).await;
        test.process().await;

        let pid = test.read_publish(|p|{
            assert_eq!(p.dup, false);
            assert_eq!(p.qospid.qos(), QOS);
            p.qospid.pid().unwrap()
        });

        // Puback arrived
        let e = test.queue.process_puback(&pid).expect("puback must result in event");

        if let MqttEvent::PublishResult(id, result) = e {
            assert_eq!(id, id_sent);
            assert_eq!(result, Ok(()));

        } else {
            panic!("expected MqttEvent::PublishResult");
        }
    }

    #[tokio::test]
    async fn test_publish_success_qos_2() {

        let mut test = Test::<1024>::new();

        const RETAIN: bool = false;
        const QOS: QoS = QoS::ExactlyOnce;

        // add a publish request & process its
        let uid = test.send_publish("hello/world", "hello world", QOS, RETAIN).await;
        test.process().await;

        let pid = test.read_publish(|p|{
            assert_eq!(p.dup, false);
            assert_eq!(p.qospid.qos(), QOS);
            p.qospid.pid().unwrap()
        });

        test.queue.process_pubrec(&pid, &mut test.send_buffer.create_writer()).unwrap();

        let pid2 = test.read_pubrel();
        assert_eq!(pid, pid2);

        let e = test.queue.process_pubcomp(&pid).unwrap();

        if let MqttEvent::PublishResult(id, result) = e {
            assert_eq!(id, uid);
            assert_eq!(result, Ok(()));
        } else {
            panic!("expected MqttEvent::PublishResult");
        }

    }
}

