use core::future::Future;
use core::ops::Add;

use embassy_futures::select::select3;
use embassy_sync::pubsub::{DynSubscriber, PubSubChannel};
use embedded_nal_async::{Dns, TcpConnect};

use crate::client::MqttClient;
use crate::state::connection::{ConnectionState, TcpConnectionState};
use crate::state::pid::next_pid;
use crate::state::publish2::Publishes;
use crate::state::receives2::{ReceivedPublish, Receives};
use crate::state::request::{RequestNotification, RequestState};
use crate::state::sub2::Subs;
use crate::time::Duration;

use embassy_sync::blocking_mutex::raw::RawMutex;
use mqttrs2::{LastWill, Packet, Publish, QoS, QosPid};
use ping::PingState;

use crate::{ClientConfig, MqttError, MqttEvent, UniqueID, time};

pub(crate) const KEEP_ALIVE: usize = 60;

pub(crate) mod ping;

pub(crate) mod receives2;

pub mod connection;

/// outgoing publishes
pub(crate) mod publish2;

pub(crate) mod sub2;
pub(crate) mod pid;

pub(crate) mod request;

const RECONNECT_DURATION: Duration = Duration::from_secs(5);

/// Result returnes from methods that send packets to the network
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SendResult {
    PartiallySent,

    /// Sent all pending packets to the network or nothing to send
    SentAll,
}

impl SendResult {

    pub async fn next<F, Fut>(self, f: F) -> Result<Self, MqttError> 
    where F: FnOnce() -> Fut, Fut: Future<Output = Result<Self, MqttError>> {
        if self != Self::PartiallySent {
            let next = self + f().await?;
            Ok(next)
        } else {
            Ok(self)
        }
    }

    pub fn next_sync<F>(self, f: F) -> Result<Self, MqttError> 
    where F: FnOnce() -> Result<Self, MqttError> {
        if self != Self::PartiallySent {
            let next = self + f()?;
            Ok(next)
        } else {
            Ok(self)
        }
    }

}

impl Add<Self> for SendResult {
    type Output = Self;

    fn add(self, rhs: Self) -> Self::Output {
        match (self, rhs) {
            (Self::SentAll, Self::SentAll) => Self::SentAll,
            _ => Self::PartiallySent
        }
    }
}

pub struct State<'n, 'l, M: RawMutex, NET, DNS, const BUFFER: usize, const TOPIC: usize, const QUEUE: usize> 
where NET: TcpConnect, DNS: Dns {

    pub(crate) connection_state: TcpConnectionState<'n, 'l, M, NET, DNS, BUFFER>, 

    ping: PingState<M>,

    publishes: Publishes<M, QUEUE, BUFFER, TOPIC, 4>,
    received_publishes: Receives<M, BUFFER, TOPIC, QUEUE>,
    subscribes: Subs<M, TOPIC, QUEUE>,

    // Signal is sent, when a request is added
    on_requst_added: RequestState<M>,

    events: PubSubChannel<M, MqttEvent, 8, 16, 2>

}

impl <'n, 'l, M: RawMutex, NET, DNS, const BUFFER: usize, const TOPIC: usize, const QUEUE: usize> State<'n, 'l, M, NET, DNS, BUFFER, TOPIC, QUEUE> 
where NET: TcpConnect, DNS: Dns{

    pub fn new(config: ClientConfig<'l>, last_will: Option<LastWill<'l>>, network: &'n NET, dns: DNS) -> Self {
        Self {
            connection_state: TcpConnectionState::new(network, dns, last_will, config),

            ping: PingState::new(),

            publishes: Publishes::new(),
            received_publishes: Receives::new(),
            subscribes: Subs::new(),

            on_requst_added: RequestState::new(),
            events: PubSubChannel::new(),
        }
    }

    pub fn new_client(&self) -> MqttClient<'_, 'n, 'l, M, NET, DNS, BUFFER, TOPIC, QUEUE> {
        MqttClient::new(self)
    }

    pub(crate) async fn publish(&self, topic: &str, payload: &[u8], qos: QoS, retain: bool, unique_id: UniqueID) -> Result<(), MqttError> {

        let qospid = match qos {
            QoS::AtMostOnce => QosPid::AtMostOnce,
            QoS::AtLeastOnce => QosPid::AtLeastOnce(next_pid()),
            QoS::ExactlyOnce => QosPid::ExactlyOnce(next_pid()),
        };

        debug!("state: adding publish request with qos {}", &qospid);

        let publish = Publish {
            topic_name: topic,
            dup: false,
            qospid,
            retain,
            payload
        };

        self.publishes.publish(publish, unique_id).await?;
        self.on_requst_added.notify_new_request();

        Ok(())
    }

    pub(crate) async fn subscribe(&self, topics: &[&str], qos: QoS, unique_id: UniqueID) {
        self.subscribes.add_subscribe_request(topics, qos, unique_id).await;
        self.on_requst_added.notify_new_request();
    }

    pub(crate) async fn unsubscribe(&self, topics: &[&str], unique_id: UniqueID) {
        self.subscribes.add_unsubscribe_request(topics, unique_id).await;
        self.on_requst_added.notify_new_request();
    }

    pub(crate) fn disconnect(&self) {
        self.on_requst_added.notify_disconnect();
    }

    /// Returns true until the client disconnects
    async fn run_once(&self) -> Result<bool, MqttError> {
        if self.connection_state.get_state() != Some(connection::ConnectionStateValue::Connected) {
            info!("not connected yed: starting connection");
            self.connection_state.connect().await?;
        }

        let publisher = self.events.dyn_publisher().unwrap();

        debug!("event loop: start sending packets");

        let send_packet_result = self.publishes.send_packets(&self.connection_state, publisher).await?
            .next(|| self.received_publishes.send_packets(&self.connection_state)).await?
            .next(|| self.subscribes.send(&self.connection_state)).await?
            .next_sync(|| self.ping.send(&self.connection_state))?;

        if send_packet_result == SendResult::PartiallySent {
            debug!("partially sent packets, run io nonblocking");
            if let Some(packet) = self.connection_state.run_io_nonblocking().await? {
                self.process_packet(&packet).await?;
            }
            // Return early to rerun the loop faster
            return Ok(true);
        }

        debug!("sent all packets, run io blocking");

        // TODO make ping and resend future
        let ping_future = self.ping.ping_pause();
        let io_future = self.connection_state.run_io();
        let request_added_future = self.on_requst_added.next_notification();

        match select3(request_added_future, ping_future, io_future).await {
            embassy_futures::select::Either3::First(request) if request == RequestNotification::Disconnect => {
                debug!("run_once: disconnect request received");
                Ok(false)
            },
            embassy_futures::select::Either3::Third(packet) => {
                debug!("run_once: received packet");
                let packet = packet?;
                self.process_packet(&packet).await?;
                Ok(true)
            },
            _ => {
                debug!("run_once: stop io, new event arrived");
                Ok(true)
            },
        }
    }

    pub async fn run(&self) -> Result<(), MqttError> {
        loop {
            match self.run_once().await {
                Ok(true) => {},
                Ok(false) => {
                    // Disconnect
                    self.connection_state.disconnect().await?;
                    info!("disconnect: exit run loop");
                    return Ok(())
                },
                Err(err) => {
                    error!("connection error: {}", &err);
                    match err {
                        MqttError::ConnectionFailed2(_) |
                        MqttError::ConnackError |
                        MqttError::CodecError(_) |
                        MqttError::ReceivedMessageTooLong |
                        MqttError::QueueFull(_) |
                        MqttError::UnexpectedAck(_)  => {
                            self.connection_state.set_error();
                            time::sleep(RECONNECT_DURATION).await;
                        },

                        err => {
                            error!("not recoverable error: stop loop");
                            return Err(err)
                        },
                    }
                },
            }
        }
    }

    /// Processes incoming packets
    async fn process_packet(&self, p: &Packet<'_>) -> Result<(), MqttError> {

        let publisher = self.events.dyn_publisher().unwrap();

        match p {
            
            Packet::Connack(_connack) => {
                panic!("received connack: this must be handled by the connection module");
            },
            
            Packet::Publish(publish) => {
                self.received_publishes.on_publish(publish).await?;
                Ok(())
            },

            Packet::Puback(_) | Packet::Pubrec(_) | Packet::Pubcomp(_) => {
                self.publishes.process_incoming_packet(p, publisher).await?;
                Ok(())
            },

            Packet::Pubrel(pid) => {
                self.received_publishes.on_pubrel(*pid).await?;
                Ok(())
            },

            Packet::Suback(suback) => {
                self.subscribes.on_suback(suback, publisher).await?;
                Ok(())
            },
            
            Packet::Unsuback(pid) => {
                self.subscribes.on_unsuback(*pid, publisher).await;
                Ok(())
            },
            
            Packet::Pingresp => {
                self.ping.on_ping_response();
                Ok(())
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
                Ok(())
            }
        }
    }

    pub(crate) fn subscribe_events(&self) -> Result<DynSubscriber<'_, MqttEvent>, MqttError> {
        self.events.dyn_subscriber().map_err(|e| e.into())
    }

    /// Subscribe to received publishes
    pub fn subscribe_received_publishes(&self) -> Result<DynSubscriber<'_, ReceivedPublish<BUFFER, TOPIC>>, MqttError> {
        self.received_publishes.subscribe_publishes()
    }

}

// #[cfg(all(test, feature = "std"))]
// mod tests {
//     use core::time::Duration;
//     use std::time::Instant;

//     use embytes_buffer::{new_stack_buffer, Buffer, BufferReader, ReadWrite};
//     use embassy_sync::{blocking_mutex::raw::CriticalSectionRawMutex, channel::Channel};
//     use heapless::{String, Vec};
//     use mqttrs2::{decode_slice_with_len, Connack, ConnectReturnCode, LastWill, Packet, PacketType, QoS};

//     use crate::{state::{ConnectionState, State, KEEP_ALIVE}, time, ClientConfig, MqttError, MqttEvent};

//     use super::ping::PingState;

//     struct Test<'t> {
//         state: State<'t, CriticalSectionRawMutex>,
//         send_buffer: Buffer<[u8; 1024]>,
//         control_ch: Channel<CriticalSectionRawMutex, MqttEvent, 16>
//     }

//     impl <'t> Test<'t> {
//         fn new (config: ClientConfig) -> Self {
//             Self {
//                 state: State::new(config, None),
//                 send_buffer: new_stack_buffer(),
//                 control_ch: Channel::new()
//             }
//         }

//         fn new_with_last_will(config: ClientConfig, last_will: LastWill<'t>) -> Self {
//             Self {
//                 state: State::new(config, Some(last_will)),
//                 send_buffer: new_stack_buffer(),
//                 control_ch: Channel::new()
//             }
//         }

//         fn expect_no_packet(&mut self) {
//             let reader = self.send_buffer.create_reader();
//             let op = decode_slice_with_len(&reader).unwrap();
//             assert_eq!(op, None);
//         }

//         fn expect_packet<R, F: FnOnce(&Packet<'_>) -> R>(&mut self, operator: F) -> R {
//             let reader = self.send_buffer.create_reader();
//             let (n, packet) = decode_slice_with_len(&reader).unwrap().expect("there must be a packet");
//             reader.add_bytes_read(n);

//             operator(&packet)
//         }

//         async fn process_packet(&mut self, packet: &Packet<'_>) -> Result<Vec<MqttEvent, 16>, MqttError>{
//             self.state.process_packet(
//                 packet, 
//                 &mut self.send_buffer.create_writer()
//             ).await
//         }
//     }

//     #[tokio::test]
//     async fn test_on_ping_required() {
//         time::test_time::set_static_now();

//         let mut config = ClientConfig{
//             client_id: String::new(),
//             credentials: None,
//             auto_subscribes: Vec::new()
//         };

//         config.client_id.push_str("1234567890").unwrap();

//         let mut test = Test::new(config);
//         test.state.send_packets(&mut test.send_buffer.create_writer(), &test.control_ch).unwrap();
//         assert_eq!(test.state.get_connection_state(), ConnectionState::ConnectSent);

//         let ping_required = test.state.on_ping_required();
//         tokio::pin!(ping_required);

//         let wait = tokio::time::sleep(core::time::Duration::from_millis(50));
//         tokio::pin!(wait);

//         tokio::select! {
//             _ = &mut ping_required => {
//                 panic!("ping is not required yet!");
//             },
//             _ = wait => {}
//         }

//         let wait = tokio::time::sleep(core::time::Duration::from_millis(50));
//         tokio::pin!(wait);

//         time::test_time::advance_time(Duration::from_secs(KEEP_ALIVE as u64) / 2 + Duration::from_secs(1));

//         tokio::select! {
//             _ = &mut ping_required => {},
//             _ = wait => {
//                 panic!("ping must be now required")
//             }
//         }
//     }


//     #[tokio::test]
//     async fn test_connect_and_connack() {
//         time::test_time::set_default();

//         let mut config = ClientConfig{
//             client_id: String::new(),
//             credentials: None,
//             auto_subscribes: Vec::new()
//         };

//         config.client_id.push_str("1234567890").unwrap();

//         let mut test = Test::new(config);

//         assert_eq!(test.state.get_connection_state(), ConnectionState::InitialState);

//         test.state.send_packets(&mut test.send_buffer.create_writer(), &test.control_ch).unwrap();

//         assert_eq!(test.state.get_connection_state(), ConnectionState::ConnectSent);

//         test.expect_packet(|p| {
//             if let Packet::Connect(c) = p {
//                 assert_eq!(c.client_id, "1234567890");
//                 assert_eq!(c.password, None);
//                 assert_eq!(c.username, None);
//             } else {
//                 panic!("expected connect packet");
//             }
//         });

//         assert_eq!(test.state.get_connection_state(), ConnectionState::ConnectSent);

//         let event = test.process_packet(&Packet::Connack(Connack{
//             session_present: false,
//             code: ConnectReturnCode::Accepted
//         })).await.unwrap().into_iter().next().expect("expected connected event");

//         assert_eq!(MqttEvent::Connected, event);

//         assert_eq!(test.state.get_connection_state(), ConnectionState::Connected);
//     }

//     #[tokio::test]
//     async fn test_connect_and_connack_with_last_will() {
//         time::test_time::set_default();

//         let mut config = ClientConfig{
//             client_id: String::new(),
//             credentials: None,
//             auto_subscribes: Vec::new()
//         };

//         config.client_id.push_str("1234567890").unwrap();

//         const LAST_WILL_TOPIC: &str = "some/topic";
//         const LAST_WILL_MESSAGE: &str = "i-am-dead";
//         let last_will = LastWill {
//             topic: LAST_WILL_TOPIC,
//             message: LAST_WILL_MESSAGE.as_bytes(),
//             qos: QoS::ExactlyOnce,
//             retain: true
//         };

//         let mut test = Test::new_with_last_will(config, last_will);

//         assert_eq!(test.state.get_connection_state(), ConnectionState::InitialState);

//         test.state.send_packets(&mut test.send_buffer.create_writer(), &test.control_ch).unwrap();

//         assert_eq!(test.state.get_connection_state(), ConnectionState::ConnectSent);

//         test.expect_packet(|p| {
//             if let Packet::Connect(c) = p {
//                 assert_eq!(c.client_id, "1234567890");
//                 assert_eq!(c.password, None);
//                 assert_eq!(c.username, None);
                
//                 let received_last_will = c.last_will.as_ref().unwrap();
//                 assert_eq!(received_last_will.message, LAST_WILL_MESSAGE.as_bytes());
//                 assert_eq!(received_last_will.topic, LAST_WILL_TOPIC);
//                 assert_eq!(received_last_will.qos, QoS::ExactlyOnce);
//                 assert_eq!(received_last_will.retain, true);
//             } else {
//                 panic!("expected connect packet");
//             }
//         });

//         assert_eq!(test.state.get_connection_state(), ConnectionState::ConnectSent);

//         let event = test.process_packet(&Packet::Connack(Connack{
//             session_present: false,
//             code: ConnectReturnCode::Accepted
//         })).await.unwrap().into_iter().next().expect("expected connected event");

//         assert_eq!(MqttEvent::Connected, event);

//         assert_eq!(test.state.get_connection_state(), ConnectionState::Connected);
//     }

//     #[tokio::test]
//     async fn test_ping() {
//         let start_time = Instant::now();
//         time::test_time::set_time(start_time);

//         let config = ClientConfig{
//             client_id: String::new(),
//             credentials: None,
//             auto_subscribes: Vec::new()
//         };

//         let mut test = Test::new(config);
//         test.state.set_connection_state(ConnectionState::Connected);

//         test.state.send_packets(&mut test.send_buffer.create_writer(), &test.control_ch).unwrap();
//         test.state.send_ping(&mut test.send_buffer.create_writer()).unwrap();
//         test.expect_no_packet();

//         time::test_time::advance_time(Duration::from_secs(40));

//         test.state.send_packets(&mut test.send_buffer.create_writer(), &test.control_ch).unwrap();
//         test.state.send_ping(&mut test.send_buffer.create_writer()).unwrap();
//         test.expect_packet(|p| {
//             if Packet::Pingreq != *p {
//                 panic!("expected Packet::Pingreq");
//             }
//         });

//         test.state.ping.lock(|inner|{
//             let inner = inner.borrow();

//             if let PingState::AwaitingResponse { last_success, ping_request_sent } = *inner {
//                 assert_eq!(last_success, start_time);
//                 assert_eq!(ping_request_sent, start_time + Duration::from_secs(40));
//             } else {
//                 panic!("expected PingState::AwaitingResponse");
//             }
//         });
//         time::test_time::advance_time(Duration::from_secs(2));

//         test.process_packet(&Packet::Pingresp).await.unwrap();

//         test.state.ping.lock(|inner|{
//             let inner = inner.borrow();

//             if let PingState::PingSuccess(last_ping) = *inner {
//                 assert_eq!(last_ping, start_time + Duration::from_secs(42));
//             } else {
//                 panic!("expected PingState::PingSuccess");
//             }
//         });

//     }

//     #[tokio::test]
//     async fn test_auto_subscribe() {

//         let config: ClientConfig = ClientConfig::new_with_auto_subscribes(
//             "asghfdasdhasdh", 
//             None, 
//             [ "test1", "test2" ].into_iter(), 
//             QoS::AtLeastOnce
//         );

//         let mut test = Test::new(config);

//         test.state.send_packets(&mut test.send_buffer.create_writer(), &test.control_ch).unwrap();
//         test.expect_packet(|p|{
//             assert_eq!(p.get_type(), PacketType::Connect, "expected connect packet");
//         });

//         test.process_packet(&Packet::Connack(Connack { 
//             session_present: false, 
//             code: ConnectReturnCode::Accepted 
//         })).await.unwrap();

//         test.state.send_packets(&mut test.send_buffer.create_writer(), &test.control_ch).unwrap();
//         test.expect_packet(|p|{
//             if let Packet::Subscribe(s) = p {
//                 assert_eq!(1, s.topics.len());
//                 let topic = s.topics.first().unwrap();
//                 assert_eq!(&topic.topic_path, "test1");
//                 assert_eq!(topic.qos, QoS::AtLeastOnce);
//             } else {
//                 panic!("expected subscribe packet but got {:?}", p.get_type());
//             }
//         });

//         test.state.send_packets(&mut test.send_buffer.create_writer(), &test.control_ch).unwrap();
//         test.expect_packet(|p|{
//             if let Packet::Subscribe(s) = p {
//                 assert_eq!(1, s.topics.len());
//                 let topic = s.topics.first().unwrap();
//                 assert_eq!(&topic.topic_path, "test2");
//                 assert_eq!(topic.qos, QoS::AtLeastOnce);
//             } else {
//                 panic!("expected subscribe packet but got {:?}", p.get_type());
//             }
//         });

//         test.state.send_packets(&mut test.send_buffer.create_writer(), &test.control_ch).unwrap();
//         test.expect_no_packet();
//     }

// }