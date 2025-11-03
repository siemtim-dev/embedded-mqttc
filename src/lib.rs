#![cfg_attr(not(feature = "std"), no_std)]

use core::cell::RefCell;

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

pub mod network;

pub(crate) mod mutex;

#[cfg(test)]
pub mod testutils;


#[derive(Debug, Error, Clone, PartialEq)]
#[cfg_attr(feature = "defmt", derive(defmt::Format))]
pub enum MqttError {

    #[error("TCP Connection failed")]
    ConnectionFailed(network::NetworkError),

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

impl From<network::NetworkError>  for MqttError {
    fn from(err: network::NetworkError) -> Self {
        Self::ConnectionFailed(err)
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
pub struct ClientCredentials {
    pub username: String<32>,
    pub password: String<128>,
}

impl ClientCredentials {
    pub fn new(username: &str, password: &str) -> Self {
        let mut this = Self {
            username: String::new(),
            password: String::new()
        };

        this.username.push_str(username).unwrap();
        this.password.push_str(password).unwrap();
        this
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
pub struct ClientConfig {
    pub client_id: String<128>,
    pub credentials: Option<ClientCredentials>,
    pub auto_subscribes: Vec<AutoSubscribe, 10>
}

impl ClientConfig {
    pub fn new(client_id: &str, credentials: Option<ClientCredentials>) -> Self {
        let mut cid = String::new();
        cid.push_str(client_id).unwrap();
        Self {
            client_id: cid,
            credentials,
            auto_subscribes: Vec::new()
        }
    }

    pub fn new_with_auto_subscribes<'a>(client_id: &str, credentials: Option<ClientCredentials>, auto_subscribes: impl Iterator<Item = &'a str>, qos: QoS) -> Self {
        let mut cid = String::new();
        cid.push_str(client_id).unwrap();

        let mut this = Self {
            client_id: cid,
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




