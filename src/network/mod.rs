use core::future::Future;

use buffer::{Buffer, BufferReader, BufferWriter};
use embedded_io_async::{ErrorType, Read, ReadReady, Write, WriteReady};

use crate::MqttError;

#[cfg(feature = "std")]
pub mod std;

pub mod fake;

pub trait TryRead: ErrorType {
    fn try_read(&mut self, buf: &mut [u8]) -> impl Future<Output = Result<usize, Self::Error>>;
}

impl <T> TryRead for T where T: Read + ReadReady{
    async fn try_read(&mut self, buf: &mut [u8]) -> Result<usize, Self::Error> {
        if self.read_ready()? {
            self.read(buf).await
        } else {
            Ok(0)
        }
    }
}

pub trait TryWrite: ErrorType {
    fn try_write(&mut self, buf: &[u8]) -> impl Future<Output = Result<usize, Self::Error>>;
}

impl <T> TryWrite for T where T: Write + WriteReady{
    async fn try_write(&mut self, buf: &[u8]) -> Result<usize, Self::Error> {
        if self.write_ready()? {
            self.write(buf).await
        } else {
            Ok(0)
        }
    }
}

pub trait NetworkConnection: Read + Write + TryWrite + TryRead {

    /// Used to establish a connection and reconnect after a connection fail
    fn connect(&mut self) -> impl Future<Output = Result<(), MqttError>>;
    
}

pub struct Network<'a, C: NetworkConnection>(&'a mut C);

impl <'a, C: NetworkConnection> Network<'a, C> {

    pub fn new(inner: &'a mut C) -> Self {
        Self(inner)
    }

    pub async fn connect(&mut self) -> Result<(), MqttError> {
        self.0.connect().await
    }

    pub async fn send_all(&mut self, buffer: &mut impl BufferReader) -> Result<(), MqttError> {
        self.0.write_all(buffer)
            .await.map_err(|e| {
                error!("error sending to network: {}", e);
                MqttError::ConnectionFailed
            })?;

        Ok(())
    }

    /// Send data from the buffer to the network and block is the network is not ready
    pub async fn send<T: AsMut<[u8]> + AsRef<[u8]>>(&mut self, buf: &mut Buffer<T>) -> Result<usize, MqttError> {
        let reader = buf.create_reader();
        let result = self.0.write(&reader[..]).await
            .map_err(|e| {
                error!("error sending to network: {}", e);
                MqttError::ConnectionFailed
            });
        match result {
            Ok(n) => {
                reader.add_bytes_read(n);
                trace!("sent {} bytes to network", n);
                Ok(0)
            },
            Err(e) => {
                Err(e)
            },
        }
    }

    pub async fn try_send<T: AsMut<[u8]> + AsRef<[u8]>>(&mut self, buf: &mut Buffer<T>) -> Result<usize, MqttError> {

        let reader = buf.create_reader();
        let result = self.0.try_write(&reader[..]).await
            .map_err(|e| {
                error!("error try_sending to network: {}", e);
                MqttError::ConnectionFailed}
            );
        match result {
            Ok(n) => {
                reader.add_bytes_read(n);
                trace!("sent {} bytes to network", n);
                Ok(0)
            },
            Err(e) => {
                Err(e)
            },
        }
    }

    pub async fn receive<T: AsMut<[u8]> + AsRef<[u8]>>(&mut self, buf: &mut Buffer<T>) -> Result<usize, MqttError> {
        if ! buf.ensure_remaining_capacity() {
            warn!("cannot receive from network: buffer is full");
            return Ok(0);
        }
        
        let mut writer = buf.create_writer();

        let result = self.0.read(&mut writer).await
            .map_err(|e| {
                error!("error receive from network: {}", e);
                MqttError::ConnectionFailed
            });

        match result {
            Ok(n) => {
                // Error is unwrapped because read() should ensure that not too many bytes are written
                writer.commit(n).unwrap();

                trace!("received {} bytes from network", n);
                Ok(n)
            },
            Err(e) => {
                Err(e)
            },
        }
    }

    pub async fn try_receive<T: AsMut<[u8]> + AsRef<[u8]>>(&mut self, buf: &mut Buffer<T>) -> Result<usize, MqttError> {
        if ! buf.ensure_remaining_capacity() {
            warn!("cannot receive from network: buffer is full");
            return Ok(0);
        }
        
        let mut writer = buf.create_writer();

        let result = self.0.try_read(&mut writer).await
            .map_err(|e| {
                error!("error try_receive from network: {}", e);
                MqttError::ConnectionFailed
            });

        match result {
            Ok(n) => {
                // Error is unwrapped because read() should ensure that not too many bytes are written
                writer.commit(n).unwrap();

                trace!("received {} bytes from network", n);
                Ok(n)
            },
            Err(e) => {
                Err(e)
            },
        }
    }

}
