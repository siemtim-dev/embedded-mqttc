

use std::{env::{self, VarError}, fmt::Debug, pin::Pin, str::{from_utf8, FromStr}, time::Duration};
use network::std::StdNetworkConnection;
use embassy_mqtt::{io::MqttEventLoop, ClientConfig, ClientCredentials};
use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use mqttrs::QoS;

use test_log::test;

mod broker_common;
use broker_common::create_sinple_client;

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
#[ntest::timeout(3000)]
#[cfg_attr(not(feature = "test_with_broker"), ignore = "broker test skipped")]
async fn test_broker_publish_qos0() {
    dotenv::dotenv().ok();

    let broker_config = BrokerConfig::from_env();
    
    let mqtt_config = broker_config.new_client_config("ff9h01238chhz3999hf");
    let event_loop = 
        MqttEventLoop::<CriticalSectionRawMutex, 1024>::new(mqtt_config);

    let client = event_loop.client();

    let client_loop_future = async {
        let mut connection = broker_config.new_connection();
        let connection = Pin::new(&mut connection);
        event_loop.run(connection).await.unwrap();
    };

    let test_future = async {
        let topic = "test-hsduifhds";
        let payload = "test payload".as_bytes();

        client.publish(topic, payload, QoS::AtMostOnce, false).await.unwrap();
    };

    tokio::select! {
        _ = client_loop_future => {
            panic!("client loop must not stop");
        },
        _ = test_future => {}
    }
}

#[test(tokio::test)]
#[ntest::timeout(3000)]
#[cfg_attr(not(feature = "test_with_broker"), ignore = "broker test skipped")]
async fn test_broker_publish_qos1() {
    dotenv::dotenv().ok();

    let broker_config = BrokerConfig::from_env();
    
    let mqtt_config = broker_config.new_client_config("gzug83gh30ugd");
    let event_loop = 
        MqttEventLoop::<CriticalSectionRawMutex, 1024>::new(mqtt_config);

    let client = event_loop.client();

    let client_loop_future = async {
        let mut connection = broker_config.new_connection();
        let connection = Pin::new(&mut connection);
        event_loop.run(connection).await.unwrap();
    };

    let test_future = async {
        let topic = "test-ashjhkasjhdj";
        let payload = "test payload".as_bytes();

        client.publish(topic, payload, QoS::AtLeastOnce, false).await.unwrap();
    };

    tokio::select! {
        _ = client_loop_future => {
            panic!("client loop must not stop");
        },
        _ = test_future => {}
    }
}

#[test(tokio::test)]
#[ntest::timeout(3000)]
#[cfg_attr(not(feature = "test_with_broker"), ignore = "broker test skipped")]
async fn test_broker_publish_qos2() {
    dotenv::dotenv().ok();

    let broker_config = BrokerConfig::from_env();
    
    let mqtt_config = broker_config.new_client_config("dhk3a09udgwp2ih");
    let event_loop = 
        MqttEventLoop::<CriticalSectionRawMutex, 1024>::new(mqtt_config);

    let client = event_loop.client();

    let client_loop_future = async {
        let mut connection = broker_config.new_connection();
        let connection = Pin::new(&mut connection);
        event_loop.run(connection).await.unwrap();
    };

    let test_future = async {
        let topic = "test-hsdjkfhsdhf";
        let payload = "test payload".as_bytes();

        client.publish(topic, payload, QoS::ExactlyOnce, false).await.unwrap();
    };

    tokio::select! {
        _ = client_loop_future => {
            panic!("client loop must not stop");
        },
        _ = test_future => {}
    }
}

#[test(tokio::test)]
#[ntest::timeout(5000)]
#[cfg_attr(not(feature = "test_with_broker"), ignore = "broker test skipped")]
async fn test_broker_subscribe_unsubscribe() {
    dotenv::dotenv().ok();

    let broker_config = BrokerConfig::from_env();
    
    let mqtt_config = broker_config.new_client_config("jh330u9887609efhpw22");
    let event_loop = 
        MqttEventLoop::<CriticalSectionRawMutex, 1024>::new(mqtt_config);

    let client = event_loop.client();

    let client_loop_future = async {
        let mut connection = broker_config.new_connection();
        let connection = Pin::new(&mut connection);
        event_loop.run(connection).await.unwrap();
    };

    let test_future = async {
        let topic = "test-jdfoifu98z";

        client.subscribe(topic).await.unwrap();
        client.unsubscribe(topic).await.unwrap();
        client.disconnect().await;
    };

    tokio::join!(
        client_loop_future,
        test_future,
    );

}

#[test(tokio::test)]
#[ntest::timeout(3000)]
#[cfg_attr(not(feature = "test_with_broker"), ignore = "broker test skipped")]
async fn test_broker_publish_and_subscribe() {
    dotenv::dotenv().ok();

    let broker_config = BrokerConfig::from_env();
    
    let mqtt_config = broker_config.new_client_config("jhfvp3330u9efhpw22");
    let event_loop = 
        MqttEventLoop::<CriticalSectionRawMutex, 1024>::new(mqtt_config);

    let client = event_loop.client();

    let client_loop_1_future = async {
        let mut connection = broker_config.new_connection();
        let connection = Pin::new(&mut connection);
        event_loop.run(connection).await.unwrap();
    };

    let publish_future = async {
        let topic = "test";
        let payload = "test-payload-hjh3".as_bytes();

        tokio::time::sleep(core::time::Duration::from_millis(100)).await;
        client.publish(topic, payload, QoS::AtLeastOnce, false).await.unwrap();
        client.disconnect().await;
    };

    let mqtt_config = broker_config.new_client_config("jjl43nn29jk");
    let event_loop = 
        MqttEventLoop::<CriticalSectionRawMutex, 1024>::new(mqtt_config);

    let client = event_loop.client();

    let client_loop_2_future = async {
        let mut connection = broker_config.new_connection();
        let connection = Pin::new(&mut connection);
        event_loop.run(connection).await.unwrap();
    };

    let subscribe_future = async {
        client.subscribe("test").await.unwrap();
        let publish = client.receive().await;
        assert_eq!(&publish.topic, "test");
        let payload = from_utf8(publish.payload.data()).unwrap();
        assert_eq!(payload, "test-payload-hjh3");
        client.disconnect().await;
    };

    tokio::join! (
        client_loop_1_future,
        client_loop_2_future,
        publish_future,
        subscribe_future
    );
}


#[test(tokio::test)]
#[ntest::timeout(6000)]
#[cfg_attr(not(feature = "test_with_broker"), ignore = "broker test skipped")]
async fn test_auto_subscribe() {
    dotenv::dotenv().ok();

    let broker_config = BrokerConfig::from_env();
    let auto_subscribe_topics = [ "test-autosub-1", "test-autosub-2" ];
    
    let (client, _, cancel_token) = create_sinple_client("jjl43nhd74hd7wn29jk", &broker_config);

    let publish_future = async move {
        tokio::time::sleep(Duration::from_millis(500)).await;

        client.publish("test-autosub-1", rumqttc::QoS::AtLeastOnce, false, "test-payload-hjhasdas3")
            .await.unwrap();

        if let Err(e) = client.disconnect().await {
            tracing::error!("error disconnecting from broker: {}", e);
        }

        cancel_token.cancel();
    };
    

    let mqtt_config = broker_config.new_client_config_with_auto_subscribe(
        "jhfvp3330uhcf8sj9efhpw22",
        auto_subscribe_topics.into_iter(),
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

    let subscribe_future = async {
        let publish = client.receive().await;
        assert_eq!(&publish.topic, "test-autosub-1");
        let payload = from_utf8(publish.payload.data()).unwrap();
        assert_eq!(payload, "test-payload-hjhasdas3");
        client.disconnect().await;
    };

    tokio::join! (
        client_loop_future,
        publish_future,
        subscribe_future
    );
}



