
use embassy_sync::{blocking_mutex::raw::RawMutex, channel::{Receiver, Sender}, pubsub::{PubSubChannel, WaitResult}};
use mqttrs2::QoS;

use crate::{MqttError, MqttEvent, MqttPublish, MqttRequest, Topic, UniqueID};

/// The MQTT Client to publish messages, subscribe, unsubscribe and receive messages
#[derive(Clone)]
pub struct MqttClient<'a, M: RawMutex> {

    pub(super) control_reveiver: &'a PubSubChannel<M, MqttEvent, 4, 16, 8>,
    pub(super) request_sender: Sender<'a, M, MqttRequest, 4>,
    pub(super) received_publishes: Receiver<'a, M, MqttPublish, 4>

}

impl <'a, M: RawMutex> MqttClient<'a, M> {

    /// Publish a MQTT message with the given parameters
    /// 
    /// Waits until there is a successful publish result. The publish is successful after all acknolodgements 
    /// accordings to the selected [`QoS`] have bee exchanged
    pub async fn publish(&self, topic: &str, payload: &[u8], qos: QoS, retain: bool) -> Result<(), MqttError> {

        let id = UniqueID::new();
        let publish = MqttPublish::new(topic, payload, qos, retain);

        let mut subscriber = self.control_reveiver.subscriber()
            .map_err(|e| {
                error!("error subscribing to control receiver: {}", e);
                MqttError::InternalError
            })?;

        self.request_sender.send(MqttRequest::Publish(publish, id)).await;

        loop {
            let msg = subscriber.next_message().await;
            if let WaitResult::Message(msg) = msg {
                if let MqttEvent::PublishResult(msg_id, result) = msg {
                    if id == msg_id {
                        return result;
                    }
                }
            } else {
                error!("error reading subscrition: lost messages");
                return Err(MqttError::InternalError);
            }
        }
    }

    /// Subscribe to a topic
    /// 
    /// The method returns after the suback has bee received
    pub async fn subscribe(&self, topic: &str) -> Result<(), MqttError> {
        let id = UniqueID::new();

        let mut subscriber = self.control_reveiver.subscriber()
            .map_err(|e| {
                error!("error subscribing to control receiver: {}", e);
                MqttError::InternalError
            })?;

        let mut topic_owned = Topic::new();
        topic_owned.push_str(topic).unwrap();
        self.request_sender.send(MqttRequest::Subscribe(topic_owned, id)).await;

        loop {
            let msg = subscriber.next_message().await;
            if let WaitResult::Message(msg) = msg {
                if let MqttEvent::SubscribeResult(msg_id, result) = msg {
                    if id == msg_id {
                        return result.map(|_| ());
                    }
                }
            } else {
                error!("error reading subscrition: lost messages");
                return Err(MqttError::InternalError);
            }
        }
    }

    /// unsubscribe from a topic
    /// 
    /// waits until the unsuback has bee received
    pub async fn unsubscribe(&self, topic: &str) -> Result<(), MqttError> {
        let id = UniqueID::new();

        let mut subscriber = self.control_reveiver.subscriber()
            .map_err(|e| {
                error!("error subscribing to control receiver: {}", e);
                MqttError::InternalError
            })?;

        let mut topic_owned = Topic::new();
        topic_owned.push_str(topic).unwrap();
        self.request_sender.send(MqttRequest::Unsubscribe(topic_owned, id)).await;

        loop {
            let msg = subscriber.next_message().await;
            if let WaitResult::Message(msg) = msg {
                if let MqttEvent::UnsubscribeResult(msg_id, result) = msg {
                    if id == msg_id {
                        return result;
                    }
                }
            } else {
                error!("error reading subscrition: lost messages");
                return Err(MqttError::InternalError);
            }
        }
    }

    /// Waits for the next message
    pub async fn receive(&self) -> MqttPublish {
        self.received_publishes.receive().await
    }

    /// send a disconnect packet to the broker
    pub async fn disconnect(&self) {
        self.request_sender.send(MqttRequest::Disconnect).await;
    }

    /// wait for the next event matching the `matcher`
    pub async fn on<F>(&self, matcher: F) where F: Fn(&MqttEvent) -> bool {
        let mut sub = self.control_reveiver.subscriber().unwrap();

        loop {
            let event = sub.next_message_pure().await;
            if matcher(&event) {
                break;
            }
        }
    }

}
