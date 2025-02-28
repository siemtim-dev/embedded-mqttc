

use std::pin::Pin;

use network::fake::{new_connection, ConnectionRessources};
use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use embedded_io_async::Read;
use mqttrs::{decode_slice, Packet};
use embassy_mqtt::{io::MqttEventLoop, ClientConfig};


#[tokio::test]
async fn test_connect() {
    let resources = ConnectionRessources::<256>::new();
    let (mut client, mut server) = new_connection(&resources);
    let client = Pin::new(&mut client);

    let mut client_id = heapless::String::new();
    client_id.push_str("1234567890").unwrap();
    let config = ClientConfig{
        client_id,
        credentials: None
    };

    let event_loop = MqttEventLoop::<CriticalSectionRawMutex, 1024>::new(config);

    let work_future = async {
        event_loop.run(client).await.unwrap();
    };

    let _client = event_loop.client();


    let server_future = async {

        let mut buf = [0; 256];
        server.read(&mut buf[..]).await.unwrap();

        let packet = decode_slice(&buf).unwrap().expect("there must be a connect packet");

        if let Packet::Connect(c) = packet {
            assert_eq!(c.client_id, "1234567890");
            assert_eq!(c.password, None);
            assert_eq!(c.username, None);
        } else {
            panic!("first packet must be a connect");
        }
    };

    tokio::select! {
        _ = server_future => {},
        _ = work_future => {}
    }

}
