#![cfg_attr(not(feature = "std"), no_std)]

use core::{cell::RefCell, net::IpAddr};

use embassy_sync::blocking_mutex::{raw::CriticalSectionRawMutex, Mutex};
use heapless::{Deque, String};
use thiserror::Error;

use heapless::Vec;

use mqttrs2::{Pid, QosPid};
pub use mqttrs2::QoS;

// This must come first so the macros are visible
pub(crate) mod fmt;

pub mod state;

pub(crate) mod time;
pub mod client;

pub(crate) mod buffer;

pub mod packet;

pub(crate) mod mutex;

#[cfg(test)]
pub mod testutils;


#[derive(Debug, Error, Clone, PartialEq)]
#[cfg_attr(feature = "defmt", derive(defmt::Format))]
pub enum MqttError {

    #[error("TCP conenction failed")]
    ConnectionFailed2(embedded_io_async::ErrorKind),

    #[error("DNS failed")]
    DnsFailed,

    #[error("buffer too small")]
    BufferTooSmall,

    #[error("connection rejected by broker")]
    ConnackError,

    #[error("The connection was rejected because of invalid / missing authentication")]
    AuthenticationError,

    #[error("Error while encoding and decoding packages")]
    CodecError(mqttrs2::Error),

    #[error("Payload of received message is too long")]
    ReceivedMessageTooLong,

    #[error("The suback / unsuback packet arrived with an error code")]
    SubscribeOrUnsubscribeFailed,

    #[error("Some internal error occured")]
    InternalError,

    /// The send queue is full. According to the mqtt spec the connection should be closed in this situation
    #[error("sending packet `{0:?}`: send queue full")]
    QueueFull(QosPid),

    #[error("received ack for unknown pid `{0:?}`")]
    UnexpectedAck(Pid),

    #[error("error with pubsub: `{0:?}`")]
    PubsubError(embassy_sync::pubsub::Error),

    #[error("topic size too big")]
    TopicSizeError,
}
impl MqttError {
    pub(crate) fn new_dns(err: &dyn core::fmt::Debug) -> Self {
        error!("dns failed: {:?}", err);
        Self::DnsFailed
    }
}

impl From<embedded_io_async::ErrorKind> for MqttError {
    fn from(err_kind: embedded_io_async::ErrorKind) -> Self {
        Self::ConnectionFailed2(err_kind)
    }
}

impl From<embassy_sync::pubsub::Error>  for MqttError {
    fn from(err: embassy_sync::pubsub::Error) -> Self {
        Self::PubsubError(err)
    }
}

impl From<mqttrs2::Error> for MqttError {
    fn from(value: mqttrs2::Error) -> Self {
        Self::CodecError(value)
    }
}


/// Credentials used to connecto to the broker
#[derive(Clone)]
pub struct ClientCredentials<'a> {
    pub username: &'a str,
    pub password: &'a str,
}

impl <'a> ClientCredentials<'a> {
    pub fn new(username: &'a str, password: &'a str) -> Self {
        Self {
            username, password
        }
    }
}

/// An [`AutoSubscribe`] is sent to the broker after connected.
/// 
/// It is also sent to the broker after reconnects. This should be the preferrd way to subscribe to topics.
#[derive(Debug, Clone)]
pub struct AutoSubscribe {
    pub topic: Topic,
    pub qos: QoS
}

impl TryInto<mqttrs2::SubscribeTopic> for &AutoSubscribe {
    type Error = MqttError;

    fn try_into(self) -> Result<mqttrs2::SubscribeTopic, Self::Error> {
        let topic = mqttrs2::SubscribeTopic {
            qos: self.qos,
            topic_path: String::try_from(self.topic.as_ref())
                .map_err(|_| MqttError::TopicSizeError)?
        };
        Ok(topic)
    }
}

impl AutoSubscribe {
    pub fn new(topic: &str, qos: QoS) -> Self {
        let mut this = Self {
            topic: Topic::new(),
            qos
        };
        this.topic.push_str(topic).unwrap();
        this
    }
}

#[derive(Clone)]
pub enum Host<'a> {
    Hostname(&'a str),
    Ip(IpAddr)
}

#[derive(Clone)]
pub struct ClientConfig<'a> {
    pub host: Host<'a>,
    pub port: Option<u16>,
    pub client_id: &'a str,
    pub credentials: Option<ClientCredentials<'a>>,
    pub auto_subscribes: Vec<AutoSubscribe, 10>
}

impl <'a> ClientConfig<'a> {
    pub fn new(host: Host<'a>, port: Option<u16>, client_id: &'a str, credentials: Option<ClientCredentials<'a>>) -> Self {
        Self {
            host,
            port,
            client_id,
            credentials,
            auto_subscribes: Vec::new()
        }
    }

    pub fn new_with_auto_subscribes<'b>(host: Host<'a>, port: Option<u16>, client_id: &'a str, credentials: Option<ClientCredentials<'a>>, auto_subscribes: impl Iterator<Item = &'b str>, qos: QoS) -> Self {

        let mut this = Self {
            host,
            port,
            client_id,
            credentials,
            auto_subscribes: Vec::new()
        };

        for topic in auto_subscribes {
            let mut topic_string = Topic::new();
            topic_string.push_str(topic).unwrap();
            let auto_subscribe = AutoSubscribe{
                topic: topic_string,
                qos
            };
            this.auto_subscribes.push(auto_subscribe).unwrap();
        }


        this
    }
}

pub const MAX_TOPIC_SIZE: usize = 64;
pub const MQTT_PAYLOAD_MAX_SIZE: usize = 1024;

pub type Topic = heapless::String<MAX_TOPIC_SIZE>;


#[derive(Debug, Clone, PartialEq)]
#[cfg_attr(feature = "defmt", derive(defmt::Format))]
pub(crate) enum MqttEvent {

    PublishDone(UniqueID),
    SubscribeDone(UniqueID, Result<QoS, MqttError>),
    UnsubscribeDone(UniqueID)
}

struct UniqueIDPool {
    next_unused: u32,
    pool: Deque<u32, 16>
}

impl UniqueIDPool {

    const fn new() -> Self {
        Self {
            next_unused: 0,
            pool: Deque::new()
        }
    }

    fn next(&mut self) -> UniqueID {
        if let Some(id) = self.pool.pop_front() {
            UniqueID(id)
        } else {
            self.take()
        }
    }

    fn take(&mut self) -> UniqueID {
        let id = self.next_unused;
        if self.next_unused == u32::MAX {
            panic!("used up all unique ids");
        } else {
            self.next_unused += 1;
        }


        UniqueID(id)
    }

    fn free(&mut self, id: UniqueID) {
        self.pool.push_back(id.0)
            .inspect_err(|err| {
                error!("UniqueId pool full, dropping {} forever", err);
            })
        .unwrap()
    }

}

static UNIQUE_ID_POOL: Mutex<CriticalSectionRawMutex, RefCell<UniqueIDPool>> = Mutex::new(RefCell::new(UniqueIDPool::new()));

#[derive(Debug, PartialEq, Eq, Clone, Copy)]
pub(crate) struct UniqueID(u32);

#[cfg(test)]
impl From<u32> for UniqueID {
    fn from(value: u32) -> Self {
        Self(value)
    }
}

impl UniqueID {

    pub(crate) fn new() -> Self {
        UNIQUE_ID_POOL.lock(|inner| inner.borrow_mut().next())
    }

    pub(crate) fn free(self) {
        UNIQUE_ID_POOL.lock(|inner| inner.borrow_mut().free(self))
    }

}




