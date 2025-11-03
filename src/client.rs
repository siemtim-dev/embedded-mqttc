

use embassy_sync::{blocking_mutex::raw::RawMutex, pubsub::DynSubscriber};
use mqttrs2::QoS;

use crate::{MqttError, MqttEvent, UniqueID, network::PlattformNetwork, state::{State, connection::ConnectionState, receives2::ReceivedPublish}};

/// The MQTT Client to publish messages, subscribe, unsubscribe and receive messages
#[derive(Clone)]
pub struct MqttClient<'a, 'b, M: RawMutex, NET: PlattformNetwork, const BUFFER: usize, const TOPIC: usize, const QUEUE: usize> {
    state: &'a State<'b, M, NET, BUFFER, TOPIC, QUEUE>
}

impl <'a, 'b, M: RawMutex, NET: PlattformNetwork, const BUFFER: usize, const TOPIC: usize, const QUEUE: usize> MqttClient<'a, 'b, M, NET, BUFFER, TOPIC, QUEUE> {

    pub(crate) fn new(state: &'a State<'b, M, NET, BUFFER, TOPIC, QUEUE>) -> Self {
        Self {
            state
        }
    }

    pub async fn on_auto_subscribes_done(&self) {
        self.state.connection_state.await_connected().await;
    }

    async fn await_event<F, U>(mut subscriber: DynSubscriber<'_, MqttEvent>, f: F) -> U 
    where F: Fn(MqttEvent) -> Option<U> {
        loop {
            let event = subscriber.next_message_pure().await;
            if let Some(result) = f(event) {
                return result;
            }
        }
    } 

    /// Publish a MQTT message with the given parameters
    /// 
    /// Waits until there is a successful publish result. The publish is successful after all acknolodgements 
    /// accordings to the selected [`QoS`] have bee exchanged
    pub async fn publish(&self, topic: &str, payload: &[u8], qos: QoS, retain: bool) -> Result<(), MqttError> {
        let unique_id = UniqueID::new();
        let subscriber = self.state.subscribe_events()?;

        self.state.publish(topic, payload, qos, retain, unique_id).await?;

        Self::await_event(subscriber, |event | match event {
            MqttEvent::PublishDone(id) if id == unique_id => Some(()),
            _ => None
        }).await;

        unique_id.free();

        Ok(())
    }

    /// Subscribe to a topic
    /// 
    /// The method returns after the suback has bee received
    pub async fn subscribe(&self, topic: &str, qos: QoS) -> Result<(), MqttError> {
        let unique_id = UniqueID::new();
        let subscriber = self.state.subscribe_events()?;

        self.state.subscribe(&[topic], qos, unique_id).await;

        let result = Self::await_event(subscriber, |event | match event {
            MqttEvent::SubscribeDone(id, result) if id == unique_id => Some(result),
            _ => None
        }).await;

        unique_id.free();

        result?;

        Ok(())
    }

    /// unsubscribe from a topic
    /// 
    /// waits until the unsuback has bee received
    pub async fn unsubscribe(&self, topic: &str) -> Result<(), MqttError> {
        let unique_id = UniqueID::new();
        let subscriber = self.state.subscribe_events()?;

        self.state.unsubscribe(&[topic], unique_id).await;

        Self::await_event(subscriber, |event | match event {
            MqttEvent::UnsubscribeDone(id) if id == unique_id => Some(()),
            _ => None
        }).await;

        unique_id.free();

        Ok(())
    }

    /// send a disconnect packet to the broker
    pub fn disconnect(&self) {
        self.state.disconnect();
    }

    /// Subscribe to received publishes
    pub fn subscribe_received_publishes(&self) -> Result<DynSubscriber<'_, ReceivedPublish<BUFFER, TOPIC>>, MqttError> {
        self.state.subscribe_received_publishes()
    }

}
