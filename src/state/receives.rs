
use buffer::BufferWriter;
use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use network::mqtt::{MqttPacketError, WriteMqttPacketMut};
use crate::time::Instant;
use mqttrs::{Packet, Pid, Publish, QosPid};
use queue_vec::QueuedVec;

use crate::{time, MqttError, MqttPublish};

const MAX_CONCURRENT_PUBLISHES: usize = 8;

#[derive(Debug, PartialEq, Clone, Copy)]
#[cfg_attr(feature = "defmt", derive(defmt::Format))]
enum ReceiveState {
    
    /// Initial Puhlish State for all publishes: Nothing is done yet
    Initial,

    SendPubrec,

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

            ReceiveState::SendPubrec => {
                let pid = self.qospid.pid();
                if let Some(pid) = pid {
                    trace!("resend pubrec for {}", pid);
                    self.send_pubrec(pid, send_buffer);
                } else {
                    error!("illegal state: SendPubrec but QoS is {}", self.qospid.qos());
                }
            }
            
            //TODO resend pubrec
            ReceiveState::AwaitPubrel(_instant) => {},

            ReceiveState::SendPubcomp => self.send_pubcomp(self.qospid.pid().expect("When state is SendPubcomp there must be a pid"), send_buffer),
            ReceiveState::Done => {},
        }
    }

    fn send_pubcomp(&mut self, pid: Pid, send_buffer: &mut impl BufferWriter) {
        let result = send_buffer.write_mqtt_packet_sync(&Packet::Pubcomp(pid));
        match result {
            Ok(()) => {
                self.state = ReceiveState::Done;
            },
            Err(MqttPacketError::NotEnaughBufferSpace) => {
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
        let result = send_buffer.write_mqtt_packet_sync(&Packet::Puback(pid));
        match result {
            Ok(()) => {
                self.state = ReceiveState::Done;
            },
            Err(MqttPacketError::NotEnaughBufferSpace) => {
                debug!("not enaugh space to write puback to send buffer for {}", pid);
            },
            Err(e) => {
                error!("could not send puback to {}: {}", pid, e);
            }
        }
    }

    fn send_pubrec(&mut self, pid: Pid, send_buffer: &mut impl BufferWriter) {
        let result = send_buffer.write_mqtt_packet_sync(&Packet::Pubrec(pid));
        match result {
            Ok(()) => {
                self.state = ReceiveState::AwaitPubrel(time::now());
            },
            Err(MqttPacketError::NotEnaughBufferSpace) => {
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

    /**
     * Processes a received pubrel
     */
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

    /**
     * Process a received publish
     */
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
                if publish.dup && self.check_duplicate_publish(pid, &publish.topic_name) {
                    None
                } else {
                    self.publishes.push(ReceivedPublish::new(publish.qospid)).await;
                    Some(p)
                }
            }
        }
    }

    /**
     * Checks if the provided pid is a duplicate
     * if so sets the state to [`ReceiveState::SendPubrec`]
     * otherwise does nothing
     */
    fn check_duplicate_publish(&self, pid: Pid, topic: &str) -> bool {
        self.publishes.operate(|publishes|{
            for p in publishes {
                if p.qospid.pid() == Some(pid) {
                    p.state = ReceiveState::SendPubrec;
                    debug!("received publish dup: pid = {}, topic = {}", pid, topic);
                    return true
                }
            }

            trace!("received new publish: pid = {}, topic = {}", pid, topic);
            false
        })
    }

}


#[cfg(test)]
mod tests {
    use buffer::{new_stack_buffer, ReadWrite};
    use mqttrs::{Packet, Pid, Publish, QosPid};
    use network::mqtt::ReadMqttPacket;

    use super::ReceivedPublishQueue;

    #[tokio::test]
    #[ntest::timeout(1000)]
    async fn test_receive_qos_0() {
        let queue = ReceivedPublishQueue::new();
        let mut send_buffer = new_stack_buffer::<1024>();

        let publish = Publish{
            dup: false,
            qospid: QosPid::AtMostOnce,
            retain: false,
            payload: "test".as_bytes(),
            topic_name: "test-topic"
        };

        let event = queue.process_publish(&publish).await;
        assert!(event.is_some());

        queue.process(&mut send_buffer.create_writer()).unwrap();

        assert!(! send_buffer.has_remaining_len());
    }

    #[tokio::test]
    #[ntest::timeout(1000)]
    async fn test_receive_qos_1() {
        let queue = ReceivedPublishQueue::new();
        let mut send_buffer = new_stack_buffer::<1024>();

        let publish = Publish{
            dup: false,
            qospid: QosPid::AtLeastOnce(Pid::try_from(34).unwrap()),
            retain: false,
            payload: "test".as_bytes(),
            topic_name: "test-topic"
        };

        let event = queue.process_publish(&publish).await;
        assert!(event.is_some());

        queue.process(&mut send_buffer.create_writer()).unwrap();

        let reader = send_buffer.create_reader();
        let p = reader.read_packet()
            .unwrap()
            .expect("expected to read puback");
        
        if let Packet::Puback(pid) = p {
            assert_eq!(u16::from(pid), 34);
        } else {
            panic!("expected puback");
        }
    }

    #[tokio::test]
    #[ntest::timeout(1000)]
    async fn test_receive_qos_2() {
        let queue = ReceivedPublishQueue::new();
        let mut send_buffer = new_stack_buffer::<1024>();

        let pid = Pid::try_from(34).unwrap();

        let publish = Publish{
            dup: false,
            qospid: QosPid::ExactlyOnce(pid),
            retain: false,
            payload: "test".as_bytes(),
            topic_name: "test-topic"
        };

        let event = queue.process_publish(&publish).await;
        assert!(event.is_some());

        queue.process(&mut send_buffer.create_writer()).unwrap();

        let reader = send_buffer.create_reader();
        let p = reader.read_packet()
            .unwrap()
            .expect("expected to read pubrec");
        
        if let Packet::Pubrec(received_pid) = p {
            assert_eq!(received_pid, pid);
        } else {
            panic!("expected pubrec");
        }
        drop(reader);

        queue.process_pubrel(pid);
        queue.process(&mut send_buffer.create_writer()).unwrap();

        let reader = send_buffer.create_reader();
        let p = reader.read_packet()
            .unwrap()
            .expect("expected to read pubcomp");
        
        if let Packet::Pubcomp(received_pid) = p {
            assert_eq!(received_pid, pid);
        } else {
            panic!("expected pubcomp");
        }
    }

    #[tokio::test]
    #[ntest::timeout(1000)]
    async fn test_receive_qos_2_dup() {
        let queue = ReceivedPublishQueue::new();
        let mut send_buffer = new_stack_buffer::<1024>();

        let pid = Pid::try_from(34).unwrap();

        let mut publish = Publish{
            dup: false,
            qospid: QosPid::ExactlyOnce(pid),
            retain: false,
            payload: "test".as_bytes(),
            topic_name: "test-topic"
        };

        // Send first publish
        let event = queue.process_publish(&publish).await;
        assert!(event.is_some());

        queue.process(&mut send_buffer.create_writer()).unwrap();

        let reader = send_buffer.create_reader();
        let p = reader.read_packet()
            .unwrap()
            .expect("expected to read pubrec");
        
        if let Packet::Pubrec(received_pid) = p {
            assert_eq!(received_pid, pid);
        } else {
            panic!("expected pubrec");
        }
        drop(reader);

        // send second, duplicate publish
        publish.dup = true;
        let event = queue.process_publish(&publish).await;
        assert!(event.is_none());
        queue.process(&mut send_buffer.create_writer()).unwrap();

        let reader = send_buffer.create_reader();
        let p = reader.read_packet()
            .unwrap()
            .expect("expected to read pubrec");
        
        if let Packet::Pubrec(received_pid) = p {
            assert_eq!(received_pid, pid);
        } else {
            panic!("expected pubrec");
        }
    }

}