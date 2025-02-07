use core::cell::RefCell;

use buffer::BufferWriter;

use embassy_sync::blocking_mutex;
use embassy_sync::blocking_mutex::raw::RawMutex;
use embassy_sync::signal::Signal;
use embassy_time::{Instant, Timer};
use mqttrs::{encode_slice, Connack, Connect, Error, Packet, Protocol, Publish, QosPid};
use ping::PingState;
use publish::PublishQueue;

use crate::io::AsyncSender;
use crate::{ClientConfig, MqttError, MqttEvent, MqttPublish};
use crate::fmt::Debug2Format;

pub(crate) const KEEP_ALIVE: usize = 60;

pub(crate) mod ping;
pub(crate) mod publish;

#[derive(Debug, Clone)]
pub(crate) enum ConnectionState {
    /// TCP Connection established, but nothis has happened yet
    InitialState,

    /// The Connect package is sent but the connack is not received yet
    ConnectSent,

    Connected,

    Failed(MqttError)

}

pub(crate) struct State<M: RawMutex> {

    connection: blocking_mutex::Mutex<M, RefCell<ConnectionState>>,
    config: ClientConfig,
    ping: blocking_mutex::Mutex<M, RefCell<PingState>>,

    pub(crate) publishes: PublishQueue,

    // Signal is sent, when a request is added
    // TODO update to emassy_sync::watch::Watch is update is there
    pub(crate) on_requst_added: Signal<M, usize>

}

impl <M: RawMutex> State<M> {

    pub fn new(config: ClientConfig) -> Self {
        Self {
            connection: blocking_mutex::Mutex::new(RefCell::new(ConnectionState::InitialState)),
            config,
            ping: blocking_mutex::Mutex::new(RefCell::new(PingState::PingSuccess(Instant::now()))),

            publishes: PublishQueue::new(),

            on_requst_added: Signal::new()
        }
    }

    pub fn reset(&self) {
        self.set_connection_state(ConnectionState::InitialState);
    }

    fn set_connection_state(&self, new_state: ConnectionState) {
        self.connection.lock(|inner| {
            let mut inner = inner.borrow_mut();
            *inner = new_state;
        })
    }

    fn send_connect_package(&self, send_buffer: &mut impl BufferWriter) -> Result<(), MqttError> {

        let mut connect_packet = Connect{
            protocol: Protocol::MQTT311,
            keep_alive: KEEP_ALIVE as u16,
            client_id: &self.config.client_id,
            clean_session: false,
            last_will: None,
            username: None,
            password: None
        };

        if let Some(cred) = &self.config.credentials {
            connect_packet.username = Some(&cred.username);
            connect_packet.password = Some(cred.password.as_bytes());
        }

        let connect_packet = Packet::Connect(connect_packet);

        let sent = Self::encode_packet(&connect_packet, send_buffer)?;
        if sent {
            self.set_connection_state(ConnectionState::ConnectSent);
        }

        Ok(())
    }

    fn encode_packet(packet: &Packet<'_>, send_buffer: &mut impl BufferWriter) -> Result<bool, MqttError> {
        let result = encode_slice(packet, send_buffer);

        match result {
            Ok(n) => {
                send_buffer.commit(n).unwrap();
                trace!("successfully encoded {} package to send_buffer: {} bytes", packet.get_type(), n);
                Ok(true)
            },
            Err(Error::WriteZero) => {
                debug!("cannot write {} packet: buffer not enaugh space", packet.get_type());
                Ok(false)
            },
            Err(e) => {
                error!("error encoding {} package: {}", packet.get_type(), Debug2Format(&e));
                Err(MqttError::CodecError)
            }
        }
    }


    fn process_connack(&self, connack: &Connack) -> Result<(), MqttError> {

        match connack.code {
            mqttrs::ConnectReturnCode::Accepted => {
                self.set_connection_state(ConnectionState::Connected);
                info!("connction to broker established");
                // TODO send connected notification
                Ok(())
            },
            mqttrs::ConnectReturnCode::RefusedProtocolVersion => {
                error!("connack: broker refused protocol version");
                self.set_connection_state(ConnectionState::Failed(MqttError::ConnackError));
                Err(MqttError::ConnackError)
            },
            mqttrs::ConnectReturnCode::RefusedIdentifierRejected => {
                error!("connack: RefusedIdentifierRejected");
                self.set_connection_state(ConnectionState::Failed(MqttError::ConnackError));
                Err(MqttError::ConnackError)
            },
            mqttrs::ConnectReturnCode::ServerUnavailable => {
                error!("connack: ServerUnavailable");
                self.set_connection_state(ConnectionState::Failed(MqttError::ConnackError));
                Err(MqttError::ConnackError)
            },
            mqttrs::ConnectReturnCode::BadUsernamePassword => {
                error!("connack: bad credentials");
                self.set_connection_state(ConnectionState::Failed(MqttError::AuthenticationError));
                Err(MqttError::AuthenticationError)
            },
            mqttrs::ConnectReturnCode::NotAuthorized => {
                error!("connack: not authorized");
                self.set_connection_state(ConnectionState::Failed(MqttError::AuthenticationError));
                Err(MqttError::AuthenticationError)
            },
        }
    }

    fn process_pingresp(&self) {
        debug!("received pingresp from broker");
        self.ping.lock(|inner|{
            inner.borrow_mut().on_ping_response();
        });
    }

    pub(crate) async fn send_packets(&self, send_buffer: &mut impl BufferWriter, control_sender: & impl AsyncSender<MqttEvent>) -> Result<(), MqttError> {

        let state = self.connection.lock(|inner| inner.borrow().clone());

        match state {
            ConnectionState::InitialState => self.send_connect_package(send_buffer),

            // Do not send anything while connecting
            ConnectionState::ConnectSent => Ok(()),

            // Send ping, subscribes, publishes, ...
            ConnectionState::Connected => self.send_packets_connected(send_buffer, control_sender).await,

            ConnectionState::Failed(mqtt_error) => Err(mqtt_error.clone()),
        }
    }

    async fn send_packets_connected(&self, send_buffer: &mut impl BufferWriter, control_sender: &impl AsyncSender<MqttEvent>) -> Result<(), MqttError> {

        let is_critical = self.ping.lock(|inner|{
            let mut inner = inner.borrow_mut();

            if inner.should_send_ping() {
                let ping = Packet::Pingreq;
                let sent = Self::encode_packet(&ping, send_buffer)?;
                if sent {
                    inner.ping_sent();
                }
            }
    
            Ok(inner.is_critical_delay())
        })?;

        // Do not do anything else if the ping delay is critical (near keepalive)
        if is_critical {
            return Ok(());
        }

        // Publish and republish packets
        self.publishes.process(send_buffer, control_sender).await?;

        Ok(())
    }

    /// Processes an incoming publish packet
    fn process_publish(&self, publish: &Publish<'_>) -> Result<Option<MqttPublish>, MqttError> {

        if publish.dup {
            warn!("retrieved package with dup flag set, ignoring");
            return Ok(None);
        }

        if publish.qospid != QosPid::AtMostOnce {
            warn!("retrieved package with QoS {}. Only AtMostOnce supportes", publish.qospid);
        }

        Ok(Some(MqttPublish::try_from(publish)?))
    }

    /// Processes incoming packets
    pub(crate) fn process_packet(&self, p: &Packet<'_>, send_buffer: &mut impl BufferWriter) -> Result<Option<MqttEvent>, MqttError> {

        match p {
            
            Packet::Connack(connack) => {
                self.process_connack(connack)?;
                Ok(Some(MqttEvent::Connected))
            },
            
            Packet::Publish(publish) => {
                self.process_publish(publish)
                    .map(|o| o.map(|p| MqttEvent::PublishReceived(p)))
            },
            
            Packet::Puback(pid) => {
                let result = self.publishes.process_puback(pid);
                Ok(result)
            },
            
            Packet::Pubrec(pid) => {
                self.publishes.process_pubrec(pid, send_buffer)?;
                Ok(None)
            },

            Packet::Pubcomp(pid) => {
                let result = self.publishes.process_pubcomp(pid);
                Ok(result)
            },

            Packet::Suback(suback) => todo!(),
            
            Packet::Unsuback(pid) => todo!(),
            
            Packet::Pingresp => {
                self.process_pingresp();
                Ok(None)
            },

            // # These Packages cannot be send Server -> Client
            // # And are treated as unexpected
            // Packet::Connect(connect) => todo!(),
            // Packet::Disconnect => todo!(),
            // Packet::Pingreq => todo!(),
            // Packet::Unsubscribe(unsubscribe) => todo!(),
            // Packet::Subscribe(subscribe) => todo!(),
            // Packet::Pubrel(pid) => todo!(),

            unexpected => {
                error!("unexpected packet {} received from broker", unexpected.get_type());
                Ok(None)
            }
        }
    }

    pub(crate) async fn on_ping_required(&self) {
        match self.ping.lock(|p| p.borrow().ping_pause()) {
            Some(pause) => Timer::after(pause).await,
            None => {},
        }
        debug!("Mqtt ping required");
    }

}