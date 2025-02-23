extern crate std;

use embedded_io_async::Write;
use embedded_io_async::{ErrorKind, ErrorType, Read};
use tokio::net::{TcpStream, ToSocketAddrs};
use tokio::io::AsyncReadExt;
use tokio::io::AsyncWriteExt;

use crate::MqttError;

pub struct StdNetworkConnection<T: ToSocketAddrs> {
    stream: Option<TcpStream>,
    addr: T
}

impl <T: ToSocketAddrs> StdNetworkConnection<T> {
    pub fn new(addr: T) -> Self {
        Self {
            stream: None,
            addr
        }
    }
}
// Read + Write + WriteReady + ReadReady

impl <T: ToSocketAddrs> ErrorType for StdNetworkConnection<T> {
    type Error = embedded_io_async::ErrorKind;
}

impl <T: ToSocketAddrs> super::TryRead for StdNetworkConnection<T> {
    async fn try_read(&mut self, buf: &mut [u8]) -> Result<usize, Self::Error> {
        let result = self.stream.as_mut().ok_or(ErrorKind::Other)?
            .try_read(buf);

        match result {
            Ok(n) => Ok(n),
            Err(e) => {
                if e.kind() == tokio::io::ErrorKind::WouldBlock { Ok(0) } else { Err(ErrorKind::Other) }
            }
        }
    }
}

impl <T: ToSocketAddrs> super::TryWrite for StdNetworkConnection<T> {
    async fn try_write(&mut self, buf: &[u8]) -> Result<usize, Self::Error> {
        let result = self.stream.as_mut().ok_or(ErrorKind::Other)?
            .try_write(buf);

        match result {
            Ok(n) => Ok(n),
            Err(e) => {
                if e.kind() == tokio::io::ErrorKind::WouldBlock { Ok(0) } else { Err(ErrorKind::Other) }
            }
        }
    }
}

impl <T: ToSocketAddrs> Read for StdNetworkConnection<T> {
    async fn read(&mut self, buf: &mut [u8]) -> Result<usize, Self::Error> {
        self.stream.as_mut().ok_or(ErrorKind::Other)?
            .read(buf).await
            .map_err(|_| ErrorKind::Other)
    }
}

impl <T: ToSocketAddrs> Write for StdNetworkConnection<T> {
    async fn write(&mut self, buf: &[u8]) -> Result<usize, Self::Error> {
        self.stream.as_mut().ok_or(ErrorKind::Other)?
            .write(buf).await
            .map_err(|_| ErrorKind::Other)
    }
}

impl <T: ToSocketAddrs> super::NetworkConnection for StdNetworkConnection<T> {
    async fn connect(&mut self) -> Result<(), crate::MqttError> {
        let stream: TcpStream = TcpStream::connect(&self.addr).await
            .map_err(|_| MqttError::ConnectionFailed)?;
        self.stream = Some(stream);
        Ok(())
    }
}