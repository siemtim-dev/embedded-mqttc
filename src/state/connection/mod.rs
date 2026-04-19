use core::{cell::RefCell, future::Future, net::SocketAddr, ops::Deref};

use embassy_futures::poll_once;
use embassy_sync::{blocking_mutex::raw::RawMutex, watch::Watch};
use embedded_nal_async::{Dns, TcpConnect};
use embedded_io_async::{Read, Write, Error};
use mqttrs2::{Connack, Connect, LastWill, Packet, Protocol, Suback, Subscribe, decode_slice_with_len, encode_slice};

use crate::{AutoSubscribe, ClientConfig, MqttError, buffer::{MappedBufferRef, StackBufferCell}, fmt::Debug2Format, state::pid::next_pid};

const MQTT_DEFAULT_PORT: u16 = 1883;

macro_rules! network_write {
    ($data:expr, $conn:expr) => {
        $conn.write($data).await
            .map_err(|err| MqttError::ConnectionFailed2(err.kind()))?
    };
}

macro_rules! network_read {
    ($data:expr, $conn:expr) => {
        $conn.read($data).await
            .map_err(|err| MqttError::ConnectionFailed2(err.kind()))?
    };
}

macro_rules! network_flush {
    ($conn:expr) => {
        $conn.flush().await
            .map_err(|err| MqttError::ConnectionFailed2(err.kind()))?
    };
}

fn network_try_read(connection: &mut impl Read, buf: &mut [u8]) -> Result<usize, MqttError> {
    let read_fut = connection.read(buf);
    match poll_once(read_fut) {
        core::task::Poll::Ready(Ok(n)) => {
            trace!("network_try_read: read {} bytes", n);
            Ok(n)
        },
        core::task::Poll::Ready(Err(err)) => {
            trace!("network_try_read err: {}", Debug2Format(&err));
            Err(MqttError::ConnectionFailed2(err.kind()))
        },
        core::task::Poll::Pending => {
            trace!("try_network_read did not read anything");
            Ok(0)
        },
    }
}

#[cfg(test)]
pub mod test;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConnectionStateValue {
    ConnectSent,
    Connected,
    Disconnected,
    Error
}

pub trait ConnectionState {

    /// Establishes the connection to the broker
    fn connect(&self) -> impl Future<Output = Result<(), MqttError>>;

    fn disconnect(&self) -> impl Future<Output = Result<(), MqttError>>;

    fn get_state(&self) -> Option<ConnectionStateValue>;

    fn on_state_change(&self) -> impl Future<Output = ConnectionStateValue>;

    fn await_connected(&self) -> impl Future<Output = ()>;

    fn set_error(&self);

    /// Send a mqtt packet to the broker
    /// This method may return without sending the packet returning Ok(false). This indicates that the send sould be tried later.
    fn try_write_packet(&self, packet: &Packet<'_>) -> Result<bool, MqttError>;

    /// Writes a packet to the send buffer and sends data until teh packet fits
    fn write_packet(&self, packet: &Packet<'_>) -> impl Future<Output = Result<(), MqttError>>;


    /// Run pending io tasks until there is a new packet
    fn run_io(&self) -> impl Future<Output = Result<impl Deref<Target = Packet<'_>>, MqttError>>;

    /// Run until there is a new packet or something has been sent
    fn run_io_nonblocking(&self) -> impl Future<Output = Result<Option<impl Deref<Target = Packet<'_>>>, MqttError>>;
}

pub struct TcpConnectionState<'a, 'l, M: RawMutex, NETWORK, DNS, const BUFFER_SIZE: usize> 
where NETWORK: TcpConnect, DNS: Dns {
    inner: Watch<M, ConnectionStateValue, 8>,
    network: &'a NETWORK,
    dns: DNS,
    connection: RefCell<Option<<NETWORK as TcpConnect>::Connection<'a>>>,

    send_buffer: StackBufferCell<BUFFER_SIZE>, 
    recv_buffer: StackBufferCell<BUFFER_SIZE>, 

    last_will: Option<LastWill<'l>>,
    config: ClientConfig<'l>
}

impl<'a, 'l, M: RawMutex, NETWORK, DNS, const BUFFER_SIZE: usize> TcpConnectionState<'a, 'l, M, NETWORK, DNS, BUFFER_SIZE> 
where NETWORK: TcpConnect, DNS: Dns {

    pub fn new(network: &'a NETWORK, dns: DNS, last_will: Option<LastWill<'l>>, config: ClientConfig<'l>) -> Self {
        Self {
            inner: Watch::new(),
            network,
            dns,
            connection: RefCell::new(None),

            send_buffer: StackBufferCell::new(),
            recv_buffer: StackBufferCell::new(),

            last_will,
            config
        }
    }


    /// Read data from network until a packet can be received
    async fn read_packet(&self) -> Result<MappedBufferRef<'_, Packet<'_>, BUFFER_SIZE>, MqttError> {
        let mut connection = self.connection.borrow_mut();
        let connection = connection.as_mut().unwrap();
        
        loop {
            if let Some(packet) = self.try_read_packet()? {
                return Ok(packet);
            }

            trace!("could not read packet from network, wait for new data");
            let mut buffer = self.recv_buffer.borrow();
            let bytes_received = network_read!(buffer.writeable_data(), connection);
            buffer.commit_bytes_written(bytes_received).unwrap();
        }
    }

    fn try_read_packet(&self) -> Result<Option<MappedBufferRef<'_, Packet<'_>, BUFFER_SIZE>>, MqttError> {
        let buffer = self.recv_buffer.borrow();
        let is_max_len = buffer.is_max_len();

        let result = buffer.try_map_maybe(|buf| decode_slice_with_len(buf))
            .map_err(|err| MqttError::CodecError(err));

        match result {
            Ok(None) if is_max_len => {
                error!("recv buffer too small to receive packet");
                Err(MqttError::BufferTooSmall)
            },
            r => r
        }
    }


    /// Write bytes to network without blocking
    // fn try_send(&self) -> Result<(), MqttError> {
    //     let mut send_buffer = self.send_buffer.borrow();
    //     let mut connection = self.connection.borrow_mut();
    //     let connection = connection.as_mut().unwrap();

    //     let bytes_sent = NETWORK::try_write(send_buffer.reaable_data(), connection)?;
    //     send_buffer.add_bytes_read(bytes_sent).unwrap();

    //     Ok(())
    // }

    async fn send_all_intern(&self, connection: &mut NETWORK::Connection<'_>) -> Result<(), MqttError> {
        let mut send_buffer = self.send_buffer.borrow();
        while send_buffer.has_remaining_len() {
            let bytes_sent = network_write!(send_buffer.reaable_data(), connection);
            send_buffer.add_bytes_read(bytes_sent).unwrap();
        }

        network_flush!(connection);

        Ok(())
    }

    /// Sends data until the send buffer is empty
    async fn send_all(&self) -> Result<(), MqttError> {
        let mut connection = self.connection.borrow_mut();
        let connection = connection.as_mut().unwrap();

        self.send_all_intern(connection).await
    }

    // async fn receive(&self) -> Result<(), MqttError> {
    //     let mut recv_buffer = self.recv_buffer.borrow();
    //     let mut connection = self.connection.borrow_mut();
    //     let connection = connection.as_mut().unwrap();

    //     let bytes_received = NETWORK::read(recv_buffer.writeable_data(), connection).await?;
    //     recv_buffer.commit_bytes_written(bytes_received).unwrap();
    //     Ok(())
    // }

    /// Write bytes to network and block until at least one byte is written
    /// Returns arly if there is nothing to send
    async fn send(&self) -> Result<(), MqttError> {
        let mut send_buffer = self.send_buffer.borrow();
        // early return if there is nothing to send
        if ! send_buffer.has_remaining_len() {
            trace!("network send: nothing to send");
            return Ok(());
        }

        let mut connection = self.connection.borrow_mut();
        let connection = connection.as_mut().unwrap();

        let bytes_sent = network_write!(send_buffer.reaable_data(), connection);
        debug!("sent {} bytes to network", bytes_sent);
        send_buffer.add_bytes_read(bytes_sent).unwrap();
        connection.flush().await
            .map_err(|err| MqttError::ConnectionFailed2(err.kind()))?;

        Ok(())
    }

    /// Writes a packet to the network and waits until there is enaugh space
    async fn write_packet_async(&self, packet: &Packet<'_>) -> Result<(), MqttError> {
        loop {
            if self.try_write_packet(packet)? {
                return Ok(())
            }

            self.send().await?;
        }
    }

    /// Tries to write a packet to the send buffer
    /// Returns an error if the packet does not fit in the send buffer
    fn try_write_packet(&self, packet: &Packet<'_>) -> Result<bool, MqttError> {
        let mut send_buffer = self.send_buffer.borrow();
        send_buffer.flip();
        let buf = send_buffer.writeable_data();

        match encode_slice(packet, buf) {
            Ok(bytes_written) => {
                send_buffer.commit_bytes_written(bytes_written).unwrap();
                Ok(true)
            },
            Err(mqttrs2::Error::WriteZero) => {
                if send_buffer.is_max_capacity() {
                    error!("send buffer too small to write packet {}", packet);
                    Err(MqttError::BufferTooSmall)
                } else {
                    trace!("could not write packet: no space in buffer");
                    Ok(false)
                }
            },
            Err(err) => Err(err.into()),
        }
    }

    async fn subscribe_auto_subscribes(&self) -> Result<(), MqttError> {
        for chunk in self.config.auto_subscribes.chunks(5) {
            debug!("autosubscribe chunk of {} topics", chunk.len());
            let pid = next_pid();
            
            let topics = chunk.iter()
                .map(|el| el.try_into())
                .collect::<Result<_, _>>()?;
            
            let request = Subscribe{
                pid,
                topics
            };
            let request = Packet::Subscribe(request);

            self.write_packet_async(&request).await?;
            
            // Send everything that is in the send buffer
            self.send_all().await?;


            // let suback = self.send_receive_until(connection, |packet| match packet {
            //     Packet::Suback(suback) if suback.pid == pid => Ok(suback),
            //     unexpected_packet => {
            //         error!("got unexpected packet {} while waiting for suback", unexpected_packet.get_type());
            //         return Err(MqttError::ConnackError)
            //     }
            // }).await??;

            let next_packet = self.read_packet().await?;
            let suback = match next_packet.deref() {
                Packet::Suback(suback) => suback,
                unexpected_packet => {
                    error!("got unexpected packet {} while waiting for suback", unexpected_packet.get_type());
                    return Err(MqttError::ConnackError)
                }
            };

            Self::process_suback(suback, chunk)?;
        }

        info!("auto subscribes done");

        Ok(())
    }

    fn process_suback(suback: &Suback, auto_subscribes: &[AutoSubscribe]) -> Result<(), MqttError> {

        for (suback, requestes_topic) in suback.return_codes.iter().zip(auto_subscribes.iter()) {
            match suback {
                mqttrs2::SubscribeReturnCodes::Success(qos) if *qos == requestes_topic.qos => {
                    info!("successfully auto subscribes to {} with {}", &requestes_topic.topic, qos);
                },
                mqttrs2::SubscribeReturnCodes::Success(qos) => {
                    warn!("autosubscribes to {} with different qos: requested {} but got {}", &requestes_topic.topic, requestes_topic.qos, qos);
                },
                mqttrs2::SubscribeReturnCodes::Failure => {
                    error!("could not auto subscribe t {}", &requestes_topic.topic);
                    return Err(MqttError::SubscribeOrUnsubscribeFailed);
                },
            }
        }

        Ok(())
    }

    async fn process_connack(&self, connack: &Connack) -> Result<(), MqttError> {

        match connack.code {
            mqttrs2::ConnectReturnCode::Accepted => {
                info!("connction to broker established");

                // Add autosubscribe requests
                self.subscribe_auto_subscribes().await?;

                self.inner.sender().send(ConnectionStateValue::Connected);

                Ok(())
            },
            mqttrs2::ConnectReturnCode::RefusedProtocolVersion | mqttrs2::ConnectReturnCode::RefusedIdentifierRejected | mqttrs2::ConnectReturnCode::ServerUnavailable => {
                error!("connack returned error: {}", connack.code);
                self.inner.sender().send(ConnectionStateValue::Error);
                Err(MqttError::ConnackError)
            },
            mqttrs2::ConnectReturnCode::BadUsernamePassword | mqttrs2::ConnectReturnCode::NotAuthorized => {
                error!("connack: authentication failed: {}", connack.code);
                self.inner.sender().send(ConnectionStateValue::Error);
                Err(MqttError::AuthenticationError)
            }
        }
    }

    async fn send_receive(&self) -> Result<(), MqttError> {
        let mut send_buffer = self.send_buffer.borrow();
        let mut recv_buffer = self.recv_buffer.borrow();
        recv_buffer.flip();

        let mut connection = self.connection.borrow_mut();
        let connection = connection.as_mut().unwrap();

        while send_buffer.has_remaining_len() {
            let bytes_sent = network_write!(send_buffer.reaable_data(), connection);
            send_buffer.add_bytes_read(bytes_sent).unwrap();
            network_flush!(connection);
            debug!("write {} bytes to network, {} remaining", bytes_sent, send_buffer.reaable_data().len());

            let bytes_received = network_try_read(connection, recv_buffer.writeable_data())?;
            recv_buffer.commit_bytes_written(bytes_received).unwrap();
            debug!("try_read {} bytes from network", bytes_received);

            if bytes_received > 0 {
                return Ok(())
            }
        }

        let bytes_received = network_read!(recv_buffer.writeable_data(), connection);
        recv_buffer.commit_bytes_written(bytes_received).unwrap();
        debug!("read {} bytes from network", bytes_received);

        Ok(())
    }

}

impl <'a, 'l, M: RawMutex, NETWORK, DNS, const BUFFER_SIZE: usize> ConnectionState for TcpConnectionState<'a, 'l, M, NETWORK, DNS, BUFFER_SIZE> 
where NETWORK: TcpConnect, DNS: Dns {
    
    fn get_state(&self) -> Option<ConnectionStateValue> {
        self.inner.try_get()
    }

    async fn on_state_change(&self) -> ConnectionStateValue {
        self.inner.dyn_receiver().unwrap().changed().await
    }

    async fn disconnect(&self) -> Result<(), MqttError> {
        assert!(self.inner.try_get() == Some(ConnectionStateValue::Connected));
        info!("disconnect started");
        
        // Empty send buffer
        self.send_all().await?;

        let sent = self.try_write_packet(&Packet::Disconnect)?;
        if ! sent {
            panic!("could not write disconnect to send buffer");
        }

        self.send_all().await?;

        // Drop connection
        let connection = self.connection.borrow_mut().take().unwrap();
        drop(connection);

        self.inner.sender().send(ConnectionStateValue::Disconnected);

        Ok(())
    }
    
    async fn connect(&self) -> Result<(), MqttError> {
        assert!(self.inner.try_get() == None || self.inner.try_get() == Some(ConnectionStateValue::Error));

        self.recv_buffer.borrow().reset();
        self.send_buffer.borrow().reset();

        {       
            let port = self.config.port.unwrap_or(MQTT_DEFAULT_PORT);
            debug!("connect using port {}", port);
            let ip = self.config.host.resolve(&self.dns).await?;
            let addr = SocketAddr::new(ip, port);

            trace!("start connecting to socket addr {}", Debug2Format(&addr));
            let connection = self.network.connect(addr).await
                .map_err(|err| MqttError::ConnectionFailed2(err.kind()))?;
            trace!("successfully established tcp connection to broker");


            let mut connection_lock = self.connection.borrow_mut();
            *connection_lock = Some(connection);
        }

        info!("tcp connection to broker established");

        let mut connect = Connect{
            protocol: Protocol::MQTT311,
            keep_alive: super::KEEP_ALIVE as u16,
            client_id: &self.config.client_id,
            clean_session: false,
            last_will: self.last_will.clone(),
            username: None,
            password: None
        };

        if let Some(cred) = &self.config.credentials {
            connect.username = Some(&cred.username);
            connect.password = Some(cred.password.as_bytes());
        }

        let connect = Packet::Connect(connect);

        let sent = self.try_write_packet(&connect)?;
        if ! sent {
            error!("connect packet does not fit in send buffer");
            return Err(MqttError::BufferTooSmall);
        }

        // Send everything that is in the send buffer
        self.send().await?;

        self.inner.sender().send(ConnectionStateValue::ConnectSent);

        let next_packet = self.read_packet().await?;
        let connack = match next_packet.deref() {
            Packet::Connack(connack) => connack,
            unexpected_packet => {
                error!("got unexpected packet {} while waiting for connack", unexpected_packet.get_type());
                return Err(MqttError::ConnackError)
            }
        };

        let connack = connack.clone();
        drop(next_packet); // Frees borrow of recv_buffer

        
        self.process_connack(&connack).await?;
        
        Ok(())
    }

    async fn await_connected(&self) {
        self.inner.receiver().unwrap().get_and(|value| *value == ConnectionStateValue::Connected).await;
    }

    fn set_error(&self) {
        self.inner.sender().send(ConnectionStateValue::Error);
    }

    fn try_write_packet(&self, packet: &Packet<'_>) -> Result<bool, MqttError> {
        let mut send_buffer = self.send_buffer.borrow();

        match encode_slice(packet, send_buffer.writeable_data()) {
            Err(mqttrs2::Error::WriteZero) => Ok(false),
            Ok(n) => {
                send_buffer.commit_bytes_written(n).unwrap();
                Ok(true)
            }
            Err(err) => Err(MqttError::CodecError(err))
        }
    }

    async fn write_packet(&self, packet: &Packet<'_>) -> Result<(), MqttError> {
        self.write_packet_async(packet).await
    }

    async fn run_io(&self) -> Result<impl Deref<Target = Packet<'_>>, MqttError> {
        assert_eq!(self.inner.try_get(), Some(ConnectionStateValue::Connected));

        loop {
            self.send_receive().await?;

            if let Some(packet) = self.try_read_packet()? {
                return Ok(packet)
            }
        }
    }

    async fn run_io_nonblocking(&self) -> Result<Option<impl Deref<Target = Packet<'_>>>, MqttError> {
        assert_eq!(self.inner.try_get(), Some(ConnectionStateValue::Connected));

        self.send().await?;
        
        {
            let mut connection = self.connection.borrow_mut();
            let connection = connection.as_mut().unwrap();
            let mut recv_buffer = self.recv_buffer.borrow();
            let bytes_received = network_try_read(connection, recv_buffer.writeable_data())?;
            recv_buffer.commit_bytes_written(bytes_received).unwrap();
        }


        self.try_read_packet()
    }
}

#[cfg(test)]
mod tests {
    use core::{net::{IpAddr, Ipv4Addr}, pin::{Pin, pin}};

    use embassy_sync::blocking_mutex::raw::{CriticalSectionRawMutex, RawMutex};
    use heapless::Vec;
    use mqttrs2::{Connack, ConnectReturnCode, Packet, PacketType};

    use crate::{ClientConfig, Host, state::connection::{ConnectionState, ConnectionStateValue, TcpConnectionState, test::{TestDns, TestTcpConnect}}};
    use crate::testutils::*;

    #[test]
    fn test_connect() {
        let tcp = TestTcpConnect::new(); 
        let dns = TestDns::new_single("my-mqtt-test-broker", IpAddr::V4(Ipv4Addr::new(127, 0, 0, 1)));

        let client_config = ClientConfig {
            host: Host::Hostname("my-mqtt-test-broker"),
            port: None,
            client_id: "clien-12345", 
            credentials: None, 
            auto_subscribes: Vec::new(),
        };

        let connection_state: TcpConnectionState<'_, '_, CriticalSectionRawMutex, _, _, 1024> = TcpConnectionState::new(&tcp, dns, None, client_config.clone());
        let mut connection_state_receiver = connection_state.inner.dyn_receiver().unwrap();

        let mut connect_future = connection_state.connect();
        let mut connect_future = unsafe {
            Pin::new_unchecked(&mut connect_future)
        };

        // Send connect packet to network, now waiting for connack
        assert_pending(connect_future.as_mut());
        assert_eq!(connection_state_receiver.try_changed(), Some(ConnectionStateValue::ConnectSent));

        // test if the connect packet was written
        tcp.assert_packet_written(|packet| match packet {
            Packet::Connect(connect) => {
                assert_eq!(connect.client_id, client_config.client_id);
                // TODO add more asserts
            },
            p => panic!("unexpected packet: {:?}", p)
        });

        // "receive" a conack
        let connack = Connack{
            session_present: false,
            code: ConnectReturnCode::Accepted
        };

        tcp.add_packet_to_receive(&Packet::Connack(connack));

        // process connack
        // Ready because there are no autosubscribes
        assert_ready(connect_future.as_mut()).unwrap();
        assert_eq!(connection_state_receiver.try_changed(), Some(ConnectionStateValue::Connected));
    }

    fn connect<M: RawMutex, const BUFFER: usize>(state: &TcpConnectionState<'_, '_, M, TestTcpConnect, TestDns, BUFFER>, tcp: &TestTcpConnect) {
        let connect_future = state.connect();
        let mut connect_future = pin!(connect_future);

        // Send connect packet to network, now waiting for connack
        assert_pending(connect_future.as_mut());

        tcp.assert_packet_written(|p| {
            assert_eq!(p.get_type(), PacketType::Connect);
        });

        // "receive" a conack
        let connack = Connack{
            session_present: false,
            code: ConnectReturnCode::Accepted
        };

        tcp.add_packet_to_receive(&Packet::Connack(connack));

        assert_ready(connect_future.as_mut()).unwrap();
    }

    #[test]
    fn test_run_io_read() {
        let tcp = TestTcpConnect::new(); 
        let dns = TestDns::new_single("my-mqtt-test-broker", IpAddr::V4(Ipv4Addr::new(127, 0, 0, 1)));

        let client_config = ClientConfig {
            host: Host::Hostname("my-mqtt-test-broker"),
            port: None,
            client_id: "clien-12345", 
            credentials: None, 
            auto_subscribes: Vec::new(),
        };

        let connection_state: TcpConnectionState<'_, '_, CriticalSectionRawMutex, _, _, 1024> = TcpConnectionState::new(&tcp, dns, None, client_config.clone());

        connect(&connection_state, &tcp);

        let run_io_future = connection_state.run_io();
        let mut run_io_future = pin!(run_io_future);

        // Nothing to send and retrieve
        assert_pending(run_io_future.as_mut());
        assert_pending(run_io_future.as_mut());
        assert_pending(run_io_future.as_mut());
        assert_pending(run_io_future.as_mut());

        tcp.add_packet_to_receive(&Packet::Pingresp);

        // There is a packet to receive, return now
        assert_ready(run_io_future).unwrap();
    }

    #[test]
    fn test_run_io_read_write() {
        let tcp = TestTcpConnect::new(); 
        let dns = TestDns::new_single("my-mqtt-test-broker", IpAddr::V4(Ipv4Addr::new(127, 0, 0, 1)));

        let client_config = ClientConfig {
            host: Host::Hostname("my-mqtt-test-broker"),
            port: None,
            client_id: "clien-12345", 
            credentials: None, 
            auto_subscribes: Vec::new(),
        };

        let connection_state: TcpConnectionState<'_, '_, CriticalSectionRawMutex, _, _, 1024> = TcpConnectionState::new(&tcp, dns, None, client_config.clone());
        connect(&connection_state, &tcp);

        assert!(connection_state.try_write_packet(&Packet::Pingreq).unwrap());

        let run_io_future = connection_state.run_io();
        let mut run_io_future = pin!(run_io_future);

        // Nothing to send and retrieve
        assert_pending(run_io_future.as_mut());
        tcp.assert_packet_written(|p| {
            assert_eq!(*p, Packet::Pingreq);
        });

        tcp.add_packet_to_receive(&Packet::Pingresp);

        // There is a packet to receive, return now
        assert_ready(run_io_future).unwrap();
    }

    #[test]
    fn test_run_io_nonblocking_read_write() {
        let tcp = TestTcpConnect::new(); 
        let dns = TestDns::new_single("my-mqtt-test-broker", IpAddr::V4(Ipv4Addr::new(127, 0, 0, 1)));

        let client_config = ClientConfig {
            host: Host::Hostname("my-mqtt-test-broker"),
            port: None,
            client_id: "clien-12345", 
            credentials: None, 
            auto_subscribes: Vec::new(),
        };

        let connection_state: TcpConnectionState<'_, '_, CriticalSectionRawMutex, _, _, 1024> = TcpConnectionState::new(&tcp, dns, None, client_config.clone());
        connect(&connection_state, &tcp);

        // First time: Nothing to send and something to retrieve
        let run_io_future = connection_state.run_io_nonblocking();

        // Nothing to send and retrieve
        assert_ready_pin(run_io_future).unwrap();
        tcp.assert_nothing_written();
        

        // Second time: something to send and nothing to retrieve
        assert!(connection_state.try_write_packet(&Packet::Pingreq).unwrap());
        let run_io_future = connection_state.run_io_nonblocking();
        let result = assert_ready_pin(run_io_future).unwrap();
        assert!(result.is_none());
    }

    #[test]
    fn test_disconnect() {
        let tcp = TestTcpConnect::new(); 
        let dns = TestDns::new_single("my-mqtt-test-broker", IpAddr::V4(Ipv4Addr::new(127, 0, 0, 1)));

        let client_config = ClientConfig {
            host: Host::Hostname("my-mqtt-test-broker"),
            port: None,
            client_id: "clien-12345", 
            credentials: None, 
            auto_subscribes: Vec::new(),
        };

        let connection_state: TcpConnectionState<'_, '_, CriticalSectionRawMutex, _, _, 1024> = TcpConnectionState::new(&tcp, dns, None, client_config.clone());
        connect(&connection_state, &tcp);

        let disconnect_future = connection_state.disconnect();
        assert_ready_pin(disconnect_future).unwrap();

        tcp.assert_packet_written(|p| {
            assert_eq!(p.get_type(), PacketType::Disconnect);
        });

        // assert_eq!(tcp.connections_closed.load(core::sync::atomic::Ordering::Acquire), 1);
        assert_eq!(connection_state.inner.dyn_receiver().unwrap().try_get(), Some(ConnectionStateValue::Disconnected));

    }


}

