use embassy_sync::pubsub::{DynPublisher, DynSubscriber};
use embassy_sync::{blocking_mutex::raw::RawMutex, pubsub::PubSubChannel};
use mqttrs2::{Packet, Pid, Publish, QosPid};

use crate::state::connection::ConnectionState;
use crate::state::SendResult;
use crate::{mutex::Mutex, MqttError};

use heapless::{Vec, String };

#[derive(Clone)]
pub struct ReceivedPublish<const BUFFER: usize, const TOPIC_SIZE: usize> {
    pub dup: bool,
    pub qospid: QosPid,
    pub retain: bool,
    pub topic_name: String<TOPIC_SIZE>,
    pub payload: Vec<u8, BUFFER>
}

impl <'a, const BUFFER: usize, const TOPIC_SIZE: usize> TryFrom<&Publish<'a>> for ReceivedPublish<BUFFER, TOPIC_SIZE> {
    type Error = MqttError;

    fn try_from(value: &Publish<'a>) -> Result<Self, Self::Error> {
        Ok(Self {
            dup: value.dup,
            qospid: value.qospid,
            retain: value.retain,
            topic_name: String::try_from(value.topic_name)
                .map_err(|_| MqttError::TopicSizeError)?, 
            payload: Vec::try_from(value.payload)
                .map_err(|_| MqttError::BufferTooSmall)?, 
        })
    }
}

struct ReceivesInner <const PARALLEL_RECEIVES: usize> {
    send_puback: Vec<Pid, PARALLEL_RECEIVES>,

    send_pubrec: Vec<Pid, PARALLEL_RECEIVES>,
    await_pubrel: Vec<Pid, PARALLEL_RECEIVES>,
    send_pubcomp: Vec<Pid, PARALLEL_RECEIVES>,

}

impl<const PARALLEL_RECEIVES: usize> ReceivesInner<PARALLEL_RECEIVES> {

    fn new() -> Self {
        Self {
            send_puback: Vec::new(),
            send_pubrec: Vec::new(),
            await_pubrel: Vec::new(),
            send_pubcomp: Vec::new()
        }
    }

    /// for QoS 2: searches for the pid in the queues and returns true if there
    /// is an entry with this pid
    fn is_dup(&self, pid: Pid) -> bool {
        self.await_pubrel.iter().any(|await_pubrel| *await_pubrel == pid) ||
        self.send_pubrec.iter().any(|send_pubrec| *send_pubrec == pid) ||
        self.send_pubcomp.iter().any(|send_pubcomp| *send_pubcomp == pid)
    }

    pub async fn on_qos_2_publish<const B: usize, const T: usize>(&mut self, publish: &Publish<'_>, publisher: DynPublisher<'_, ReceivedPublish<B, T>>) -> Result<(), MqttError> {
        let pid = publish.qospid.pid().expect("qos 2 always has a pid");

        // check dup before adding to queue
        let is_dup = self.is_dup(pid);

        // Always respond with a publish with qo2 with a pubrec
        self.send_pubrec.push(pid)
            .map_err(|pid| MqttError::QueueFull(QosPid::AtLeastOnce(pid)))?;

        if ! is_dup {
            publisher.publish(publish.try_into()?).await;
        } else {
            debug!("received dup qos 2 publish {}", pid);
        }

        Ok(())
    }

    pub fn on_pubrel(&mut self, pid: Pid) -> Result<(), MqttError>{
        // always send a pubcomp
        self.send_pubcomp.push(pid)
            .map_err(|pid| MqttError::QueueFull(QosPid::AtLeastOnce(pid)))?;

        if let Some(index) = self.await_pubrel.iter().position(|entry| *entry == pid) {
            self.await_pubrel.remove(index);
        }

        Ok(())
    }

    pub fn send_packets(&mut self, connection: &impl ConnectionState) -> Result<SendResult, MqttError> {
        let mut i = 0;
        while i < self.send_puback.len() {
            let pid = &self.send_puback[i];
            let sent = connection.try_write_packet(&Packet::Puback(*pid))?;
            if sent {
                self.send_puback.remove(i);
            } else {
                return Ok(SendResult::PartiallySent);
            }
            
            i += 1;
        }

        let mut i = 0;
        while i < self.send_pubrec.len() {
            if self.await_pubrel.is_full() {
                break;
            }

            let pid = &self.send_pubrec[i];
            let sent = connection.try_write_packet(&Packet::Pubrec(*pid))?;
            if sent {
                let pid = self.send_pubrec.remove(i);
                let _ = self.await_pubrel.push(pid);
            } else {
                return Ok(SendResult::PartiallySent);
            }
            
            i += 1;
        }

        let mut i = 0;
        while i < self.send_pubcomp.len() {
            let pid = &self.send_pubcomp[i];
            let sent = connection.try_write_packet(&Packet::Pubcomp(*pid))?;
            if sent {
                self.send_pubcomp.remove(i);
            } else {
                return Ok(SendResult::PartiallySent);
            }
            i += 1;
        }

        Ok(SendResult::SentAll)
    }
}

pub struct Receives <M, const BUFFER: usize, const TOPIC_SIZE: usize, const PARALLEL_RECEIVES: usize> where M: RawMutex{

    queue: PubSubChannel<M, ReceivedPublish<BUFFER, TOPIC_SIZE>, 1, 8, 1>,
    inner: Mutex<M, ReceivesInner<PARALLEL_RECEIVES>, 4>

}

impl<M, const BUFFER: usize, const TOPIC_SIZE: usize, const PARALLEL_RECEIVES: usize> Receives<M, BUFFER, TOPIC_SIZE, PARALLEL_RECEIVES>
where M: RawMutex
{
    pub fn new() -> Self {
        Self {
            queue: PubSubChannel::new(),
            inner: Mutex::new(ReceivesInner::new())
        }
    }

    pub async fn send_packets(&self, connection: &impl ConnectionState) -> Result<SendResult, MqttError> {
        self.inner.lock().await.send_packets(connection)
    }

    pub async fn on_publish(&self, publish: &Publish<'_>) -> Result<(), MqttError> {
        let publisher = self.queue.dyn_publisher().unwrap();

        match publish.qospid {
            QosPid::AtMostOnce => {
                publisher.publish(publish.try_into()?).await;
            },
            QosPid::AtLeastOnce(pid) => {
                publisher.publish(publish.try_into()?).await;
                let mut inner = self.inner.lock().await;
                inner.send_puback.push(pid)
                    .map_err(|pid| MqttError::QueueFull(QosPid::AtLeastOnce(pid)))?;
            },
            QosPid::ExactlyOnce(_) => {
                let mut inner = self.inner.lock().await;
                inner.on_qos_2_publish(&publish, publisher).await?;
            },
        }

        Ok(())
    }

    pub fn subscribe_publishes(&self) -> Result<DynSubscriber<'_, ReceivedPublish<BUFFER, TOPIC_SIZE>>, MqttError> {
        self.queue.dyn_subscriber()
            .map_err(|err| err.into())
    }

    pub async fn on_pubrel(&self, pid: Pid) -> Result<(), MqttError> {
        self.inner.lock().await.on_pubrel(pid)
    }
}


#[cfg(test)]
mod tests {


    use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
    use mqttrs2::{Packet, Publish, QosPid};

    use crate::{state::{SendResult, connection::test::DummyConnectionState, receives2::Receives}, testutils::*};

    #[test]
    fn test_qos_0_receive() {

        let receives = Receives::<CriticalSectionRawMutex, 1024, 128, 4>::new();
        let mut subscription = receives.subscribe_publishes().unwrap();

        let publish = Publish{
            dup: false,
            qospid: QosPid::AtMostOnce,
            retain: false,
            topic_name: "test/topic",
            payload: "test-payload".as_bytes()
        };

        let on_publish_fut = receives.on_publish(&publish);
        assert_ready_pin(on_publish_fut).unwrap();

        let received_publish = subscription.try_next_message_pure()
            .expect("expect there to be a publish");

        assert_eq!(received_publish.dup, publish.dup);
        assert_eq!(received_publish.qospid, publish.qospid);
        assert_eq!(received_publish.payload, publish.payload);
        assert_eq!(received_publish.retain, publish.retain);
        assert_eq!(received_publish.topic_name, publish.topic_name);


        let inner = receives.inner.try_lock().unwrap();
        assert!(inner.await_pubrel.is_empty());
        assert!(inner.send_puback.is_empty());
        assert!(inner.send_pubcomp.is_empty());
        assert!(inner.send_pubrec.is_empty());
    }

    #[test]
    fn test_qos_1_receive() {
        let connection_state = DummyConnectionState::new();
        let receives = Receives::<CriticalSectionRawMutex, 1024, 128, 4>::new();
        let mut subscription = receives.subscribe_publishes().unwrap();

        let pid = 8.try_into().unwrap();

        let publish = Publish{
            dup: false,
            qospid: QosPid::AtLeastOnce(pid),
            retain: false,
            topic_name: "test/topic",
            payload: "test-payload".as_bytes()
        };

        let on_publish_fut = receives.on_publish(&publish);
        assert_ready_pin(on_publish_fut).unwrap();

        // expect an entry in send_puback
        let inner = receives.inner.try_lock().unwrap();
        assert_eq!(inner.send_puback.len(), 1);
        drop(inner);

        // expect publishin queue
        let received_publish = subscription.try_next_message_pure()
            .expect("expect there to be a publish");

        assert_eq!(received_publish.dup, publish.dup);
        assert_eq!(received_publish.qospid, publish.qospid);
        assert_eq!(received_publish.payload, publish.payload);
        assert_eq!(received_publish.retain, publish.retain);
        assert_eq!(received_publish.topic_name, publish.topic_name);

        // Send puback
        let send_fut = receives.send_packets(&connection_state);
        let send_result = assert_ready_pin(send_fut).unwrap();
        assert_eq!(send_result, SendResult::SentAll);

        connection_state.assert_packet_written(|packet|match packet {
            Packet::Puback(puback_pid) if puback_pid == pid => {},
            p => panic!("unexpected packet while expecting puback: {:?}", p)
        });

        let inner = receives.inner.try_lock().unwrap();
        assert!(inner.await_pubrel.is_empty());
        assert!(inner.send_puback.is_empty());
        assert!(inner.send_pubcomp.is_empty());
        assert!(inner.send_pubrec.is_empty());
    }

    #[test]
    fn test_qos_2_receive() {
        let connection_state = DummyConnectionState::new();
        let receives = Receives::<CriticalSectionRawMutex, 1024, 128, 4>::new();
        let mut subscription = receives.subscribe_publishes().unwrap();

        let pid = 8.try_into().unwrap();

        let publish = Publish{
            dup: false,
            qospid: QosPid::ExactlyOnce(pid),
            retain: false,
            topic_name: "test/topic",
            payload: "test-payload".as_bytes()
        };

        let on_publish_fut = receives.on_publish(&publish);
        assert_ready_pin(on_publish_fut).unwrap();

        // expect an entry in send_pubrec
        let inner = receives.inner.try_lock().unwrap();
        assert_eq!(inner.send_pubrec.len(), 1);
        drop(inner);

        // expect publishin queue
        let received_publish = subscription.try_next_message_pure()
            .expect("expect there to be a publish");

        assert_eq!(received_publish.dup, publish.dup);
        assert_eq!(received_publish.qospid, publish.qospid);
        assert_eq!(received_publish.payload, publish.payload);
        assert_eq!(received_publish.retain, publish.retain);
        assert_eq!(received_publish.topic_name, publish.topic_name);

        // Send pubrec
        let send_fut = receives.send_packets(&connection_state);
        let send_result = assert_ready_pin(send_fut).unwrap();
        assert_eq!(send_result, SendResult::SentAll);

        connection_state.assert_packet_written(|packet|match packet {
            Packet::Pubrec(pubrec_pid) if pubrec_pid == pid => {},
            p => panic!("unexpected packet while expecting puback: {:?}", p)
        });

        // expect an entry in await_pubrel
        let inner = receives.inner.try_lock().unwrap();
        assert_eq!(inner.await_pubrel.len(), 1);
        drop(inner);

        // receive pubrel
        let on_pubrel_fut = receives.on_pubrel(pid);
        assert_ready_pin(on_pubrel_fut).unwrap();

        // expect an entry in send_pubcomp
        let inner = receives.inner.try_lock().unwrap();
        assert_eq!(inner.send_pubcomp.len(), 1);
        drop(inner);

        // Send pubcomp
        let send_fut = receives.send_packets(&connection_state);
        let send_result = assert_ready_pin(send_fut).unwrap();
        assert_eq!(send_result, SendResult::SentAll);

        connection_state.assert_packet_written(|packet|match packet {
            Packet::Pubcomp(pubcomp_pid) if pubcomp_pid == pid => {},
            p => panic!("unexpected packet while expecting puback: {:?}", p)
        });

        // assert all queues empty
        let inner = receives.inner.try_lock().unwrap();
        assert!(inner.await_pubrel.is_empty());
        assert!(inner.send_puback.is_empty());
        assert!(inner.send_pubcomp.is_empty());
        assert!(inner.send_pubrec.is_empty());
    }

    #[test]
    fn test_no_dup_in_qos_2() {
        let receives = Receives::<CriticalSectionRawMutex, 1024, 128, 4>::new();
        let mut subscription = receives.subscribe_publishes().unwrap();

        let pid = 8.try_into().unwrap();

        let publish = Publish{
            dup: false,
            qospid: QosPid::ExactlyOnce(pid),
            retain: false,
            topic_name: "test/topic",
            payload: "test-payload".as_bytes()
        };

        let on_publish_fut = receives.on_publish(&publish);
        assert_ready_pin(on_publish_fut).unwrap();

        // expect an entry in send_pubrec
        let inner = receives.inner.try_lock().unwrap();
        assert_eq!(inner.send_pubrec.len(), 1);
        drop(inner);

        // expect publishin queue
        let received_publish = subscription.try_next_message_pure()
            .expect("expect there to be a publish");

        assert_eq!(received_publish.dup, publish.dup);
        assert_eq!(received_publish.qospid, publish.qospid);
        assert_eq!(received_publish.payload, publish.payload);
        assert_eq!(received_publish.retain, publish.retain);
        assert_eq!(received_publish.topic_name, publish.topic_name);

        // receive a up publish
        let dup_publish = Publish{
            dup: true,
            qospid: QosPid::ExactlyOnce(pid),
            retain: false,
            topic_name: "test/topic",
            payload: "test-payload".as_bytes()
        };

        // No dup notification
        let on_publish_fut = receives.on_publish(&dup_publish);
        assert_ready_pin(on_publish_fut).unwrap();
        let received_publish = subscription.try_next_message_pure();
        assert!(received_publish.is_none());

    }

    #[test]
    /// Requirement test for [4.3.3 QoS 2: Exactly once delivery](http://docs.oasis-open.org/mqtt/mqtt/v3.1.1/os/mqtt-v3.1.1-os.html#_Toc398718102)
    /// Requirement: MUST respond with a PUBREC containing the Packet Identifier from the incoming PUBLISH Packet, 
    /// having accepted ownership of the Application Message. Until it has received the corresponding PUBREL packet, 
    /// the Receiver MUST acknowledge any subsequent PUBLISH packet with the same Packet Identifier by sending a PUBREC. 
    /// It MUST NOT cause duplicate messages to be delivered to any onward recipients in this case.
    fn test_send_pubrec_publish() {
        let connection_state = DummyConnectionState::new();
        let receives = Receives::<CriticalSectionRawMutex, 1024, 128, 4>::new();
        let mut subscription = receives.subscribe_publishes().unwrap();

        let pid = 8.try_into().unwrap();

        let publish = Publish{
            dup: false,
            qospid: QosPid::ExactlyOnce(pid),
            retain: false,
            topic_name: "test/topic",
            payload: "test-payload".as_bytes()
        };

        // first publish
        let on_publish_fut = receives.on_publish(&publish);
        assert_ready_pin(on_publish_fut).unwrap();
        assert!(subscription.try_next_message_pure().is_some());

        // Send pubrec
        let send_fut = receives.send_packets(&connection_state);
        let send_result = assert_ready_pin(send_fut).unwrap();
        assert_eq!(send_result, SendResult::SentAll);

        connection_state.assert_packet_written(|packet|match packet {
            Packet::Pubrec(pubrec_pid) if pubrec_pid == pid => {},
            p => panic!("unexpected packet while expecting puback: {:?}", p)
        });

        // second publish
        let on_publish_fut = receives.on_publish(&publish);
        assert_ready_pin(on_publish_fut).unwrap();
        assert!(subscription.try_next_message_pure().is_none());

        // Send pubrec again
        let send_fut = receives.send_packets(&connection_state);
        let send_result = assert_ready_pin(send_fut).unwrap();
        assert_eq!(send_result, SendResult::SentAll);

        connection_state.assert_packet_written(|packet|match packet {
            Packet::Pubrec(pubrec_pid) if pubrec_pid == pid => {},
            p => panic!("unexpected packet while expecting puback: {:?}", p)
        });

    }

    #[test]
    /// Requirement test for [4.3.3 QoS 2: Exactly once delivery](http://docs.oasis-open.org/mqtt/mqtt/v3.1.1/os/mqtt-v3.1.1-os.html#_Toc398718102)
    /// Requirement: MUST respond to a PUBREL packet by sending a PUBCOMP packet containing the same Packet Identifier as the PUBREL.
    fn test_send_pubcomp_to_pubrel() {
        let connection_state = DummyConnectionState::new();
        let receives = Receives::<CriticalSectionRawMutex, 1024, 128, 4>::new();
        let mut subscription = receives.subscribe_publishes().unwrap();

        let pid = 8.try_into().unwrap();
        
        // Receive pubrel
        let on_pubrel_fut = receives.on_pubrel(pid);
        assert_ready_pin(on_pubrel_fut).unwrap();
        assert!(subscription.try_next_message_pure().is_none());

        // Send pubcomp
        let send_fut = receives.send_packets(&connection_state);
        let send_result = assert_ready_pin(send_fut).unwrap();
        assert_eq!(send_result, SendResult::SentAll);

        connection_state.assert_packet_written(|packet|match packet {
            Packet::Pubcomp(pubcomp_pid) if pubcomp_pid == pid => {},
            p => panic!("unexpected packet while expecting puback: {:?}", p)
        });

        // Receive pubrel again
        let on_pubrel_fut = receives.on_pubrel(pid);
        assert_ready_pin(on_pubrel_fut).unwrap();
        assert!(subscription.try_next_message_pure().is_none());

        // Send pubcomp again
        let send_fut = receives.send_packets(&connection_state);
        let send_result = assert_ready_pin(send_fut).unwrap();
        assert_eq!(send_result, SendResult::SentAll);

        connection_state.assert_packet_written(|packet|match packet {
            Packet::Pubcomp(pubcomp_pid) if pubcomp_pid == pid => {},
            p => panic!("unexpected packet while expecting puback: {:?}", p)
        });
    }

}