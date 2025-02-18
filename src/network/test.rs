extern crate std;

use core::{future::Future, net::SocketAddr};
use embedded_io_async::Write;
use embedded_io_async::{ErrorKind, ErrorType, Read};
use tokio::net::TcpStream;
use tokio::io::AsyncReadExt;
use tokio::io::AsyncWriteExt;

use crate::MqttError;


pub struct TestNetworkConnection {
    stream: Option<TcpStream>,
    addr: SocketAddr
}

impl TestNetworkConnection {
    pub fn new(addr: SocketAddr) -> Self {
        Self {
            stream: None,
            addr
        }
    }
}
// Read + Write + WriteReady + ReadReady

impl ErrorType for TestNetworkConnection {
    type Error = embedded_io_async::ErrorKind;
}

impl super::TryRead for TestNetworkConnection {
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

impl super::TryWrite for TestNetworkConnection {
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

impl Read for TestNetworkConnection {
    async fn read(&mut self, buf: &mut [u8]) -> Result<usize, Self::Error> {
        self.stream.as_mut().ok_or(ErrorKind::Other)?
            .read(buf).await
            .map_err(|_| ErrorKind::Other)
    }
}

impl Write for TestNetworkConnection {
    async fn write(&mut self, buf: &[u8]) -> Result<usize, Self::Error> {
        self.stream.as_mut().ok_or(ErrorKind::Other)?
            .write(buf).await
            .map_err(|_| ErrorKind::Other)
    }
}

impl super::NetworkConnection for TestNetworkConnection {
    fn connect(&mut self) -> impl Future<Output = Result<(), crate::MqttError>> + Send {
        async move {
            let stream = TcpStream::connect(self.addr).await
                .map_err(|_| MqttError::ConnectionFailed)?;
            self.stream = Some(stream);
            Ok(())
        }
    }
}