use core::{cell::RefCell, future::Future};

use buffer::{new_stack_buffer, Buffer, BufferReader, BufferWriter};
use embassy_futures::select::{select, select3};
use embassy_sync::blocking_mutex::raw::RawMutex;
use mqttrs::decode_slice_with_len;
use crate::{network::{Network, NetworkConnection}, state::State, ClientConfig, MqttError, MqttEvent, MqttRequest};

pub trait AsyncSender<T>: Sync {
    fn send(&self, item: T) -> impl Future<Output = ()> + Send;
}

pub trait AsyncReceiver<T>: Sync {
    fn receive(&self) -> impl Future<Output = T> + Send;
}

pub struct MqttEventLoop<M: RawMutex, N: NetworkConnection, const B: usize, C: AsyncSender<MqttEvent>, R: AsyncReceiver<MqttRequest>> {
    recv_buffer: RefCell<Buffer<[u8; B]>>,
    send_buffer: RefCell<Buffer<[u8; B]>>,

    connection: RefCell<Network<N>>,


    state: State<M>,

    control_sender: C,
    request_receiver: R
}

impl <M: RawMutex, N: NetworkConnection, const B: usize, C: AsyncSender<MqttEvent>, R: AsyncReceiver<MqttRequest>> MqttEventLoop<M, N, B, C, R> {

    pub fn new(connection: N, config: ClientConfig, control_sender: C, request_receiver: R) -> Self {
        
        Self {
            recv_buffer: RefCell::new(new_stack_buffer::<B>()),
            send_buffer: RefCell::new(new_stack_buffer::<B>()),
            connection: RefCell::new(Network(connection)),

            state: State::new(config),

            control_sender,
            request_receiver
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
                error!("error decoding package: {}", crate::fmt::Debug2Format(&e));
                MqttError::CodecError
            })?;
        
        if let Some((len, packet)) = packet_op {
            recv_buffer.add_bytes_read(len);
            let event_option = self.state.process_packet(&packet, send_buffer)?;
            if let Some(event) = event_option {
                self.control_sender.send(event).await;
            }
        }

        Ok(())
    }

    /// Makes the receive / send of the network
    async fn work_network(&self) -> Result<!, MqttError> {
        loop {
            
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

            // Send packets (Ping, Connect, Publish)
            self.state.send_packets(&mut send_buffer_writer, &self.control_sender).await?;
        }
    }

    async fn work_request_receive(&self) -> Result<!, MqttError> {
        loop {
            let req = self.request_receiver.receive().await;

            match req {
                MqttRequest::Publish(mqtt_publish, id) => {
                    self.state.publishes.push_publish(mqtt_publish, id).await;
                },
                MqttRequest::Subscribe(topic, unique_id) => todo!(),
                MqttRequest::Unsubscribe(topic, unique_id) => todo!(),
            }

            // Signal that a new request is added
            self.state.on_requst_added.signal(0);
        }
    }

    async fn work(&mut self) -> Result<!, MqttError> {
        // Reset state on new connection
        self.state.reset();

        self.connection.borrow_mut().0.connect().await?;
            
        // Poll both futures
        // Join should never befinished because both jobs are infinite
        let network_future = self.work_network();
        let request_future = self.work_request_receive();
        match select(network_future, request_future).await {
            embassy_futures::select::Either::First(net_result) => {
                let err = net_result.unwrap_err();
                error!("network infinite jom finished: {}", err);
                Err(err)
            },
            embassy_futures::select::Either::Second(req_result) => {
                let err = req_result.unwrap_err();
                error!("infinite request receive job finished: {}", err);
                Err(err)
            },
        }
    }

    pub async fn run(&mut self) {
        loop {

            let e = self.work().await.unwrap_err();

            warn!("connection failed: {}", e);

        }
    }


}
