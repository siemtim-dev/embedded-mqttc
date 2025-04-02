use std::{future::Future, pin::Pin};

use embassy_mqtt::{io::MqttEventLoop, ClientConfig};
use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use mqttest::Mqttest;
use mqttrs::QoS;
use network::std::StdNetworkConnection;
use tokio::runtime::Builder;
use tracing_log::LogTracer;

fn block_on<T>(f: impl Future<Output = T>) -> T {
    LogTracer::init().unwrap();
    Builder::new_current_thread().enable_all().build().unwrap().block_on(f)
}

#[test_log::test]
fn test_publish() {

    block_on(async {
        let server_conf = mqttest::Conf::new().max_connect(1);
        let srv = Mqttest::start(server_conf).await.expect("Failed listen");

        let client_config = ClientConfig::new("23bskjd82", None);

        let event_loop = MqttEventLoop::<CriticalSectionRawMutex, 1024>::new(client_config);
        let client = event_loop.client();

        let client_loop_future = async {
            let mut connection = StdNetworkConnection::new(("127.0.0.1", srv.port));
            let connection = Pin::new(&mut connection);
            event_loop.run(connection).await.unwrap();
        };

        let client_future = async {
            client.publish("test/topic", "i am a test".as_bytes(), QoS::AtLeastOnce, false).await.unwrap();
            client.disconnect().await;
        };

        tokio::join!(client_loop_future, client_future);
    });

}