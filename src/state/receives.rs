
use buffer::BufferWriter;
use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use crate::{misc::{MqttPacketWriter, WritePacketError}, time::Instant};
use mqttrs::{Packet, Pid, Publish, QosPid};
use queue_vec::QueuedVec;

use crate::{time, MqttError, MqttPublish};

const MAX_CONCURRENT_PUBLISHES: usize = 8;

#[derive(Debug, PartialEq, Clone, Copy)]
#[cfg_attr(feature = "defmt", derive(defmt::Format))]
enum ReceiveState {
    
    /// Initial Puhlish State for all publishes: Nothing is done yet
    Initial,

    // QoS 2 Exactly Once
    /// Wait for the broker to send Pubrel
    /// The [`Instant`] specifies since when waiting
    AwaitPubrel(Instant),

    SendPubcomp,

    /// The publish is received
    Done
}

struct ReceivedPublish {
    qospid: QosPid,
    state: ReceiveState,
}

impl ReceivedPublish {
    fn new(qospid: QosPid) -> Self {

        Self {
            qospid,
            state: ReceiveState::Initial,
        }
    }

    fn send_and_update(&mut self, send_buffer: &mut impl BufferWriter) {

        match self.state {
            ReceiveState::Initial => self.send_initial_state(send_buffer),
            
            //TODO resend pubrec
            ReceiveState::AwaitPubrel(_instant) => {},

            ReceiveState::SendPubcomp => self.send_pubcomp(self.qospid.pid().expect("When state is SendPubcomp there must be a pid"), send_buffer),
            ReceiveState::Done => {},
        }
    }

    fn send_pubcomp(&mut self, pid: Pid, send_buffer: &mut impl BufferWriter) {
        let result = send_buffer.write_packet(&Packet::Pubcomp(pid));
        match result {
            Ok(()) => {
                self.state = ReceiveState::Done;
            },
            Err(WritePacketError::NotEnaughSpace) => {
                debug!("not enaugh space to write pubcomp to send buffer for {}", pid);
            },
            Err(e) => {
                error!("could not send pubcomp to {}: {}", pid, e);
            }
        }
    }

    fn send_initial_state(&mut self, send_buffer: &mut impl BufferWriter) {
        match self.qospid {
            QosPid::AtMostOnce => {},
            QosPid::AtLeastOnce(pid) => self.send_puback(pid, send_buffer),
            QosPid::ExactlyOnce(pid) => self.send_pubrec(pid, send_buffer),
        }
    }

    fn send_puback(&mut self, pid: Pid, send_buffer: &mut impl BufferWriter) {
        let result = send_buffer.write_packet(&Packet::Puback(pid));
        match result {
            Ok(()) => {
                self.state = ReceiveState::Done;
            },
            Err(WritePacketError::NotEnaughSpace) => {
                debug!("not enaugh space to write puback to send buffer for {}", pid);
            },
            Err(e) => {
                error!("could not send puback to {}: {}", pid, e);
            }
        }
    }

    fn send_pubrec(&mut self, pid: Pid, send_buffer: &mut impl BufferWriter) {
        let result = send_buffer.write_packet(&Packet::Pubrec(pid));
        match result {
            Ok(()) => {
                self.state = ReceiveState::AwaitPubrel(time::now());
            },
            Err(WritePacketError::NotEnaughSpace) => {
                debug!("not enaugh space to write pubrec to send buffer for {}", pid);
            },
            Err(e) => {
                error!("could not send pubrec to {}: {}", pid, e);
            }
        }
    }
}

impl <'a> From<&Publish<'a>> for ReceivedPublish {
    fn from(value: &Publish<'a>) -> Self {
        Self::new(value.qospid)
    }
}

pub(crate) struct ReceivedPublishQueue {
    publishes: QueuedVec<CriticalSectionRawMutex, ReceivedPublish, MAX_CONCURRENT_PUBLISHES>,
}

impl ReceivedPublishQueue {

    pub(crate) fn new() -> Self {
        Self {
            publishes: QueuedVec::new(),
        }
    }

    /// Publish and republish packets
    pub(crate) fn process(&self, send_buffer: &mut impl BufferWriter) -> Result<(), MqttError> {
        self.publishes.operate(|publishes|{

            for publish in publishes.iter_mut() {
                publish.send_and_update(send_buffer);
            }
            Ok(())
        })?;

        self.publishes.retain(|el| el.state != ReceiveState::Done );

        Ok(())
    }

    fn is_known_pid(&self, pid: Pid) -> bool {
        self.publishes.operate(|publishes|{
            publishes.iter()
                .find(|el| el.qospid.pid() == Some(pid))
                .is_some()
        })
    }

    pub(crate) fn process_pubrel(&self, pid: Pid) {
        self.publishes.operate(|publishes|{
            let publish = publishes.iter_mut()
                .find(|el| el.qospid.pid() == Some(pid));

            if let Some(publish) = publish {
                debug!("received pubrel for {}", pid);
                publish.state = ReceiveState::SendPubcomp;
            }
        })
    }

    pub(crate) async fn process_publish(&self, publish: &Publish<'_>) -> Option<MqttPublish>{
        let p = match MqttPublish::try_from(publish) {
            Ok(p) => p,
            Err(e) => {
                error!("could not transform &Publish<'_> to MqttPublish: {}", e);
                return None;
            },
        };

        match publish.qospid {
            QosPid::AtMostOnce => Some(p),
            QosPid::AtLeastOnce(pid) | QosPid::ExactlyOnce(pid) => {
                if publish.dup && ! self.is_known_pid(pid) {
                    None
                } else {
                    self.publishes.push(ReceivedPublish::new(publish.qospid)).await;
                    Some(p)
                }
            }
        }
    }

    /*
    pub(crate) fn process_puback(&self, puback_pid: &Pid) -> Option<MqttEvent> {
        self.publishes.operate(|publishes|{

            let op = publishes.iter_mut().find(|el| el.pid == *puback_pid);

            let result = if let Some(request) = op {
                if request.state.is_await_puback() {
                    debug!("puback processed for packet {}", request.pid);
                    request.state = ReceiveState::Done;
                    Some(MqttEvent::PublishResult(request.external_id, Ok(())))
                } else {
                    warn!("illegal state: received puback for packet {} but packet has state {}", request.pid, request.state);
                    None
                }
            } else {
                warn!("received puback for packet {} but packet is unknown", puback_pid);
                None
            };

            publishes.retain(|el| el.state != ReceiveState::Done);
            result
        })
    }

    fn send_pubrel(&self, request: &mut ReceivedPublish, send_buffer: &mut impl BufferWriter) -> Result<(), MqttError> {

        let packet = Packet::Pubrel(request.pid.clone());

        match encode_slice(&packet, send_buffer) {
            Ok(n) => {
                send_buffer.commit(n).unwrap();
                request.state = ReceiveState::AwaitPubcomp(time::now());
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
                    request.state = ReceiveState::Done;
                    Some(MqttEvent::PublishResult(request.external_id, Ok(())))
                } else {
                    warn!("illegal state: received pubcomp for packet {} but packet has state {}", request.pid, request.state);
                    None
                }
            } else {
                warn!("received pubcomp for packet {} but packet is unknown", pubcomp_pid);
                None
            };

            publishes.retain(|el| el.state != ReceiveState::Done);
            result
        })
    }

    */

}
