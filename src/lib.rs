
#![feature(never_type)]

#![cfg_attr(not(feature = "std"), no_std)]

use core::{cell::Cell, ops::Deref};

use embassy_sync::blocking_mutex::{raw::CriticalSectionRawMutex, Mutex};
use heapless::String;
use thiserror::Error;

pub use buffer::*;

use mqttrs::{Pid, Publish, QosPid};
pub use mqttrs::QoS;

// This must come first so the macros are visible
pub(crate) mod fmt;

pub mod io;
pub(crate) mod state;
pub(crate) mod time;
pub mod client;

pub(crate) mod misc;


static COUNTER: Mutex<CriticalSectionRawMutex, Cell<u64>> = Mutex::new(Cell::new(0));

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "defmt", derive(defmt::Format))]
pub struct UniqueID(u64);

impl UniqueID {
    pub fn new() -> Self {
        Self(COUNTER.lock(|inner|{
            let value = inner.get();
            inner.set(value + 1);
            value
        }))
    }
}

impl Deref for UniqueID {
    type Target = u64;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

#[derive(Debug, Error, Clone, PartialEq)]
#[cfg_attr(feature = "defmt", derive(defmt::Format))]
pub enum MqttError {

    #[error("TCP Connection failed")]
    ConnectionFailed(network::NetworkError),

    #[error("The buffer is full")]
    BufferFull,

    #[error("connection rejected by broker")]
    ConnackError,

    #[error("The connection was rejected because of invalid / missing authentication")]
    AuthenticationError,

    #[error("Error while encoding and decoding packages")]
    CodecError,

    #[error("Payload of received message is too long")]
    ReceivedMessageTooLong,

    #[error("The suback / unsuback packet arrived with an error code")]
    SubscribeOrUnsubscribeFailed,

    #[error("Some internal error occured")]
    InternalError
}

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

#[derive(Clone)]
pub struct ClientConfig {
    pub client_id: String<32>,
    pub credentials: Option<ClientCredentials>
}

impl ClientConfig {
    pub fn new(client_id: &str, credentials: Option<ClientCredentials>) -> Self {
        let mut cid = String::new();
        cid.push_str(client_id).unwrap();
        Self {
            client_id: cid,
            credentials
        }
    }
}

pub const MAX_TOPIC_SIZE: usize = 64;
pub const MQTT_PAYLOAD_MAX_SIZE: usize = 64;

pub type Topic = heapless::String<MAX_TOPIC_SIZE>;

#[derive(Debug, Clone)]
#[cfg_attr(feature = "defmt", derive(defmt::Format))]
pub struct MqttPublish {
    pub topic: Topic,
    pub payload: Buffer<[u8; MQTT_PAYLOAD_MAX_SIZE]>,
    pub qos: QoS,
    pub retain: bool,
}

impl MqttPublish {

    pub fn new(topic: &str, payload: &[u8], qos: QoS, retain: bool) -> Self {
        let mut s = Self {
            topic: Topic::new(),
            payload: new_stack_buffer(),
            qos, retain
        };
        s.topic.push_str(topic).unwrap();
        s.payload.push(payload).unwrap();

        s
    }

}

impl <'a> TryFrom<&Publish<'a>> for MqttPublish {
    type Error = MqttError;

    fn try_from(value: &Publish<'a>) -> Result<Self, Self::Error> {
        let mut topic = Topic::new();
        if let Err(_) = topic.push_str(value.topic_name) {
            warn!("Topic of received message is longer than {}: {}", MAX_TOPIC_SIZE, value.topic_name.len());
            return Err(MqttError::ReceivedMessageTooLong);
        }

        let mut payload = new_stack_buffer();
        if let Err(_e) = payload.push(&value.payload) {
            warn!("Payload of received message is longer than {}: {}", MQTT_PAYLOAD_MAX_SIZE, value.payload.len());
        }

        let qos = value.qospid.qos();

        Ok(Self {
            topic, payload, qos,
            retain: value.retain
        })
    }
}

impl  MqttPublish {
    pub(crate) fn create_publish<'a>(&'a self, pid: Pid, dup: bool) -> Publish<'a> {
        let qospid = match self.qos {
            QoS::AtMostOnce => QosPid::AtMostOnce,
            QoS::AtLeastOnce => QosPid::AtLeastOnce(pid),
            QoS::ExactlyOnce => QosPid::ExactlyOnce(pid),
        };

        Publish {
            dup,
            qospid,
            retain: self.retain,
            topic_name: &self.topic,
            payload: self.payload.data()
        }
    }
}


#[derive(Debug, Clone, PartialEq)]
#[cfg_attr(feature = "defmt", derive(defmt::Format))]
pub enum MqttEvent {

    Connected,

    PublishResult(UniqueID, Result<(), MqttError>),
    SubscribeResult(UniqueID, Result<QoS, MqttError>),
    UnsubscribeResult(UniqueID, Result<(), MqttError>)
}



#[derive(Debug)]
#[cfg_attr(feature = "defmt", derive(defmt::Format))]
enum MqttRequest {

    Publish(MqttPublish, UniqueID),

    Subscribe(Topic, UniqueID),

    Unsubscribe(Topic, UniqueID),

    Disconnect,

}




