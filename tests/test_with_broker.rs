use std::{env::{self, VarError}, fmt::Debug, str::FromStr};

use embassy_mqtt::{io::MqttEventLoop, network::std::StdNetworkConnection, ClientConfig, ClientCredentials};
use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use mqttrs::QoS;

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
}

#[tokio::test]
#[cfg_attr(not(feature = "test_with_broker"), ignore = "broker test skipped")]
async fn test_broker_publish() {

    let broker_config = BrokerConfig::from_env();
    
    let connection = broker_config.new_connection();
    let mqtt_config = broker_config.new_client_config("1234567890");
    let event_loop = 
        MqttEventLoop::<CriticalSectionRawMutex, _, 1024>::new(connection, mqtt_config);

    let client = event_loop.client();

    let client_loop_future = async {
        event_loop.run().await.unwrap()
    };

    let test_future = async {
        let topic = "test";
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
