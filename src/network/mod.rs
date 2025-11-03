use core::{fmt::Debug, future::Future};



#[cfg(feature = "embassy")]
pub mod embassy;

#[cfg(feature = "std")]
pub mod std;

#[cfg(test)]
pub mod test;

#[derive(Debug, thiserror::Error, Clone, PartialEq)]
pub enum NetworkError {
    #[deprecated]
    #[error("error while reading or writing")]
    ReadWriteError,

    #[error("host not found")]
    HostNotFound,

    #[error("hconnection reset")]
    ConnectionReset,

    #[error("network timeout")]
    Timeout,

    #[error("dns failed: `{0}`")]
    DnsFailed(&'static str),

    #[error("unexpected network error")]
    Unexpected
}

#[cfg(feature = "embassy")]
impl From<embassy_net::tcp::ConnectError> for NetworkError {
    fn from(value: embassy_net::tcp::ConnectError) -> Self {
        match value {
            embassy_net::tcp::ConnectError::InvalidState => todo!(),
            embassy_net::tcp::ConnectError::ConnectionReset => todo!(),
            embassy_net::tcp::ConnectError::TimedOut => Self::Timeout,
            embassy_net::tcp::ConnectError::NoRoute => todo!(),
        }
    }
}

#[cfg(feature = "embassy")]
impl From<embassy_net::dns::Error> for NetworkError {
    fn from(value: embassy_net::dns::Error) -> Self {
        Self::DnsFailed(match value {
            embassy_net::dns::Error::InvalidName => "invalid name",
            embassy_net::dns::Error::NameTooLong => "name too long",
            embassy_net::dns::Error::Failed => "failed",
        })
    }
}

#[cfg(feature = "embassy")]
impl From<embassy_net::tcp::Error> for NetworkError {
    fn from(value: embassy_net::tcp::Error) -> Self {
        match value {
            embassy_net::tcp::Error::ConnectionReset => Self::ConnectionReset,
        }
    }
}


pub trait PlattformNetwork {
    type Connection<'c>;

    fn write<'a>(buf: &'a [u8], connection: &'a mut Self::Connection<'_>) -> impl Future<Output = Result<usize, NetworkError>> + 'a;
    fn try_write(buf: &[u8], connection: &mut Self::Connection<'_>) -> Result<usize, NetworkError>;
    fn flush<'a>(connection: &'a mut Self::Connection<'_>) -> impl Future<Output = Result<(), NetworkError>> + 'a;

    fn read<'a>(buf: &'a mut[u8], connection: &'a mut Self::Connection<'_>) -> impl Future<Output = Result<usize, NetworkError>> + 'a;
    fn try_read(buf: &mut[u8], connection: &mut Self::Connection<'_>) -> Result<usize, NetworkError>;

    fn close(connection: Self::Connection<'_>);

    fn connect<'a>(&'a self) -> impl Future<Output = Result<Self::Connection<'a>, NetworkError>>;
}

// pub struct BufferedNetwork<'a, const BUFFER_SIZE: usize, NETWORK> where NETWORK: PlattformNetwork  {
//     recv_buffer: RefCell<StackBuffer<BUFFER_SIZE>>,
//     send_buffer: RefCell<StackBuffer<BUFFER_SIZE>>,
//     network: &'a NETWORK,
// }

// impl <'a, const BUFFER_SIZE: usize, NETWORK> BufferedNetwork<'a, BUFFER_SIZE, NETWORK> where NETWORK: PlattformNetwork {

//     pub fn new(network: &'a NETWORK) -> Self {
//         Self {
//             recv_buffer: RefCell::new(StackBuffer::new()),
//             send_buffer: RefCell::new(StackBuffer::new()),
//             network,
//         }
//     }

//     pub fn read_packet<F, U>(&self, f: F) -> Result<Option<U>, MqttError> where F: FnOnce(Packet<'_>) -> U, U: Send {
//         let reader = self.recv_buffer.create_reader();

//         reader.read_slice(|buf| {
//             match decode_slice_with_len(buf) {
//                 Ok(Some((bytes_read, p))) => {
//                     (bytes_read, Ok(Some(f(p))))
//                 },
//                 Ok(None) => (0, Ok(None)),
//                 Err(err) => (0, Err(MqttError::CodecError(err)))
//             }
//         }).unwrap() // error only occures if a wrong number of bytes is returned by the closure. 
//     }

//     pub async fn write_packet(&self, packet: &Packet<'_>) -> Result<(), MqttError> {
//         let writer = self.send_buffer.create_writer();
//         writer.write_slice_async(|buf| {
//             match encode_slice(&packet, buf) {
//                 Ok(bytes_written) => WriteSliceAsyncResult::Ready(bytes_written, Ok(())),
//                 Err(mqttrs2::Error::WriteZero) => WriteSliceAsyncResult::Wait,
//                 Err(err) => WriteSliceAsyncResult::Ready(0, Err(MqttError::CodecError(err)))
//             }
//         }).await.unwrap() // error only occures if a wrong number of bytes is returned by the closure. 
//     }

//     pub fn try_write_packet(&self, packet: &Packet<'_>) -> Result<bool, MqttError> {
//         let writer = self.send_buffer.create_writer();
//         writer.write_slice(|buf| {
//             match encode_slice(&packet, buf) {
//                 Ok(bytes_written) => (bytes_written, Ok(true)),
//                 Err(mqttrs2::Error::WriteZero) => (0, Ok(false)),
//                 Err(err) => (0, Err(MqttError::CodecError(err)))
//             }
//         }).unwrap() 
//     }

//     pub fn tcp_connect(&self) -> impl Future<Output = Result<NETWORK::Connection<'a>, NETWORK::Error>> {
//         self.network.connect()
//     }

//     pub async fn write_network_all(&self, connection: &mut NETWORK::Connection<'_>) -> Result<(), NetworkError> {
//         let send_buffer_reader = self.send_buffer.create_reader();

//         while send_buffer_reader.len() > 0 {
//             let lock = send_buffer_reader.lock().await;
//             match NETWORK::write(&lock, connection).await {
//                 Ok(bytes_written) => {
//                     debug!("write_network_all:write  {} bytes to network", bytes_written);
//                     lock.set_bytes_read(bytes_written).unwrap();
//                 },
//                 Err(err) => {
//                     error!("error writing to network: {}", err);
//                     return Err(NetworkError::ReadWriteError);
//                 }
//             }
//         }
//         Ok(())
//     }

//     pub fn try_write_network(&self, connection: &mut NETWORK::Connection<'_>) -> Result<(), NetworkError> {
//         let send_buffer_reader = self.send_buffer.create_reader();

//         send_buffer_reader.read_slice(|buf| {
//             match NETWORK::try_write(buf, connection) {
//                 Ok(bytes_written) => {
//                     debug!("try_write {} bytes to network", bytes_written);
//                     (bytes_written, Ok(()))
//                 },
//                 Err(err) => {
//                     error!("error writing to network: {}", err);
//                     (0, Err(NetworkError::ReadWriteError))
//                 }
//             }
//         }).unwrap()
//     }

//     pub async fn read_write_network(&self, connection: &mut NETWORK::Connection<'_>) -> Result<(), NetworkError> {

//         let send_buffer_reader = self.send_buffer.create_reader();
//         let recv_buffer_writer = self.recv_buffer.create_writer();

//         if send_buffer_reader.len() > 0 {
//             let send_buffer_lock = send_buffer_reader.lock().await;
//             let bytes_written = NETWORK::try_write(&send_buffer_lock, connection)
//                 .map_err(|err| {
//                     error!("error writing to network: {}", err);
//                     NetworkError::ReadWriteError
//                 })?;
//             send_buffer_lock.set_bytes_read(bytes_written).unwrap();
//         }

//         if send_buffer_reader.len() > 0 {
//             // Send buffer has something to send left, just try receive
//             let mut recv_buffer_lock = recv_buffer_writer.lock().await;
//             let bytes_read = NETWORK::try_read(&mut recv_buffer_lock, connection)
//                 .map_err(|err| {
//                     error!("error try_reading from network: {}", err);
//                     NetworkError::ReadWriteError
//                 })?;
//             recv_buffer_lock.commit(bytes_read).unwrap();
//         } else {
//             // Send buffer empty, blocking read
//             let mut recv_buffer_lock = recv_buffer_writer.lock().await;
//             let bytes_read = NETWORK::read(&mut recv_buffer_lock, connection).await
//                 .map_err(|err| {
//                     error!("error try_reading from network: {}", err);
//                     NetworkError::ReadWriteError
//                 })?;
//             recv_buffer_lock.commit(bytes_read).unwrap();
//         }

//         Ok(())
//     }

// }


