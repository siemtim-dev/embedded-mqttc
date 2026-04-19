use embassy_sync::{blocking_mutex::raw::RawMutex, pubsub::DynPublisher};
use heapless::{String, Vec};
use mqttrs2::{Packet, Pid, Publish, QoS, QosPid};

use crate::{MqttError, MqttEvent, UniqueID, mutex::Mutex, state::{SendResult, connection::ConnectionState, pid::free_pid}, time::{self, Duration, Instant}};

pub const RESEND_DURATION: Duration = Duration::from_secs(5);

pub struct OwnedPublish<const BUFFER: usize, const TOPIC_SIZE: usize> {
    pub dup: bool,
    pub qospid: QosPid,
    pub retain: bool,
    pub topic_name: String<TOPIC_SIZE>,
    pub payload: Vec<u8, BUFFER>,
    unique_id: UniqueID
}

impl<const BUFFER: usize, const TOPIC_SIZE: usize> OwnedPublish<BUFFER, TOPIC_SIZE> {

    pub fn borrow<'a>(&'a self) -> Packet<'a> {
        Packet::Publish(Publish{
            dup: self.dup,
            qospid: self.qospid,
            retain: self.retain,
            topic_name: &self.topic_name,
            payload: &self.payload
        })
    }

    pub fn new_from_publish(publish: &Publish<'_>, unique_id: UniqueID) -> Result<Self, MqttError> {
        Ok(Self {
            dup: publish.dup,
            qospid: publish.qospid,
            retain: publish.retain,
            topic_name: String::try_from(publish.topic_name)
                .map_err(|_| MqttError::TopicSizeError)?,
            payload: Vec::try_from(publish.payload)
                .map_err(|_| MqttError::BufferTooSmall)?,
            unique_id
        })
    }
}

struct AwaitPuback <const BUFFER: usize, const TOPIC_SIZE: usize> {
    pid: Pid,
    publish: OwnedPublish<BUFFER, TOPIC_SIZE>,
    publish_sent: Instant,
}

impl <const BUFFER: usize, const TOPIC_SIZE: usize> From<OwnedPublish<BUFFER, TOPIC_SIZE>> for AwaitPuback<BUFFER, TOPIC_SIZE> {
    fn from(value: OwnedPublish<BUFFER, TOPIC_SIZE>) -> Self {
        let pid = match value.qospid {
            QosPid::AtLeastOnce(pid) => pid,
            unexpected_qospid => panic!("cannot convert publish with {:?} to AwaitPuback", unexpected_qospid),
        };
        let publish_sent = time::now();


        Self {
            pid: pid,
            publish: value,
            publish_sent,
        }
    }
}

struct AwaitPubrec <const BUFFER: usize, const TOPIC_SIZE: usize> {
    pid: Pid,
    publish: OwnedPublish<BUFFER, TOPIC_SIZE>,
    publish_sent: Instant
}

impl <const BUFFER: usize, const TOPIC_SIZE: usize> From<OwnedPublish<BUFFER, TOPIC_SIZE>> for AwaitPubrec<BUFFER, TOPIC_SIZE> {
    fn from(value: OwnedPublish<BUFFER, TOPIC_SIZE>) -> Self {
        let pid = match value.qospid {
            QosPid::ExactlyOnce(pid) => pid,
            unexpected_qospid => panic!("cannot convert publish with {:?} to AwaitPubrec", unexpected_qospid),
        };
        let publish_sent = time::now();

        Self {
            pid: pid,
            publish: value,
            publish_sent
        }
    }
}

struct AwaitPubcomp {
    pid: Pid,
    pubrel_sent: Instant,
    unique_id: UniqueID
}

impl From<SendPubrel> for AwaitPubcomp {
    fn from(value: SendPubrel) -> Self {
        Self {
            pid: value.pid,
            pubrel_sent: time::now(),
            unique_id: value.unique_id
                .expect("llegal state: when crating a AwaitPubcomp from SendPubrel there must be an unique id"),
        }
    }
}

struct SendPubrel {
    pid: Pid,

    /// A pubrel must be sent for all received pubrec
    /// So if there is no await pubrec entry there may not be an unique id
    unique_id: Option<UniqueID>,
}

impl SendPubrel {
    fn new_anonymous(pid: Pid) -> Self {
        Self {
            pid,
            unique_id: None
        }
    }
}

impl <const BUFFER: usize, const TOPIC_SIZE: usize> From<AwaitPubrec<BUFFER, TOPIC_SIZE>> for SendPubrel {
    fn from(value: AwaitPubrec<BUFFER, TOPIC_SIZE>) -> Self {
        Self {
            pid: value.pid,
            unique_id: Some(value.publish.unique_id)
        }
    }
}

struct PublishesInner<const PARALLEL_PUBLISHES: usize, const BUFFER: usize, const TOPIC_SIZE: usize> {
    send_publish: Vec<OwnedPublish<BUFFER, TOPIC_SIZE>, PARALLEL_PUBLISHES>,
    
    // QoS 1
    await_puback: Vec<AwaitPuback<BUFFER, TOPIC_SIZE>, PARALLEL_PUBLISHES>,

    // QoS 2
    // After pubrec received: drop publish
    await_pubrec: Vec<AwaitPubrec<BUFFER, TOPIC_SIZE>, PARALLEL_PUBLISHES>,
    send_pubrel: Vec<SendPubrel, PARALLEL_PUBLISHES>,
    //After pubcomp: drop state
    await_pubcomp: Vec<AwaitPubcomp, PARALLEL_PUBLISHES>,
}

impl <const PARALLEL_PUBLISHES: usize, const BUFFER: usize, const TOPIC_SIZE: usize> PublishesInner<PARALLEL_PUBLISHES, BUFFER, TOPIC_SIZE> {
    fn new() -> Self {
        Self {
            send_publish: Vec::new(),
            await_puback: Vec::new(),
            await_pubrec: Vec::new(),
            send_pubrel: Vec::new(),
            await_pubcomp: Vec::new()
        }
    }

    // /// resets the queues
    // /// 
    // /// All pending publishes stay. 
    // pub fn reset(&mut self) -> Result<(), MqttError> {
    //     self.await_pubcomp.clear();
    //     self.send_pubrel.clear();

    //     while let Some(publish) = self.await_puback.pop() {
    //         self.send_publish.push(publish.publish)
    //             .map_err(|publish| MqttError::QueueFull(publish.qospid))?;
    //     }

    //     while let Some(publish) = self.await_pubrec.pop() {
    //         self.send_publish.push(publish.publish)
    //             .map_err(|publish| MqttError::QueueFull(publish.qospid))?;
    //     }

    //     Ok(())
    // }

    pub async fn process_puback(&mut self, pid: Pid, publisher: DynPublisher<'_, MqttEvent>) -> Result<(), MqttError> {
        if let Some(index) = self.await_puback.iter().position(|p| p.pid == pid) {
            let publish = self.await_puback.remove(index);
            free_pid(publish.pid);
            publisher.publish(MqttEvent::PublishDone(publish.publish.unique_id)).await;
            info!("received puback for {}, dropping state", pid);
            Ok(())
        } else {
            warn!("received puback to unknown publish");
            Err(MqttError::UnexpectedAck(pid))
        }
    }

    pub fn process_pubrec(&mut self, pid: Pid) -> Result<(), MqttError> {
        let send_pubrel = if let Some(index) = self.await_pubrec.iter().position(|p| p.pid == pid) {
            info!("received pubrec for {}, dropping payload", pid);
            let await_pubrec = self.await_pubrec.remove(index);
            SendPubrel::from(await_pubrec)
        } else {
            warn!("received pubrec to unknown publish");
            // Not an error because dup pubrec can occure
            SendPubrel::new_anonymous(pid)
        };

        self.send_pubrel.push(send_pubrel)
                .map_err(|send_pubrel| MqttError::QueueFull(QosPid::ExactlyOnce(send_pubrel.pid)))
    }

    pub async fn process_pubcomp(&mut self, pid: Pid, publisher: DynPublisher<'_, MqttEvent>) -> Result<(), MqttError> {
        if let Some(index) = self.await_pubcomp.iter().position(|p| p.pid == pid) {
            let publish = self.await_pubcomp.remove(index);
            free_pid(publish.pid);
            publisher.publish(MqttEvent::PublishDone(publish.unique_id)).await;
            info!("received pubcomp for {}, dropping state", pid);
            Ok(())
        } else {
            warn!("received pubcomp to unknown publish");
            Err(MqttError::UnexpectedAck(pid))
        }
    }

    async fn send_pending_publishes(&mut self, connection: &impl ConnectionState, publisher: DynPublisher<'_, MqttEvent>) -> Result<SendResult, MqttError> {
        info!("send pending publishes, {} pending", self.send_publish.len());
        let mut i = 0;
        
        while i < self.send_publish.len() {
            let current = &self.send_publish[i];

            // Check if the target queue has space
            let has_space = match current.qospid.qos() {
                QoS::AtMostOnce => true,
                QoS::AtLeastOnce => ! self.await_puback.is_full(),
                QoS::ExactlyOnce => ! self.await_pubrec.is_full(),
            };

            if has_space {
                let sent = connection.try_write_packet(&current.borrow())?;

                // if the packet could not be sent return indiating the queue is full
                if ! sent {
                    warn!("stop sending pending publishes: send buffer full");
                    return Ok(SendResult::PartiallySent)
                } else {
                    info!("sent pending publish {}", current.qospid);
                }

                let mut current = self.send_publish.remove(i);
                current.dup = true; // all subsequent publishes are dups
                match current.qospid.qos() {
                    QoS::AtMostOnce => {
                        // Forget about the publish
                        publisher.publish(MqttEvent::PublishDone(current.unique_id)).await;
                    }, 
                    QoS::AtLeastOnce => unsafe { self.await_puback.push_unchecked(current.into()) },
                    QoS::ExactlyOnce => unsafe { self.await_pubrec.push_unchecked(current.into()) },
                }
                
            } else {
                warn!("did not send publish because next queue is full");
                i += 1;
            }
        }

        Ok(SendResult::SentAll)
    }

    fn send_pending_pubrel(&mut self, connection: &impl ConnectionState) -> Result<SendResult, MqttError> {
        info!("send pending pubrel, {} pending", self.send_pubrel.len());

        let mut i = 0;
        while i < self.send_pubrel.len() {
            let current = &self.send_pubrel[i];

            let has_space = !self.await_pubcomp.is_full();
            if has_space {
                let sent = connection.try_write_packet(&Packet::Pubrel(current.pid))?;

                // if the packet could not be sent return indiating the queue is full
                if ! sent {
                    warn!("stop sending pending pubrel: send buffer full");
                    return Ok(SendResult::PartiallySent)
                }

                let current = self.send_pubrel.remove(i);
                unsafe { self.await_pubcomp.push_unchecked(current.into()) };
            } else {
                warn!("did not send pubrel because await_pubcomp queue is full");
                i += 1;
            }

        }

        Ok(SendResult::SentAll)
    }

    fn resend_publishes(&mut self, connection: &impl ConnectionState) -> Result<SendResult, MqttError>  {
        let now = time::now();


        // resend publishes waiting for puback
        for pending in self.await_puback.iter_mut().filter(|pending| now - pending.publish_sent > RESEND_DURATION) {
            let sent = connection.try_write_packet(&pending.publish.borrow())?;
            if sent {
                warn!("resend publish {} for pending puback", pending.pid);
                pending.publish_sent = now;
            } else {
                warn!("could not resend publish for pending puback: send buffer full");
                return Ok(SendResult::PartiallySent);
            }
        }

        let now = time::now();

        // resend publishes waiting for pubrec
        for pending in self.await_pubrec.iter_mut().filter(|pending| now - pending.publish_sent > RESEND_DURATION) {
            let sent = connection.try_write_packet(&pending.publish.borrow())?;
            if sent {
                warn!("resend publish {} for pending pubrec", pending.pid);
                pending.publish_sent = now;
            } else {
                warn!("could not resend publish for pending pubrec: send buffer full");
                return Ok(SendResult::PartiallySent);
            }
        }

        Ok(SendResult::SentAll)
    }

    fn resend_pubrel(&mut self, connection: &impl ConnectionState) -> Result<SendResult, MqttError>  {
        let now = time::now();

        // resend pubrel waiting for pubcomp
        for pending in self.await_pubcomp.iter_mut().filter(|pending| now - pending.pubrel_sent > RESEND_DURATION) {
            let sent = connection.try_write_packet(&Packet::Pubrel(pending.pid))?;
            if sent {
                warn!("resend pubrel {}", pending.pid);
                pending.pubrel_sent = now;
            } else {
                warn!("could not resend pubrel: send buffer full");
                return Ok(SendResult::PartiallySent);
            }
        }

        Ok(SendResult::SentAll)
    }



    pub async fn send_packets(&mut self, connection: &impl ConnectionState, publisher: DynPublisher<'_, MqttEvent>) -> Result<SendResult, MqttError> {        
        info!("publishes: send_packets");
        self.send_pending_pubrel(connection)?
            .next(|| self.send_pending_publishes(connection, publisher)).await?
            .next_sync(|| self.resend_publishes(connection))?
            .next_sync(|| self.resend_pubrel(connection))
    }
}

pub struct Publishes<M: RawMutex, const PARALLEL_PUBLISHES: usize, const BUFFER: usize, const TOPIC_SIZE: usize, const WAKERS: usize> {
    inner: Mutex<M, PublishesInner<PARALLEL_PUBLISHES, BUFFER, TOPIC_SIZE>, WAKERS>
}

impl<M: RawMutex, const PARALLEL_PUBLISHES: usize, const BUFFER: usize, const TOPIC_SIZE: usize, const WAKERS: usize> Publishes<M, PARALLEL_PUBLISHES, BUFFER, TOPIC_SIZE, WAKERS> {

    pub fn new() -> Self {
        Self { 
            inner: Mutex::new(PublishesInner::new())
        }
    }

    pub async fn send_packets(&self, connection: &impl ConnectionState, publicher: DynPublisher<'_, MqttEvent>) -> Result<SendResult, MqttError> {
        self.inner.lock().await.send_packets(connection, publicher).await
    }

    // pub fn lock(&self) -> LockFuture<'_, M, PublishesInner<PARALLEL_PUBLISHES, BUFFER, TOPIC_SIZE>, WAKERS> {
    //     self.inner.lock()
    // }

    pub async fn process_incoming_packet(&self, packet: &Packet<'_>, publisher: DynPublisher<'_, MqttEvent>) -> Result<(), MqttError> {
        let mut inner = self.inner.lock().await;
        match packet {
            Packet::Puback(pid) => inner.process_puback(*pid, publisher).await,
            Packet::Pubrec(pid) => inner.process_pubrec(*pid),
            Packet::Pubcomp(pid) => inner.process_pubcomp(*pid, publisher).await,
            unexpected_packet => {
                error!("unexpected packet {} in publish::process_incoming_packet", unexpected_packet);
                Ok(())
            }
        }
    }

    /// Add a message to the publish queue
    pub async fn publish(&self, publish: Publish<'_>, unique_id: UniqueID) -> Result<(), MqttError> {
        self.inner.try_with_lock(move |inner| -> Result<bool, MqttError> {
            if inner.send_publish.is_full() {
                Ok(false)
            } else {
                let publish = OwnedPublish::new_from_publish(&publish, unique_id)?;
                let _ = inner.send_publish.push(publish); // Checked capacity before
                Ok(true)
            }
        }).await.unwrap();

        Ok(())
    }
}


#[cfg(test)]
mod tests {

    use embassy_sync::{blocking_mutex::raw::CriticalSectionRawMutex, pubsub::PubSubChannel};
    use mqttrs2::{Packet, Publish, QosPid};

    use crate::{MqttEvent, state::{SendResult, connection::test::DummyConnectionState, pid::next_pid, publish2::Publishes}, testutils::*};

    #[test]
    fn test_qos_0_publish() {
        let connection_state = DummyConnectionState::new();
        let publishes = Publishes::<CriticalSectionRawMutex, 4, 1024, 128, 2>::new();
        let events = PubSubChannel::<CriticalSectionRawMutex, MqttEvent, 8, 1, 1>::new();

        let mut event_sub = events.subscriber().unwrap();

        let publish = Publish{
            dup: false,
            qospid: QosPid::AtMostOnce,
            retain: false,
            topic_name: "a/test/tpic",
            payload: "a test payload".as_bytes()
        };

        let unique_id = 45.into();

        // add publish to queue
        let publish_fut = publishes.publish(publish.clone(), unique_id);
        assert_ready_pin(publish_fut).unwrap();

        // send publish
        let send_fut = publishes.send_packets(&connection_state, events.dyn_publisher().unwrap());
        let send_result = assert_ready_pin(send_fut).unwrap();
        assert_eq!(send_result, SendResult::SentAll);

        // assert publish written
        connection_state.assert_packet_written(|packet| match packet {
            Packet::Publish(p) => {
                assert_eq!(p, publish);
            },
            packet => panic!("unexpected packet {:?}", packet)
        });

        // assert event fired
        let event = event_sub.try_next_message_pure();
        assert_eq!(event, Some(MqttEvent::PublishDone(unique_id)));

        // assert empty state
        let inner = publishes.inner.try_lock().unwrap();
        assert!(inner.await_puback.is_empty());
        assert!(inner.await_pubcomp.is_empty());
        assert!(inner.await_pubrec.is_empty());
        assert!(inner.send_publish.is_empty());
    }

    #[test]
    fn test_qos_1_publish() {
        let connection_state = DummyConnectionState::new();
        let publishes = Publishes::<CriticalSectionRawMutex, 4, 1024, 128, 2>::new();
        let events = PubSubChannel::<CriticalSectionRawMutex, MqttEvent, 8, 1, 1>::new();

        let mut event_sub = events.subscriber().unwrap();

        let pid = next_pid();

        let publish = Publish{
            dup: false,
            qospid: QosPid::AtLeastOnce(pid),
            retain: false,
            topic_name: "a/test/tpic",
            payload: "a test payload".as_bytes()
        };

        let unique_id = 45.into();

        // add publish to queue
        let publish_fut = publishes.publish(publish.clone(), unique_id);
        assert_ready_pin(publish_fut).unwrap();

        // send publish
        let send_fut = publishes.send_packets(&connection_state, events.dyn_publisher().unwrap());
        let send_result = assert_ready_pin(send_fut).unwrap();
        assert_eq!(send_result, SendResult::SentAll);

        // assert publish written
        connection_state.assert_packet_written(|packet| match packet {
            Packet::Publish(p) => {
                assert_eq!(p, publish);
            },
            packet => panic!("unexpected packet {:?}", packet)
        });

        // assert event not fired
        let event = event_sub.try_next_message_pure();
        assert_eq!(event, None);

        // assert request in puback await queue
        let inner = publishes.inner.try_lock().unwrap();
        assert_eq!(inner.await_puback.len(), 1);
        drop(inner);

        crate::state::pid::inspections::assert_not_freed(pid);

        // receive puback
        let puback = Packet::Puback(pid);
        let on_puback_fut = publishes.process_incoming_packet(&puback, events.dyn_publisher().unwrap());
        assert_ready_pin(on_puback_fut).unwrap();

        crate::state::pid::inspections::assert_freed(pid);

        // assert event fired
        let event = event_sub.try_next_message_pure();
        assert_eq!(event, Some(MqttEvent::PublishDone(unique_id)));

        // assert empty state
        let inner = publishes.inner.try_lock().unwrap();
        assert!(inner.await_puback.is_empty());
        assert!(inner.await_pubcomp.is_empty());
        assert!(inner.await_pubrec.is_empty());
        assert!(inner.send_publish.is_empty());
    }

    #[test]
    fn test_qos_2_publish() {
        let connection_state = DummyConnectionState::new();
        let publishes = Publishes::<CriticalSectionRawMutex, 4, 1024, 128, 2>::new();
        let events = PubSubChannel::<CriticalSectionRawMutex, MqttEvent, 8, 1, 1>::new();

        let mut event_sub = events.subscriber().unwrap();

        let pid = next_pid();

        let publish = Publish{
            dup: false,
            qospid: QosPid::ExactlyOnce(pid),
            retain: false,
            topic_name: "a/test/tpic",
            payload: "a test payload".as_bytes()
        };

        let unique_id = 45.into();

        // add publish to queue
        let publish_fut = publishes.publish(publish.clone(), unique_id);
        assert_ready_pin(publish_fut).unwrap();

        // send publish
        let send_fut = publishes.send_packets(&connection_state, events.dyn_publisher().unwrap());
        let send_result = assert_ready_pin(send_fut).unwrap();
        assert_eq!(send_result, SendResult::SentAll);

        // assert publish written
        connection_state.assert_packet_written(|packet| match packet {
            Packet::Publish(p) => {
                assert_eq!(p, publish);
            },
            packet => panic!("unexpected packet {:?}", packet)
        });

        // assert request in pubrec await queue
        let inner = publishes.inner.try_lock().unwrap();
        assert_eq!(inner.await_pubrec.len(), 1);
        drop(inner);

        // receive puback
        let pubrec = Packet::Pubrec(pid);
        let on_puback_fut = publishes.process_incoming_packet(&pubrec, events.dyn_publisher().unwrap());
        assert_ready_pin(on_puback_fut).unwrap();

        // assert request in pubrel send queue
        let inner = publishes.inner.try_lock().unwrap();
        assert_eq!(inner.send_pubrel.len(), 1);
        drop(inner);

        // send pubrel
        let send_fut = publishes.send_packets(&connection_state, events.dyn_publisher().unwrap());
        let send_result = assert_ready_pin(send_fut).unwrap();
        assert_eq!(send_result, SendResult::SentAll);

        // assert pubrel written
        connection_state.assert_packet_written(|packet| match packet {
            Packet::Pubrel(p) => {
                assert_eq!(p, pid);
            },
            packet => panic!("unexpected packet {:?}", packet)
        });

        // assert request in pubcomp await queue
        let inner = publishes.inner.try_lock().unwrap();
        assert_eq!(inner.await_pubcomp.len(), 1);
        drop(inner);

        // assert event not fired untin now
        let event = event_sub.try_next_message_pure();
        assert_eq!(event, None);

        crate::state::pid::inspections::assert_not_freed(pid);

        // receive pubcomp
        let pubcomp = Packet::Pubcomp(pid);
        let on_puback_fut = publishes.process_incoming_packet(&pubcomp, events.dyn_publisher().unwrap());
        assert_ready_pin(on_puback_fut).unwrap();

        crate::state::pid::inspections::assert_freed(pid);

        // assert event fired
        let event = event_sub.try_next_message_pure();
        assert_eq!(event, Some(MqttEvent::PublishDone(unique_id)));

        // assert empty state
        let inner = publishes.inner.try_lock().unwrap();
        assert!(inner.await_puback.is_empty());
        assert!(inner.await_pubcomp.is_empty());
        assert!(inner.await_pubrec.is_empty());
        assert!(inner.send_publish.is_empty());
    }

}