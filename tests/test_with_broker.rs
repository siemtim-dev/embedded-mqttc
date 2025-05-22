

use std::{env::{self, VarError}, fmt::Debug, pin::Pin, str::{from_utf8, FromStr}};
use embedded_mqtt::network::std::StdNetworkConnection;
use embedded_mqtt::{io::MqttEventLoop, ClientConfig, ClientCredentials, MqttEvent};
use embassy_sync::{blocking_mutex::raw::CriticalSectionRawMutex, signal::Signal};
use mqttrs::QoS;

use test_log::test;

mod broker_common;
use broker_common::{create_sinple_client, random_topic};
use uuid::Uuid;

const MQTT_DEFAULT_PORT: u16 = 1883;

struct BrokerConfig {
    host: String,
    port: Option<u16>,
    username: Option<String>,
    password: Option<String>
}

impl BrokerConfig {
    fn from_env() -> Self {
        let host = Self::lookup_env("MQTT_HOST")
            .expect("MQTT_HOST must be present");

        let port = Self::lookup_env::<u16>("MQTT_PORT");
        let username = Self::lookup_env("MQTT_USER");
        let password = Self::lookup_env("MQTT_PASSWORD");

        Self {
            host, port, username, password
        }
    }

    fn lookup_env<T>(name: &str) -> Option<T> where T: FromStr, T::Err: Debug {
        let env = match env::var(name) {
            Ok(value) => Some(value),
            Err(VarError::NotPresent) => None,
            Err(e) => panic!("cannot read env {}: {}", name, e)
        };

        env.map(|env_str| env_str.parse().expect("failed to parse env value"))
    }

    fn new_connection(&self) -> StdNetworkConnection<(String, u16)> {
        let connection_params = match self.port {
            Some(port) => (self.host.clone(), port),
            None => (self.host.clone(), MQTT_DEFAULT_PORT),
        };

        StdNetworkConnection::new(connection_params)
    }

    fn new_client_config(&self, client_id: &str) -> ClientConfig {
        let credentials = match &self.username {
            Some(username) => {
                let password = self.password.as_ref().unwrap();
                Some(ClientCredentials::new(username, password))
            },
            None => None,
        };

        ClientConfig::new(client_id, credentials)
    }

    fn new_client_config_with_auto_subscribe<'a>(&self, client_id: &str, auto_subscribes: impl Iterator<Item = &'a str>, qos: QoS) -> ClientConfig {
        let credentials = match &self.username {
            Some(username) => {
                let password = self.password.as_ref().unwrap();
                Some(ClientCredentials::new(username, password))
            },
            None => None,
        };

        ClientConfig::new_with_auto_subscribes(client_id, credentials, auto_subscribes, qos)
    }

    fn unwrap_port(&self) -> u16 {
        self.port.unwrap_or(MQTT_DEFAULT_PORT)
    }
}

#[test(tokio::test)]
#[ntest::timeout(10000)]
#[cfg_attr(not(feature = "test_with_broker"), ignore = "broker test skipped")]
async fn test_broker_publish_qos0() {
    dotenvy::dotenv().ok();

    let broker_config = BrokerConfig::from_env();
    
    let client_id = Uuid::new_v4().to_string();
    let mqtt_config = broker_config.new_client_config(&client_id);
    let event_loop = 
        MqttEventLoop::<CriticalSectionRawMutex, 1024>::new(mqtt_config);

    let client = event_loop.client();

    let client_loop_future = async {
        let mut connection = broker_config.new_connection();
        let connection = Pin::new(&mut connection);
        event_loop.run(connection).await.unwrap();
    };

    let test_future = async {
        let topic = random_topic(None);
        let payload = "test payload".as_bytes();

        client.publish(&topic, payload, QoS::AtMostOnce, false).await.unwrap();
    };

    tokio::select! {
        _ = client_loop_future => {
            panic!("client loop must not stop");
        },
        _ = test_future => {}
    }
}

#[test(tokio::test)]
#[ntest::timeout(10000)]
#[cfg_attr(not(feature = "test_with_broker"), ignore = "broker test skipped")]
async fn test_broker_publish_qos1() {
    dotenvy::dotenv().ok();

    let broker_config = BrokerConfig::from_env();
    
    let client_id = Uuid::new_v4().to_string();
    let mqtt_config = broker_config.new_client_config(&client_id);
    let event_loop = 
        MqttEventLoop::<CriticalSectionRawMutex, 1024>::new(mqtt_config);

    let client = event_loop.client();

    let client_loop_future = async {
        let mut connection = broker_config.new_connection();
        let connection = Pin::new(&mut connection);
        event_loop.run(connection).await.unwrap();
    };

    let test_future = async {
        let topic = random_topic(None);
        let payload = "test payload".as_bytes();

        client.publish(&topic, payload, QoS::AtLeastOnce, false).await.unwrap();
    };

    tokio::select! {
        _ = client_loop_future => {
            panic!("client loop must not stop");
        },
        _ = test_future => {}
    }
}

#[test(tokio::test)]
#[ntest::timeout(10000)]
#[cfg_attr(not(feature = "test_with_broker"), ignore = "broker test skipped")]
async fn test_broker_publish_qos2() {
    dotenvy::dotenv().ok();

    let broker_config = BrokerConfig::from_env();
    
    let client_id = Uuid::new_v4().to_string();
    let mqtt_config = broker_config.new_client_config(&client_id);
    let event_loop = 
        MqttEventLoop::<CriticalSectionRawMutex, 1024>::new(mqtt_config);

    let client = event_loop.client();

    let client_loop_future = async {
        let mut connection = broker_config.new_connection();
        let connection = Pin::new(&mut connection);
        event_loop.run(connection).await.unwrap();
    };

    let test_future = async {
        let topic = random_topic(None);
        let payload = "test payload".as_bytes();

        client.publish(&topic, payload, QoS::ExactlyOnce, false).await.unwrap();
    };

    tokio::select! {
        _ = client_loop_future => {
            panic!("client loop must not stop");
        },
        _ = test_future => {}
    }
}

#[test(tokio::test)]
#[ntest::timeout(10000)]
#[cfg_attr(not(feature = "test_with_broker"), ignore = "broker test skipped")]
async fn test_broker_subscribe_unsubscribe() {
    dotenvy::dotenv().ok();

    let broker_config = BrokerConfig::from_env();
    
    let client_id = Uuid::new_v4().to_string();
    let mqtt_config = broker_config.new_client_config(&client_id);
    let event_loop = 
        MqttEventLoop::<CriticalSectionRawMutex, 1024>::new(mqtt_config);

    let client = event_loop.client();

    let client_loop_future = async {
        let mut connection = broker_config.new_connection();
        let connection = Pin::new(&mut connection);
        event_loop.run(connection).await.unwrap();
    };

    let test_future = async {
        let topic = random_topic(None);

        client.subscribe(&topic).await.unwrap();
        client.unsubscribe(&topic).await.unwrap();
        client.disconnect().await;
    };

    tokio::join!(
        client_loop_future,
        test_future,
    );

}

#[test(tokio::test)]
#[ntest::timeout(10000)]
#[cfg_attr(not(feature = "test_with_broker"), ignore = "broker test skipped")]
async fn test_broker_subscribe() {
    dotenvy::dotenv().ok();

    let subscribe_ready_signal = Signal::<CriticalSectionRawMutex, usize>::new();

    let broker_config = BrokerConfig::from_env();

    let topic = random_topic(None);
    
    let (client, _, cancel_token) = create_sinple_client(&broker_config);

    let publish_future = async {
        let payload = "test-payload-hjh3".as_bytes();

        if ! subscribe_ready_signal.signaled() {
            tracing::trace!("TEST: wait for subscribe");
            subscribe_ready_signal.wait().await; // Wait until the receiver side has subscribed
            tracing::trace!("TEST: received signal, subscription done")
        }
        
        client.publish(&topic, rumqttc::QoS::AtLeastOnce, false, payload).await.unwrap();
        client.disconnect().await.unwrap();
        tracing::trace!("TEST: publish_future done");
    };

    let client_id = Uuid::new_v4().to_string();
    let mqtt_config = broker_config.new_client_config(&client_id);
    let event_loop = 
        MqttEventLoop::<CriticalSectionRawMutex, 1024>::new(mqtt_config);

    let client = event_loop.client();

    let client_loop_future = async {
        let mut connection = broker_config.new_connection();
        let connection = Pin::new(&mut connection);
        event_loop.run(connection).await.unwrap();

        tracing::trace!("TEST: client_loop_future done");
    };

    let subscribe_future = async {
        client.subscribe(&topic).await.unwrap();

        tracing::trace!("TEST: signal publish_future to continue");
        subscribe_ready_signal.signal(0); // Signal the sender side that the subscribe is done

        let publish = client.receive().await;
        assert_eq!(&publish.topic[..], &topic[..]);
        let payload = from_utf8(publish.payload.data()).unwrap();
        assert_eq!(payload, "test-payload-hjh3");
        client.disconnect().await;

        tracing::trace!("TEST: subscribe_future done");
    };

    tokio::join!(
        client_loop_future,
        publish_future,
        subscribe_future
    );

    cancel_token.cancel();
}

#[test(tokio::test)]
#[ntest::timeout(10000)]
#[cfg_attr(not(feature = "test_with_broker"), ignore = "broker test skipped")]
async fn test_broker_publish() {
    dotenvy::dotenv().ok();

    let broker_config = BrokerConfig::from_env();
    
    let client_id = Uuid::new_v4().to_string();
    let mqtt_config = broker_config.new_client_config(&client_id);
    let event_loop = 
        MqttEventLoop::<CriticalSectionRawMutex, 1024>::new(mqtt_config);

    let client = event_loop.client();

    let client_loop_future = async {
        let mut connection = broker_config.new_connection();
        let connection = Pin::new(&mut connection);
        event_loop.run(connection).await.unwrap();
        tracing::trace!("TEST: client_loop_future done");
    };

    let topic = random_topic(None);

    let publish_future = async {
        let payload = "test-payload-hjh3".as_bytes();

        tokio::time::sleep(core::time::Duration::from_millis(500)).await;
        client.publish(&topic, payload, QoS::AtLeastOnce, false).await.unwrap();
        client.disconnect().await;
        tracing::trace!("TEST: publish_future done");
    };

    let (client, mut receiver, cancel_token) = create_sinple_client(&broker_config);
    let subscribe_future = async {
        client.subscribe(&topic, rumqttc::QoS::AtLeastOnce).await.unwrap();
        let publish = receiver.recv().await.expect("there must be a publish");
        assert_eq!(publish.topic, &topic[..]);
        let payload_str = std::str::from_utf8(&publish.payload).unwrap();
        assert_eq!(payload_str, "test-payload-hjh3");
        client.disconnect().await.unwrap();
        tracing::trace!("TEST: subscribe_future done");
    };

    tokio::join! (
        client_loop_future,
        publish_future,
        subscribe_future
    );

    cancel_token.cancel();
}


#[test(tokio::test)]
#[ntest::timeout(20000)]
#[cfg_attr(not(feature = "test_with_broker"), ignore = "broker test skipped")]
async fn test_auto_subscribe() {
    dotenvy::dotenv().ok();

    let broker_config = BrokerConfig::from_env();

    let auto_subscribe_topics = [
        random_topic(Some("test-autosub-1")), 
        random_topic(Some("test-autosub-2")), 
    ];

    let topic = auto_subscribe_topics.first().unwrap();
    
    let client_id = Uuid::new_v4().to_string();
    let mqtt_config = broker_config.new_client_config_with_auto_subscribe(
        &client_id,
        auto_subscribe_topics.iter().map(|s| &s[..]),
        QoS::AtLeastOnce
    );
    let event_loop = 
        MqttEventLoop::<CriticalSectionRawMutex, 1024>::new(mqtt_config);

    let client = event_loop.client();

    let client_loop_future = async {
        let mut connection = broker_config.new_connection();
        let connection = Pin::new(&mut connection);
        event_loop.run(connection).await.unwrap();
    };

    let receive_message_future = client.receive();
    let subscribe_future = async {
        let publish = receive_message_future.await;
        assert_eq!(&publish.topic, &topic[..]);
        let payload = from_utf8(publish.payload.data()).unwrap();
        assert_eq!(payload, "test-payload-hjhasdas3");
        client.disconnect().await;
    };

    let (publish_client, _, cancel_token) = create_sinple_client(&broker_config);

    let initial_auto_subscribe_success_future = client.on(|event| *event == MqttEvent::InitialSubscribesDone);
    let publish_future = async {
        initial_auto_subscribe_success_future.await;
        tracing::debug!("TEST: publish: stop waiting, initial subscribes done");

        publish_client.publish(topic, rumqttc::QoS::AtLeastOnce, false, "test-payload-hjhasdas3")
            .await.unwrap();

        if let Err(e) = publish_client.disconnect().await {
            tracing::error!("error disconnecting from broker: {}", e);
        }

        publish_client.disconnect().await.unwrap();
    };

    tokio::join! (
        client_loop_future,
        publish_future,
        subscribe_future
    );

    cancel_token.cancel();
}



