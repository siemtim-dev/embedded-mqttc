use core::{cell::RefCell, future::Future};

use buffer::{new_stack_buffer, Buffer, BufferReader, BufferWriter};
use embassy_futures::select::{select, select3};
use embassy_sync::{blocking_mutex::raw::RawMutex, channel::Channel, pubsub::PubSubChannel};
use mqttrs::{decode_slice_with_len, QoS};
use crate::{client::MqttClient, network::{Network, NetworkConnection}, state::State, time, ClientConfig, MqttError, MqttEvent, MqttPublish, MqttRequest};

use crate::time::Duration;

pub trait AsyncSender<T> {
    fn send(&self, item: T) -> impl Future<Output = ()>;
    fn try_send(&self, item: T) -> Result<(), T>;
}

impl <M: RawMutex, T, const N: usize> AsyncSender<T> for Channel<M, T, N> {
    fn send(&self, item: T) -> impl Future<Output = ()> {
        self.send(item)
    }

    fn try_send(&self, item: T) -> Result<(), T> {
        self.try_send(item)
            .map_err(|err|{
                match err {
                    embassy_sync::channel::TrySendError::Full(item) => item,
                }
            })
    }
}

impl <M: RawMutex, T: Clone, const CAP: usize, const SUBS: usize, const PUBS: usize> AsyncSender<T> for PubSubChannel<M, T, CAP, SUBS, PUBS> {
    async fn send(&self, item: T) {
        self.publisher().unwrap().publish(item).await;
    }

    fn try_send(&self, item: T) -> Result<(), T> {
        self.publisher().unwrap().try_publish(item)
    }
}

pub trait AsyncReceiver<T> {
    fn receive(&self) -> impl Future<Output = T>;
}

impl <M: RawMutex, T, const N: usize> AsyncReceiver<T> for Channel<M, T, N> {
    fn receive(&self) -> impl Future<Output = T> {
        self.receive()
    }
}

pub struct MqttEventLoop<M: RawMutex, N: NetworkConnection, const B: usize> {
    recv_buffer: RefCell<Buffer<[u8; B]>>,
    send_buffer: RefCell<Buffer<[u8; B]>>,

    connection: RefCell<Network<N>>,

    state: State<M>,

    control_sender: PubSubChannel<M, MqttEvent, 4, 16, 8>,
    request_receiver: Channel<M, MqttRequest, 4>,
    received_publishes: Channel<M, MqttPublish, 4>
}

impl <M: RawMutex, N: NetworkConnection, const B: usize> MqttEventLoop<M, N, B> {

    pub fn new(connection: N, config: ClientConfig) -> Self {
        
        Self {
            recv_buffer: RefCell::new(new_stack_buffer::<B>()),
            send_buffer: RefCell::new(new_stack_buffer::<B>()),
            connection: RefCell::new(Network::new(connection)),

            state: State::new(config),

            control_sender: PubSubChannel::new(),
            request_receiver: Channel::new(),
            received_publishes: Channel::new()
        }
    }

    pub fn client<'a>(&'a self) -> MqttClient<'a, M> {
        MqttClient{
            control_reveiver: &self.control_sender,
            request_sender: self.request_receiver.sender(),
            received_publishes: self.received_publishes.receiver()
        }
    }

    /// Receive / sent bytes from / to the network
    /// Sends blocking if there are bytes to send
    /// Receives blocking if there are no more bytes to send.
    async fn network_send_receive(&self) -> Result<(), MqttError> {
        let mut send_buffer = self.send_buffer.borrow_mut();
        let mut connection = self.connection.borrow_mut();
        
        if send_buffer.has_remaining_len() {
            connection.send(&mut send_buffer).await?;
        } else {
            trace!("send buffer is empty, skipping send");
        }

        let mut recv_buffer = self.recv_buffer.borrow_mut();
        // Do not block for receiving if there is still something to send
        if send_buffer.has_remaining_len() {
            connection.try_receive(&mut recv_buffer).await?;
        } else {
            connection.receive(&mut recv_buffer).await?;
        }

        Ok(())
    }

    /// Try to read a packet from recv buffer. 
    async fn try_package_receive(&self, send_buffer: &mut impl BufferWriter, recv_buffer: impl BufferReader) -> Result<(), MqttError> {
        if recv_buffer.is_empty() {
            return Ok(())
        }
        
        let packet_op = decode_slice_with_len(&recv_buffer[..])
            .map_err(|e| {
                error!("error decoding package: {}", e);
                MqttError::CodecError
            })?;
        
        if let Some((len, packet)) = packet_op {
            recv_buffer.add_bytes_read(len);
            let event_option = 
                self.state.process_packet(&packet, send_buffer, &self.received_publishes).await?;
            if let Some(event) = event_option {
                self.control_sender.publisher().unwrap().publish(event).await;
            }
        }

        Ok(())
    }

    /// Makes the receive / send of the network
    /// First tries to write outgoing traffic to buffer
    /// Then tries to read / write to / from the connection
    /// Then read data from receive buffer
    async fn work_network(&self) -> Result<!, MqttError> {
        loop {
            // Try to send packets first before blocking for network traffic
            // Send packets (Ping, Connect, Publish)
            {
                let mut send_buffer = self.send_buffer.borrow_mut();
                let mut send_buffer_writer = send_buffer.create_writer();
                self.state.send_packets(&mut send_buffer_writer, &self.control_sender)?;
            }
            
            // Send / Receive Network traffic
            // Interript this when ...
            // - a new Request (e. g. Publish) is added to process it
            // - sending a ping message is required
            let network_future = self.network_send_receive();
            let on_request_signal_future = self.state.on_requst_added.wait();
            let next_ping_future = self.state.on_ping_required();
            match select3(network_future, on_request_signal_future, next_ping_future).await {
                embassy_futures::select::Either3::First(res) => res,
                embassy_futures::select::Either3::Second(_) => Ok(()),
                embassy_futures::select::Either3::Third(_) => Ok(()),
            }?;

            let mut recv_buffer = self.recv_buffer.borrow_mut();
            let recv_reader = recv_buffer.create_reader();
            let mut send_buffer = self.send_buffer.borrow_mut();
            let mut send_buffer_writer = send_buffer.create_writer();

            // Try to read a package from the receive buffer and write answers (e. g. acknoledgements) 
            // to the send buffer
            self.try_package_receive(&mut send_buffer_writer, recv_reader).await?;
        }
    }

    async fn work_request_receive(&self) -> Result<!, MqttError> {
        loop {
            let req = self.request_receiver.receive().await;
            let pid = self.state.pid_source.next_pid();

            match req {
                MqttRequest::Publish(mqtt_publish, id) => {
                    self.state.publishes.push_publish(mqtt_publish, id, pid).await;
                },
                MqttRequest::Subscribe(topic, unique_id) => {
                    const SUBSCRIBE_QOS: QoS = QoS::AtMostOnce;
                    self.state.subscribes.push_subscribe(topic, pid, unique_id, SUBSCRIBE_QOS).await;
                },
                MqttRequest::Unsubscribe(topic, unique_id) => {
                    self.state.subscribes.push_unsubscribe(topic, pid, unique_id).await;
                },
            }

            // Signal that a new request is added
            self.state.on_requst_added.signal(0);
        }
    }

    async fn connect(&self) -> Result<(), MqttError> {
        let mut tries = 0;

        while tries < 5 {

            let result = self.connection.borrow_mut().connect().await;
            if result.is_err() {
                tries += 1;
                warn!("{}. try to connecto to host failed", tries);
                time::sleep(Duration::from_secs(3)).await;
            } else {
                return Ok(())
            }
            
        }

        error!("{} tries to connect failed", tries);
        Err(MqttError::ConnectionFailed)
    }

    async fn work(&self) -> Result<!, MqttError> {
        // Reset state on new connection
        self.state.reset();
            
        // Poll both futures
        // Select should never befinished because both jobs are infinite
        let network_future = self.work_network();
        let request_future = self.work_request_receive();
        match select(network_future, request_future).await {
            embassy_futures::select::Either::First(net_result) => {
                let err = net_result.unwrap_err();
                error!("network infinite job finished: {}", err);
                Err(err)
            },
            embassy_futures::select::Either::Second(req_result) => {
                let err = req_result.unwrap_err();
                error!("infinite request receive job finished: {}", err);
                Err(err)
            },
        }
    }

    pub async fn run(&self) -> Result<!, MqttError> {

        self.connect().await?;

        loop {
            let err = self.work().await.unwrap_err();
            if err == MqttError::ConnectionFailed {
                self.connect().await?;
            } else {
                return Err(err);
            }
        }

    }
}

#[cfg(test)]
mod test {
    use crate::misc::MqttPacketReader;
    use crate::state::KEEP_ALIVE;
    use crate::time::Duration;

    use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
    use heapless::String;
    use mqttrs::{Connack, ConnectReturnCode, Packet, PacketType, QoS, Suback, SubscribeReturnCodes};
    use crate::time;

    use crate::{network::fake::{self, ConnectionRessources, ReadAtomic, WriteMqttPacket}, ClientConfig};

    use super::MqttEventLoop;

    fn print_packet(p: &Packet<'_>) -> std::string::String {
        match p {
            Packet::Connect(_) => format!("Connect"),
            Packet::Connack(_) => format!("Connack"),
            Packet::Publish(_) => format!("Publish"),
            Packet::Puback(_) => format!("Puback"),
            Packet::Pubrec(_) => format!("Pubrec"),
            Packet::Pubrel(_) => format!("Pubrel"),
            Packet::Pubcomp(_) => format!("Pubcomp"),
            Packet::Subscribe(_) => format!("Subscribe"),
            Packet::Suback(_) => format!("Suback"),
            Packet::Unsubscribe(_) => format!("Unsubscribe"),
            Packet::Unsuback(_) => format!("Unsuback"),
            Packet::Pingreq => format!("Pingreq"),
            Packet::Pingresp => format!("Pingresp"),
            Packet::Disconnect => format!("Disconnect"),
        }
    }

    #[tokio::test]
    async fn test_run() {
        time::test_time::set_default();

        let mut config = ClientConfig{
            client_id: String::new(),
            credentials: None
        };

        config.client_id.push_str("asjdkaljs").unwrap();

        let connection_resources = ConnectionRessources::<1024>::new();

        let (client, server) = fake::new_connection(&connection_resources);

        let event_loop = MqttEventLoop::<CriticalSectionRawMutex, _, 1024>::new(client, config);
        let mqtt_client = event_loop.client();

        let runner_future = async {
            event_loop.run().await.unwrap();
        };

        let test_future = async move {
            let client_future = async {
                mqtt_client.subscribe("test").await.unwrap();
            };
            
            let server_future = async {

                let connect = server.read_mqtt_packet(|p| p.get_type()).await.unwrap();
                assert_eq!(connect, PacketType::Connect);

                server.write_mqtt_packet(&Packet::Connack(Connack{
                    session_present: false,
                    code: ConnectReturnCode::Accepted
                })).await.unwrap();

                let subscribe = server.read_mqtt_packet(|s| {
                    match s {
                        Packet::Subscribe(sub) => sub.clone(),
                        other => panic!("expected subscribe, got {}", print_packet(other))
                    }
                }).await.unwrap();

                let mut return_codes = heapless::Vec::new();
                return_codes.push(SubscribeReturnCodes::Success(QoS::AtLeastOnce)).unwrap();
                
                server.write_mqtt_packet(&Packet::Suback(Suback{
                    pid: subscribe.pid,
                    return_codes 
                })).await.unwrap();
            };

            tokio::join!(client_future, server_future);
        };

        tokio::select! {
            _ = runner_future => {},
            _ = test_future => {}
        }
    }

    #[tokio::test]
    async fn test_idle_connection() {
        let config = ClientConfig{
            client_id: String::new(),
            credentials: None
        };

        time::test_time::set_static_now();

        let connection_resources = ConnectionRessources::<1024>::new();
        let (client, server) = fake::new_connection(&connection_resources);

        let event_loop = MqttEventLoop::<CriticalSectionRawMutex, _, 1024>::new(client, config);

        let runner_future = async {
            event_loop.run().await.unwrap();
        };
            
        let server_future = async {

            let connect = server.read_mqtt_packet(|p| p.get_type()).await.unwrap();
            assert_eq!(connect, PacketType::Connect);

            server.write_mqtt_packet(&Packet::Connack(Connack{
                session_present: false,
                code: ConnectReturnCode::Accepted
            })).await.unwrap();

            time::test_time::advance_time(Duration::from_secs(2));
            tokio::time::sleep(core::time::Duration::from_millis(100)).await;

            let pingreq = server.with_reader(|reader| reader.read_packet().unwrap().map(|p| p.get_type()));
            assert_eq!(pingreq, None);

            time::test_time::advance_time(Duration::from_secs(KEEP_ALIVE as u64) / 2);
            tokio::time::sleep(core::time::Duration::from_millis(100)).await;

            let pingreq = server.with_reader(|reader| reader.read_packet().unwrap().map(|p| p.get_type()));
            assert_eq!(pingreq, Some(PacketType::Pingreq));

            server.write_mqtt_packet(&Packet::Pingresp).await.unwrap();

            time::test_time::advance_time(Duration::from_secs(2));
            tokio::time::sleep(core::time::Duration::from_millis(100)).await;

            let pingreq = server.with_reader(|reader| reader.read_packet().unwrap().map(|p| p.get_type()));
            assert_eq!(pingreq, None);
        };

        tokio::select! {
            _ = runner_future => {},
            _ = server_future => {}
        }
    }

}

