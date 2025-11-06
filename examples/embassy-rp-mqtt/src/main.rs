#![no_std]
#![no_main]

use defmt::{info, unwrap};
use embassy_executor::Spawner;
use embassy_net::{dns::DnsSocket, tcp::client::{TcpClient, TcpClientState}};
use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use embassy_time::Timer;
use embedded_mqttc::{ClientConfig, ClientCredentials, Host, QoS, client::MqttClient, state::State};
use heapless::Vec;
use static_cell::StaticCell;

use crate::wifi::{WifiParams, WifiPins, wifi_connect};

use {defmt_rtt as _, panic_probe as _};

mod wifi;

const MQTT_HOST: Option<&'static str> = option_env!("MQTT_HOST");
const MQTT_PORT: Option<&'static str> = option_env!("MQTT_PORT");
const MQTT_CLIENT_ID: Option<&'static str> = option_env!("MQTT_CLIENT_ID");
const MQTT_USERNAME: Option<&'static str> = option_env!("MQTT_USERNAME");
const MQTT_PASSWORD: Option<&'static str> = option_env!("MQTT_PASSWORD");

const MQTT_SUBSCIBE_TO: Option<&'static str> = option_env!("MQTT_SUBSCIBE_TO");
const MQTT_PUBLISH_TO: Option<&'static str> = option_env!("MQTT_PUBLISH_TO");

const WIFI_SSID: Option<&'static str> = option_env!("WIFI_SSID");
const WIFI_PASSWORD: Option<&'static str> = option_env!("WIFI_PASSWORD");

#[embassy_executor::main]
async fn main(spawner: Spawner) {
    let p = embassy_rp::init(Default::default());

    let wifi_pins = WifiPins{
        pwr: p.PIN_23,
        cs: p.PIN_25,
        pio: p.PIO0,
        dio: p.PIN_24,
        clk: p.PIN_29,
        dma: p.DMA_CH0
    };

    let wifi_params = WifiParams {
        ssid: WIFI_SSID.unwrap(),
        key: WIFI_PASSWORD
    };

    let network_stack = wifi_connect(wifi_pins, &wifi_params, &spawner).await;

    info!("network up");

    static TCP_CLIENT_STATE: StaticCell<TcpClientState<1, 1024, 1024>> = StaticCell::new();
    let tcp_client_state = TCP_CLIENT_STATE.init(TcpClientState::new());
    static TCP_CLIENT: StaticCell<TcpClient<'static, 1, 1024, 1024>> = StaticCell::new();
    let tcp_client = TCP_CLIENT.init(TcpClient::new(network_stack.clone(), tcp_client_state));

    let dns_client = DnsSocket::new(network_stack.clone());

    let mqtt_credentials = if let Some(username) = MQTT_USERNAME {
        Some(ClientCredentials::new(username, MQTT_PASSWORD.unwrap()))
    } else {
        None
    };

    let mqtt_host = Host::Hostname(MQTT_HOST.unwrap());
    let mqtt_port = MQTT_PORT.map(|port_string| port_string.parse().unwrap());

    let mqtt_client_config = ClientConfig{
        host: mqtt_host,
        port: mqtt_port,
        client_id: MQTT_CLIENT_ID.unwrap(),
        credentials: mqtt_credentials,
        auto_subscribes: Vec::new()
    };

    static MQTT_STATE: StaticCell<StaticMqttState> = StaticCell::new();
    let mqtt_state = MQTT_STATE.init_with(|| StaticMqttState::new(mqtt_client_config, None, tcp_client, dns_client));


    unwrap!( spawner.spawn(run_mqtt_loop(mqtt_state)));
    unwrap!( spawner.spawn(send_data(mqtt_state.new_client())));
    unwrap!( spawner.spawn(receive_data(mqtt_state.new_client())));    
}

type StaticMqttState = State<'static, 'static, CriticalSectionRawMutex, TcpClient<'static, 1, 1024, 1024>, DnsSocket<'static>, 1024, 128, 8>;
type StaticMqttClient = MqttClient<'static, 'static, 'static, CriticalSectionRawMutex, TcpClient<'static, 1, 1024, 1024>, DnsSocket<'static>, 1024, 128, 8>;

#[embassy_executor::task]
async fn receive_data(client: StaticMqttClient) {
    let mut received_publishes = client.subscribe_received_publishes().unwrap();

    info!("starting mqtt receive");

    if let Some(topic) = MQTT_SUBSCIBE_TO {
        info!("subscribe to {}", topic);
        client.subscribe(topic, QoS::AtLeastOnce).await.unwrap();
    }

    loop {
        let received_publish = received_publishes.next_message_pure().await;
        info!("received publish on topic {}", received_publish.topic_name)
    }
}

#[embassy_executor::task]
async fn send_data(client: StaticMqttClient) {
    let topic = MQTT_PUBLISH_TO.unwrap();
    loop {
        info!("publishing to topic");
        client.publish(topic, "i am a test payload".as_bytes(), QoS::AtLeastOnce, true).await.unwrap();
        Timer::after_secs(10).await;
    }

}

#[embassy_executor::task]
async fn run_mqtt_loop(mqtt_state: &'static StaticMqttState) {
    unwrap!(mqtt_state.run().await);
}