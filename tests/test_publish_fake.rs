

use std::{cell::RefCell, pin::Pin};

use network::{fake::{new_connection, ClientConnection, ConnectionRessources, ReadAtomic, ServerConnection}, mqtt::WriteMqttPacket};
use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use embedded_io_async::Read;
use mqttrs::{decode_slice, Connack, ConnectReturnCode, Packet, PacketType, QoS};
use embedded_mqtt::{client::MqttClient, io::MqttEventLoop, ClientConfig, ClientCredentials};
use heapless::Vec;

struct Test <'a, const N: usize> {
    server: ServerConnection<'a, N>,
    event_loop: MqttEventLoop<CriticalSectionRawMutex, N>,
    client: RefCell<Option<ClientConnection<'a, N>>>,
}

impl <'a, const N: usize> Test<'a, N> {

    fn create(client_id: &str, credentials: Option<ClientCredentials>, resources: &'a ConnectionRessources<N>) -> Self {
        let mut config = ClientConfig {
            client_id: heapless::String::new(), 
            credentials, 
            auto_subscribes: Vec::new()
        };

        config.client_id.push_str(client_id).unwrap();

        let (client, server) = new_connection(resources);
        let event_loop = MqttEventLoop::<CriticalSectionRawMutex, N>::new(config);

        Self {
            server,
            event_loop,
            client: RefCell::new(Some(client))
        }
    }

    fn create_client(&'a self) -> MqttClient<'a, CriticalSectionRawMutex> {
        self.event_loop.client()
    }

    async fn read_packet<O, R>(&self, o: O) -> R where O: Fn(&Packet<'_>) -> R{
        self.server.read_mqtt_packet(o).await.unwrap()
    }

    async fn write_packet(&self, packet: Packet<'_>) {
        self.server.write_mqtt_packet(&packet).await.unwrap()
    }


    async fn run(&self) {
        let mut connection = self.client.borrow_mut().take().unwrap();
        let connection = Pin::new(&mut connection);
        self.event_loop.run(connection).await.unwrap();
    }

}


#[tokio::test]
async fn test_connect() {
    let resources = ConnectionRessources::<256>::new();
    let (mut client, mut server) = new_connection(&resources);
    let client = Pin::new(&mut client);

    let mut client_id = heapless::String::new();
    client_id.push_str("1234567890").unwrap();
    let config = ClientConfig{
        client_id,
        credentials: None,
        auto_subscribes: Vec::new()
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



#[tokio::test]
#[ntest::timeout(1000)]
async fn test_publish() {
    let resources = ConnectionRessources::<256>::new();
    
    let test = Test::create("1234567890", None, &resources);

    let client = test.create_client();

    let work_future = test.run();

    let client_future = async {
        client.publish("topic", "a test payload".as_bytes(), QoS::AtMostOnce, false).await.unwrap();
        client.disconnect().await;
    };


    let server_future = async {

        test.read_packet(|p| {
            assert_eq!(p.get_type(), PacketType::Connect);
        }).await;

        test.write_packet(Packet::Connack(Connack{
            session_present: false,
            code: ConnectReturnCode::Accepted
        })).await;

        test.read_packet(|p|{
            assert_eq!(p.get_type(), PacketType::Publish);
        }).await;
    };

    tokio::join! {
        server_future,
        client_future,
        work_future
    };
}

