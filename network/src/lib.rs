#![cfg_attr(not(feature = "std"), no_std)]

use core::future::Future;

use buffer::{Buffer, BufferReader, BufferWriter, ReadWrite};
use embedded_io_async::{ErrorType, Read, ReadReady, Write, WriteReady};
use thiserror::Error;


pub(crate) mod fmt;
use fmt::Debug2Format;


#[cfg(feature = "embassy")]
pub mod embassy;

#[cfg(feature = "std")]
pub mod std;

pub mod fake;

pub mod mqtt;

#[derive(Debug, PartialEq, Clone, Error)]
#[cfg_attr(feature = "defmt", derive(defmt::Format))]
pub enum NetworkError {

    #[error("Could not find host")]
    HostNotFound,

    #[error("DNS Request failed")]
    DnsFailed,

    #[error("sending / retrieving from network failed")]
    ConnectionFailed,

    #[cfg(feature = "embassy")]
    #[error("failed to connect to tcp endpoint")]
    ConnectError(embassy_net::tcp::ConnectError),
}

#[cfg(feature = "embassy")]
impl From<embassy_net::tcp::ConnectError> for NetworkError {
    fn from(value: embassy_net::tcp::ConnectError) -> Self {
        Self::ConnectError(value)
    }
}



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

pub trait NetworkConnection: Read + Write + TryWrite + TryRead + ErrorType {

    /// Used to establish a connection and reconnect after a connection fail
    fn connect(&mut self) -> impl Future<Output = Result<(), NetworkError>>;
    
}

pub trait NetwordSendReceive {
    fn send_all(&mut self, buffer: &mut impl BufferReader) -> impl Future<Output = Result<(), NetworkError>>;
    fn send<T: AsMut<[u8]> + AsRef<[u8]>>(&mut self, buf: &mut Buffer<T>) -> impl Future<Output = Result<usize, NetworkError>>;
    fn try_send<T: AsMut<[u8]> + AsRef<[u8]>>(&mut self, buf: &mut Buffer<T>) -> impl Future<Output = Result<usize, NetworkError>>;

    fn receive<T: AsMut<[u8]> + AsRef<[u8]>>(&mut self, buf: &mut Buffer<T>) -> impl Future<Output = Result<usize, NetworkError>>;
    fn try_receive<T: AsMut<[u8]> + AsRef<[u8]>>(&mut self, buf: &mut Buffer<T>) -> impl Future<Output = Result<usize, NetworkError>>;

}

impl <C> NetwordSendReceive for C where C: NetworkConnection {

    async fn send_all(&mut self, buffer: &mut impl BufferReader) -> Result<(), NetworkError> {
        self.write_all(buffer)
            .await.map_err(|e| {
                error!("error sending to network: {}", Debug2Format(e));
                NetworkError::ConnectionFailed
            })?;

        Ok(())
    }

    /// Send data from the buffer to the network and block is the network is not ready
    async fn send<T: AsMut<[u8]> + AsRef<[u8]>>(&mut self, buf: &mut Buffer<T>) -> Result<usize, NetworkError> {
        let reader = buf.create_reader();
        let result = self.write(&reader[..]).await
            .map_err(|e| {
                error!("error sending to network: {}", Debug2Format(e));
                NetworkError::ConnectionFailed
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

    async fn try_send<T: AsMut<[u8]> + AsRef<[u8]>>(&mut self, buf: &mut Buffer<T>) -> Result<usize, NetworkError> {

        let reader = buf.create_reader();
        let result = self.try_write(&reader[..]).await
            .map_err(|e| {
                error!("error try_sending to network: {}", Debug2Format(e));
                NetworkError::ConnectionFailed}
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

    async fn receive<T: AsMut<[u8]> + AsRef<[u8]>>(&mut self, buf: &mut Buffer<T>) -> Result<usize, NetworkError> {
        if ! buf.ensure_remaining_capacity() {
            warn!("cannot receive from network: buffer is full");
            return Ok(0);
        }
        
        let mut writer = buf.create_writer();

        let result = self.read(&mut writer).await
            .map_err(|e| {
                error!("error receive from network: {}", Debug2Format(e));
                NetworkError::ConnectionFailed
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

    async fn try_receive<T: AsMut<[u8]> + AsRef<[u8]>>(&mut self, buf: &mut Buffer<T>) -> Result<usize, NetworkError> {
        if ! buf.ensure_remaining_capacity() {
            warn!("cannot receive from network: buffer is full");
            return Ok(0);
        }
        
        let mut writer = buf.create_writer();

        let result = self.try_read(&mut writer).await
            .map_err(|e| {
                error!("error try_receive from network: {}", Debug2Format(e));
                NetworkError::ConnectionFailed
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
