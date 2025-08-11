use core::cell::RefCell;

use embytes_buffer::BufferWriter;

use crate::misc::AsVec;

use embassy_sync::blocking_mutex;
use embassy_sync::blocking_mutex::raw::RawMutex;
use embassy_sync::signal::Signal;
use heapless::Vec;
use mqttrs2::{encode_slice, Connack, Connect, Error, LastWill, Packet, Protocol};
use pid::PidSource;
use ping::PingState;
use publish::PublishQueue;
use receives::ReceivedPublishQueue;
use sub::SubQueue;

use crate::io::AsyncSender;
use crate::{time, ClientConfig, MqttError, MqttEvent, MqttPublish};

pub(crate) const KEEP_ALIVE: usize = 60;

pub(crate) mod ping;

pub(crate) mod receives;

/// outgoing publishes
pub(crate) mod publish;
pub(crate) mod sub;
pub(crate) mod pid;

#[derive(Debug, Clone, PartialEq)]
pub(crate) enum ConnectionState {
    /// TCP Connection established, but nothis has happened yet
    InitialState,

    /// The Connect package is sent but the connack is not received yet
    ConnectSent,

    Connected,

    Failed(MqttError)

}

pub(crate) struct State<'l, M: RawMutex> {

    connection: blocking_mutex::Mutex<M, RefCell<ConnectionState>>,
    config: ClientConfig,
    ping: blocking_mutex::Mutex<M, RefCell<PingState>>,

    last_will: Option<LastWill<'l>>,

    pub(crate) publishes: PublishQueue,
    pub(crate) received_publishes: ReceivedPublishQueue,
    pub(crate) subscribes: SubQueue,

    // Signal is sent, when a request is added
    // TODO update to emassy_sync::watch::Watch is update is there
    pub(crate) on_requst_added: Signal<M, usize>,

    pub(crate) pid_source: PidSource

}

impl <'l, M: RawMutex> State<'l, M> {

    pub fn new(config: ClientConfig, last_will: Option<LastWill<'l>>) -> Self {
        Self {
            connection: blocking_mutex::Mutex::new(RefCell::new(ConnectionState::InitialState)),
            config,
            ping: blocking_mutex::Mutex::new(RefCell::new(PingState::PingSuccess(time::now()))),

            last_will: last_will,

            publishes: PublishQueue::new(),
            received_publishes: ReceivedPublishQueue::new(),
            subscribes: SubQueue::new(),

            on_requst_added: Signal::new(),

            pid_source: PidSource::new()
        }
    }

    pub fn reset(&self) {
        self.set_connection_state(ConnectionState::InitialState);
    }

    fn set_connection_state(&self, new_state: ConnectionState) {
        self.connection.lock(|inner| {
            let mut inner = inner.borrow_mut();
            *inner = new_state;
        })
    }

    #[cfg(test)]
    fn get_connection_state(&self) -> ConnectionState {
        self.connection.lock(|inner|{
            inner.borrow().clone()
        })
    }

    fn send_connect_package(&self, send_buffer: &mut impl BufferWriter) -> Result<(), MqttError> {

        let mut connect_packet = Connect{
            protocol: Protocol::MQTT311,
            keep_alive: KEEP_ALIVE as u16,
            client_id: &self.config.client_id,
            clean_session: false,
            last_will: self.last_will.clone(),
            username: None,
            password: None
        };

        if let Some(cred) = &self.config.credentials {
            connect_packet.username = Some(&cred.username);
            connect_packet.password = Some(cred.password.as_bytes());
        }

        let connect_packet = Packet::Connect(connect_packet);

        let sent = Self::encode_packet(&connect_packet, send_buffer)?;
        if sent {
            self.set_connection_state(ConnectionState::ConnectSent);
        }

        Ok(())
    }

    fn encode_packet(packet: &Packet<'_>, send_buffer: &mut impl BufferWriter) -> Result<bool, MqttError> {
        let result = encode_slice(packet, send_buffer);

        match result {
            Ok(n) => {
                send_buffer.commit(n).unwrap();
                trace!("successfully encoded {} package to send_buffer: {} bytes", packet.get_type(), n);
                Ok(true)
            },
            Err(Error::WriteZero) => {
                debug!("cannot write {} packet: buffer not enaugh space", packet.get_type());
                Ok(false)
            },
            Err(e) => {
                error!("error encoding {} package: {}", packet.get_type(), e);
                Err(MqttError::CodecError)
            }
        }
    }

    fn process_connack(&self, connack: &Connack) -> Result<Option<MqttEvent>, MqttError> {

        match connack.code {
            mqttrs2::ConnectReturnCode::Accepted => {
                self.set_connection_state(ConnectionState::Connected);
                info!("connction to broker established");

                // Add autosubscribe requests
                self.subscribes.add_auto_subscribes(
                    &self.config.auto_subscribes,
                    || self.pid_source.next_pid()
                );

                self.on_requst_added.signal(5);

                Ok(Some(MqttEvent::Connected))
            },
            mqttrs2::ConnectReturnCode::RefusedProtocolVersion | mqttrs2::ConnectReturnCode::RefusedIdentifierRejected | mqttrs2::ConnectReturnCode::ServerUnavailable => {
                error!("connack returned error: {}", connack.code);
                self.set_connection_state(ConnectionState::Failed(MqttError::ConnackError));
                Err(MqttError::ConnackError)
            },
            mqttrs2::ConnectReturnCode::BadUsernamePassword | mqttrs2::ConnectReturnCode::NotAuthorized => {
                error!("connack: authentication failed: {}", connack.code);
                self.set_connection_state(ConnectionState::Failed(MqttError::AuthenticationError));
                Err(MqttError::AuthenticationError)
            }
        }
    }

    fn process_pingresp(&self) {
        debug!("received pingresp from broker");
        self.ping.lock(|inner|{
            inner.borrow_mut().on_ping_response();
        });
    }

    pub(crate) fn send_packets(&self, send_buffer: &mut impl BufferWriter, control_sender: & impl AsyncSender<MqttEvent>) -> Result<(), MqttError> {

        let state = self.connection.lock(|inner| inner.borrow().clone());

        match state {
            ConnectionState::InitialState => self.send_connect_package(send_buffer),

            // Do not send anything while connecting
            ConnectionState::ConnectSent => Ok(()),

            // Send ping, subscribes, publishes, ...
            ConnectionState::Connected => self.send_packets_connected(send_buffer, control_sender),

            ConnectionState::Failed(mqtt_error) => Err(mqtt_error.clone()),
        }
    }

    pub(crate) fn send_ping(&self, send_buffer: &mut impl BufferWriter) -> Result<(), MqttError> {
        self.ping.lock(|inner|{
            let mut inner = inner.borrow_mut();

            if inner.should_send_ping() {
                let ping = Packet::Pingreq;
                let sent = Self::encode_packet(&ping, send_buffer)?;
                if sent {
                    inner.ping_sent();
                }
            }
    
            Ok(())
        })
    }

    fn send_packets_connected(&self, send_buffer: &mut impl BufferWriter, control_sender: &impl AsyncSender<MqttEvent>) -> Result<(), MqttError> {

        let is_critical = self.ping.lock(|inner|{
            inner.borrow().is_critical_delay()
        });

        // Do not do anything else if the ping delay is critical (near keepalive)
        if is_critical {
            warn!("ping delay is critical: skip network traffic");
            return Ok(());
        }

        // QoS messages for received publishes
        self.received_publishes.process(send_buffer)?;

        // Subscribe & unsubscribe
        self.subscribes.process(send_buffer)?;

        // Publish and republish packets
        self.publishes.process(send_buffer, control_sender)?;

        Ok(())
    }

    /// Processes incoming packets
    pub(crate) async fn process_packet(&self, p: &Packet<'_>, send_buffer: &mut impl BufferWriter, reveived_publishes: &impl AsyncSender<MqttPublish>) -> Result<Vec<MqttEvent, 16>, MqttError> {

        match p {
            
            Packet::Connack(connack) => {
                self.process_connack(connack)
                    .map(|op| op.as_vec())
            },
            
            Packet::Publish(publish) => {
                let publish = self.received_publishes.process_publish(publish).await;
                if let Some(publish) = publish {
                    reveived_publishes.send(publish).await;
                }

                Ok(Vec::new())
            },
            
            Packet::Puback(pid) => {
                let result = self.publishes.process_puback(pid);
                Ok(result.as_vec())
            },
            
            Packet::Pubrec(pid) => {
                self.publishes.process_pubrec(pid, send_buffer)?;
                Ok(Vec::new())
            },

            Packet::Pubrel(pid) => {
                self.received_publishes.process_pubrel(pid.clone());
                Ok(Vec::new())
            },

            Packet::Pubcomp(pid) => {
                let result = self.publishes.process_pubcomp(pid);
                Ok(result.as_vec())
            },

            Packet::Suback(suback) => {
                let result = self.subscribes.process_suback(suback);
                Ok(result.as_vec())
            },
            
            Packet::Unsuback(pid) => {
                let result = self.subscribes.process_unsuback(pid);
                Ok(result.as_vec())
            },
            
            Packet::Pingresp => {
                self.process_pingresp();
                Ok(Vec::new())
            },

            // # These Packages cannot be send Server -> Client
            // # And are treated as unexpected
            // Packet::Connect(connect) => todo!(),
            // Packet::Disconnect => todo!(),
            // Packet::Pingreq => todo!(),
            // Packet::Unsubscribe(unsubscribe) => todo!(),
            // Packet::Subscribe(subscribe) => todo!(),

            unexpected => {
                error!("unexpected packet {} received from broker", unexpected.get_type());
                Ok(Vec::new())
            }
        }
    }

    pub(crate) async fn on_ping_required(&self) {
        match self.ping.lock(|p| p.borrow().ping_pause()) {
            Some(pause) => time::sleep(pause).await,
            None => {},
        }
        debug!("Mqtt ping required");
    }

}

#[cfg(test)]
mod tests {
    use core::time::Duration;
    use std::time::Instant;

    use embytes_buffer::{new_stack_buffer, Buffer, BufferReader, ReadWrite};
    use embassy_sync::{blocking_mutex::raw::CriticalSectionRawMutex, channel::Channel};
    use heapless::{String, Vec};
    use mqttrs2::{decode_slice_with_len, Connack, ConnectReturnCode, LastWill, Packet, PacketType, QoS};

    use crate::{io::AsyncSender, state::{ConnectionState, State, KEEP_ALIVE}, time, ClientConfig, MqttError, MqttEvent};

    use super::ping::PingState;

    struct PanicSender;

    impl <T> AsyncSender<T> for PanicSender {
        async fn send(&self, _item: T) {
            panic!("called send() on PanicSender");
        }
        
        fn try_send(&self, _item: T) -> Result<(), T> {
            panic!("called try_send() on PanicSender");
        }
    }

    struct Test<'t> {
        state: State<'t, CriticalSectionRawMutex>,
        send_buffer: Buffer<[u8; 1024]>,
        control_ch: Channel<CriticalSectionRawMutex, MqttEvent, 16>
    }

    impl <'t> Test<'t> {
        fn new (config: ClientConfig) -> Self {
            Self {
                state: State::new(config, None),
                send_buffer: new_stack_buffer(),
                control_ch: Channel::new()
            }
        }

        fn new_with_last_will(config: ClientConfig, last_will: LastWill<'t>) -> Self {
            Self {
                state: State::new(config, Some(last_will)),
                send_buffer: new_stack_buffer(),
                control_ch: Channel::new()
            }
        }

        fn expect_no_packet(&mut self) {
            let reader = self.send_buffer.create_reader();
            let op = decode_slice_with_len(&reader).unwrap();
            assert_eq!(op, None);
        }

        fn expect_packet<R, F: FnOnce(&Packet<'_>) -> R>(&mut self, operator: F) -> R {
            let reader = self.send_buffer.create_reader();
            let (n, packet) = decode_slice_with_len(&reader).unwrap().expect("there must be a packet");
            reader.add_bytes_read(n);

            operator(&packet)
        }

        async fn process_packet(&mut self, packet: &Packet<'_>) -> Result<Vec<MqttEvent, 16>, MqttError>{
            self.state.process_packet(
                packet, 
                &mut self.send_buffer.create_writer(), 
                &PanicSender
            ).await
        }
    }

    #[tokio::test]
    async fn test_on_ping_required() {
        time::test_time::set_static_now();

        let mut config = ClientConfig{
            client_id: String::new(),
            credentials: None,
            auto_subscribes: Vec::new()
        };

        config.client_id.push_str("1234567890").unwrap();

        let mut test = Test::new(config);
        test.state.send_packets(&mut test.send_buffer.create_writer(), &test.control_ch).unwrap();
        assert_eq!(test.state.get_connection_state(), ConnectionState::ConnectSent);

        let ping_required = test.state.on_ping_required();
        tokio::pin!(ping_required);

        let wait = tokio::time::sleep(core::time::Duration::from_millis(50));
        tokio::pin!(wait);

        tokio::select! {
            _ = &mut ping_required => {
                panic!("ping is not required yet!");
            },
            _ = wait => {}
        }

        let wait = tokio::time::sleep(core::time::Duration::from_millis(50));
        tokio::pin!(wait);

        time::test_time::advance_time(Duration::from_secs(KEEP_ALIVE as u64) / 2 + Duration::from_secs(1));

        tokio::select! {
            _ = &mut ping_required => {},
            _ = wait => {
                panic!("ping must be now required")
            }
        }
    }


    #[tokio::test]
    async fn test_connect_and_connack() {
        time::test_time::set_default();

        let mut config = ClientConfig{
            client_id: String::new(),
            credentials: None,
            auto_subscribes: Vec::new()
        };

        config.client_id.push_str("1234567890").unwrap();

        let mut test = Test::new(config);

        assert_eq!(test.state.get_connection_state(), ConnectionState::InitialState);

        test.state.send_packets(&mut test.send_buffer.create_writer(), &test.control_ch).unwrap();

        assert_eq!(test.state.get_connection_state(), ConnectionState::ConnectSent);

        test.expect_packet(|p| {
            if let Packet::Connect(c) = p {
                assert_eq!(c.client_id, "1234567890");
                assert_eq!(c.password, None);
                assert_eq!(c.username, None);
            } else {
                panic!("expected connect packet");
            }
        });

        assert_eq!(test.state.get_connection_state(), ConnectionState::ConnectSent);

        let event = test.process_packet(&Packet::Connack(Connack{
            session_present: false,
            code: ConnectReturnCode::Accepted
        })).await.unwrap().into_iter().next().expect("expected connected event");

        assert_eq!(MqttEvent::Connected, event);

        assert_eq!(test.state.get_connection_state(), ConnectionState::Connected);
    }

    #[tokio::test]
    async fn test_connect_and_connack_with_last_will() {
        time::test_time::set_default();

        let mut config = ClientConfig{
            client_id: String::new(),
            credentials: None,
            auto_subscribes: Vec::new()
        };

        config.client_id.push_str("1234567890").unwrap();

        const LAST_WILL_TOPIC: &str = "some/topic";
        const LAST_WILL_MESSAGE: &str = "i-am-dead";
        let last_will = LastWill {
            topic: LAST_WILL_TOPIC,
            message: LAST_WILL_MESSAGE.as_bytes(),
            qos: QoS::ExactlyOnce,
            retain: true
        };

        let mut test = Test::new_with_last_will(config, last_will);

        assert_eq!(test.state.get_connection_state(), ConnectionState::InitialState);

        test.state.send_packets(&mut test.send_buffer.create_writer(), &test.control_ch).unwrap();

        assert_eq!(test.state.get_connection_state(), ConnectionState::ConnectSent);

        test.expect_packet(|p| {
            if let Packet::Connect(c) = p {
                assert_eq!(c.client_id, "1234567890");
                assert_eq!(c.password, None);
                assert_eq!(c.username, None);
                
                let received_last_will = c.last_will.as_ref().unwrap();
                assert_eq!(received_last_will.message, LAST_WILL_MESSAGE.as_bytes());
                assert_eq!(received_last_will.topic, LAST_WILL_TOPIC);
                assert_eq!(received_last_will.qos, QoS::ExactlyOnce);
                assert_eq!(received_last_will.retain, true);
            } else {
                panic!("expected connect packet");
            }
        });

        assert_eq!(test.state.get_connection_state(), ConnectionState::ConnectSent);

        let event = test.process_packet(&Packet::Connack(Connack{
            session_present: false,
            code: ConnectReturnCode::Accepted
        })).await.unwrap().into_iter().next().expect("expected connected event");

        assert_eq!(MqttEvent::Connected, event);

        assert_eq!(test.state.get_connection_state(), ConnectionState::Connected);
    }

    #[tokio::test]
    async fn test_ping() {
        let start_time = Instant::now();
        time::test_time::set_time(start_time);

        let config = ClientConfig{
            client_id: String::new(),
            credentials: None,
            auto_subscribes: Vec::new()
        };

        let mut test = Test::new(config);
        test.state.set_connection_state(ConnectionState::Connected);

        test.state.send_packets(&mut test.send_buffer.create_writer(), &test.control_ch).unwrap();
        test.state.send_ping(&mut test.send_buffer.create_writer()).unwrap();
        test.expect_no_packet();

        time::test_time::advance_time(Duration::from_secs(40));

        test.state.send_packets(&mut test.send_buffer.create_writer(), &test.control_ch).unwrap();
        test.state.send_ping(&mut test.send_buffer.create_writer()).unwrap();
        test.expect_packet(|p| {
            if Packet::Pingreq != *p {
                panic!("expected Packet::Pingreq");
            }
        });

        test.state.ping.lock(|inner|{
            let inner = inner.borrow();

            if let PingState::AwaitingResponse { last_success, ping_request_sent } = *inner {
                assert_eq!(last_success, start_time);
                assert_eq!(ping_request_sent, start_time + Duration::from_secs(40));
            } else {
                panic!("expected PingState::AwaitingResponse");
            }
        });
        time::test_time::advance_time(Duration::from_secs(2));

        test.process_packet(&Packet::Pingresp).await.unwrap();

        test.state.ping.lock(|inner|{
            let inner = inner.borrow();

            if let PingState::PingSuccess(last_ping) = *inner {
                assert_eq!(last_ping, start_time + Duration::from_secs(42));
            } else {
                panic!("expected PingState::PingSuccess");
            }
        });

    }

    #[tokio::test]
    async fn test_auto_subscribe() {

        let config: ClientConfig = ClientConfig::new_with_auto_subscribes(
            "asghfdasdhasdh", 
            None, 
            [ "test1", "test2" ].into_iter(), 
            QoS::AtLeastOnce
        );

        let mut test = Test::new(config);

        test.state.send_packets(&mut test.send_buffer.create_writer(), &test.control_ch).unwrap();
        test.expect_packet(|p|{
            assert_eq!(p.get_type(), PacketType::Connect, "expected connect packet");
        });

        test.process_packet(&Packet::Connack(Connack { 
            session_present: false, 
            code: ConnectReturnCode::Accepted 
        })).await.unwrap();

        test.state.send_packets(&mut test.send_buffer.create_writer(), &test.control_ch).unwrap();
        test.expect_packet(|p|{
            if let Packet::Subscribe(s) = p {
                assert_eq!(1, s.topics.len());
                let topic = s.topics.first().unwrap();
                assert_eq!(&topic.topic_path, "test1");
                assert_eq!(topic.qos, QoS::AtLeastOnce);
            } else {
                panic!("expected subscribe packet but got {:?}", p.get_type());
            }
        });

        test.state.send_packets(&mut test.send_buffer.create_writer(), &test.control_ch).unwrap();
        test.expect_packet(|p|{
            if let Packet::Subscribe(s) = p {
                assert_eq!(1, s.topics.len());
                let topic = s.topics.first().unwrap();
                assert_eq!(&topic.topic_path, "test2");
                assert_eq!(topic.qos, QoS::AtLeastOnce);
            } else {
                panic!("expected subscribe packet but got {:?}", p.get_type());
            }
        });

        test.state.send_packets(&mut test.send_buffer.create_writer(), &test.control_ch).unwrap();
        test.expect_no_packet();
    }

}