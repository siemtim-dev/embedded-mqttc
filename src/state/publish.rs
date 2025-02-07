use core::cell::RefCell;

use buffer::BufferWriter;
use embassy_sync::blocking_mutex::{raw::CriticalSectionRawMutex, Mutex};
use embassy_time::{Duration, Instant};
use mqttrs::{encode_slice, Error, Packet, Pid};
use queue_vec::QueuedVec;

use crate::{io::AsyncSender, MqttError, MqttEvent, MqttPublish, UniqueID};

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
    use embassy_time::Instant;

    use crate::state::publish::RequestState;

    #[test]
    fn test_should_publish() {
        let now = Instant::from_secs(2000);
        
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
                    self.state = RequestState::AwaitPuback(Instant::now())
                },
                mqttrs::QoS::ExactlyOnce => {
                    self.state = RequestState::AwaitPubrec(Instant::now())
                },
            },
            _ => {}
        }
    }
}

pub(crate) struct PublishQueue {
    publishes: QueuedVec<CriticalSectionRawMutex, PublishRequest, MAX_CONCURRENT_PUBLISHES>,
    pid_source: Mutex<CriticalSectionRawMutex, RefCell<Pid>>
}

impl PublishQueue {

    pub(crate) fn new() -> Self {
        Self {
            publishes: QueuedVec::new(),

            pid_source: Mutex::new(RefCell::new(Pid::default()))
        }
    }

    /// Genrates the next unique pid for the packet
    fn next_pid(&self) -> Pid {
        self.pid_source.lock(|pid|{
            let mut pid = pid.borrow_mut();

            let result = pid.clone();
            *pid = result + 1;
            result
        })
    }

    /// Adds a `MqttPublosh to the publish queue` 
    pub(crate) async fn push_publish(&self, publish: MqttPublish, id: UniqueID) {
        let request = PublishRequest::new(publish, self.next_pid(), id);
        self.publishes.push(request).await;
    }

    /// Publish and republish packets
    pub(crate) async fn process(&self, send_buffer: &mut impl BufferWriter, control_sender: &impl AsyncSender<MqttEvent>) -> Result<(), MqttError> {
        self.publishes.operate(|publishes|{

            for publish in publishes.iter_mut() {
                // TODO answer quetsion:
                //   Should the loop `break;` if a publish cannot be written to buffer 
                //   beause of insufficient space?
                if publish.state.should_publish(Instant::now()) {
                    self.publish(publish, send_buffer)?;
                }
            }
            Ok(())
        })?;

        self.cleanup(control_sender).await;

        Ok(())
    }
    /// Remove done requests and inform the sender 
    async fn cleanup(&self, control_sender: &impl AsyncSender<MqttEvent>) {
        for req in self.publishes.remove(|el| el.state == RequestState::Done) {
            // TODO maybe this is a deadlock by the client
            // The Clients waits until thereis space for publish
            // And this call waits until he client consumes the events
            control_sender.send(MqttEvent::PublishResult(req.external_id, Ok(()))).await;
        }
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
                request.state = RequestState::AwaitPubcomp(Instant::now());
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


