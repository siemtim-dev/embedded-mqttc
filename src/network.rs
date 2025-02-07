use core::future::Future;

use buffer::{Buffer, BufferReader, BufferWriter};
use embedded_io_async::{ErrorType, Read, ReadReady, Write, WriteReady};

use crate::MqttError;

pub trait NetworkConnection: Read + Write + WriteReady + ReadReady + ErrorType<Error = MqttError> {

    /// Used to establish a connection and reconnect after a connection fail
    fn connect(&mut self) -> impl Future<Output = Result<(), MqttError>> + Send;
    
}

pub struct Network<C: NetworkConnection>(pub(crate) C);

impl <C: NetworkConnection> Network<C> {

    /// Send data from the buffer to the network and block is the network is not ready
    pub async fn send<T: AsMut<[u8]> + AsRef<[u8]>>(&mut self, buf: &mut Buffer<T>) -> Result<usize, MqttError> {
        if ! buf.has_remaining_len() {
            trace!("no data to send to network");
            return Ok(0);
        }

        let reader = buf.create_reader();
        let result = self.0.write(&reader[..]).await;
        match result {
            Ok(n) => {
                reader.add_bytes_read(n);
                trace!("sent {} bytes to network", n);
                Ok(0)
            },
            Err(e) => {
                error!("error writing to network: {}", e);
                Err(e)
            },
        }
    }

    pub async fn try_send<T: AsMut<[u8]> + AsRef<[u8]>>(&mut self, buf: &mut Buffer<T>) -> Result<usize, MqttError> {
        if ! self.0.write_ready()? {
            trace!("network not write_ready: skip writing to network");
            return Ok(0);
        }

        self.send(buf).await
    }

    pub async fn receive<T: AsMut<[u8]> + AsRef<[u8]>>(&mut self, buf: &mut Buffer<T>) -> Result<usize, MqttError> {
        if ! buf.ensure_remaining_capacity() {
            warn!("cannot receive from network: buffer is full");
            return Ok(0);
        }
        
        let mut writer = buf.create_writer();

        let result = self.0.read(&mut writer).await;

        match result {
            Ok(n) => {
                // Error is unwrapped because read() should ensure that not too many bytes are written
                writer.commit(n).unwrap();

                trace!("received {} bytes from network", n);
                Ok(n)
            },
            Err(e) => {
                error!("error reading from network: {}", e);
                Err(e)
            },
        }
    }

    pub async fn try_receive<T: AsMut<[u8]> + AsRef<[u8]>>(&mut self, buf: &mut Buffer<T>) -> Result<usize, MqttError> {
        if ! self.0.read_ready()? {
            trace!("network not read_ready: skip reading from network");
            return Ok(0);
        }

        self.receive(buf).await
    }

}
