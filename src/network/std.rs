use tokio::{io::{AsyncReadExt, AsyncWriteExt}, net::TcpStream};
use core::future::Future;
use std::io::ErrorKind;

use crate::network::NetworkError;

fn map_result<T>(result: Result<T, std::io::Error>) -> Result<T, NetworkError> {
    result.map_err(|err| map_err(err))
}

fn map_err(err: std::io::Error) -> NetworkError {
    match err.kind() {
        ErrorKind::ConnectionRefused => todo!(),
        ErrorKind::ConnectionReset => NetworkError::ConnectionReset,
        ErrorKind::HostUnreachable => todo!(),
        ErrorKind::NetworkUnreachable => todo!(),
        ErrorKind::ConnectionAborted => todo!(),
        ErrorKind::NotConnected => todo!(),
        ErrorKind::AddrInUse => todo!(),
        ErrorKind::AddrNotAvailable => todo!(),
        ErrorKind::NetworkDown => todo!(),
        
        ErrorKind::TimedOut => NetworkError::Timeout,

        ErrorKind::PermissionDenied |
        ErrorKind::WouldBlock |
        ErrorKind::StaleNetworkFileHandle |
        ErrorKind::InvalidInput |
        ErrorKind::InvalidData |
        ErrorKind::WriteZero |
        ErrorKind::ResourceBusy |
        ErrorKind::Interrupted |
        ErrorKind::Unsupported |
        ErrorKind::UnexpectedEof |
        ErrorKind::OutOfMemory |
        ErrorKind::Other => {
            warn!("unexpected networ error {}", err);
            NetworkError::Unexpected
        },

        e => panic!("unexpected error kind from network: {}", e),
    }
}

pub struct StdNetwork<'a> {
    host: &'a str,
    port: u16
}

impl<'a> StdNetwork<'a> {

    pub fn new(host: &'a str, port: u16) -> Self {
        Self {
            host, port
        }
    }

}

impl <'a> super::PlattformNetwork for StdNetwork<'a> {
    type Connection<'r> = TcpStream;

    fn write<'b>(buf: &'b [u8], connection: &'b mut Self::Connection<'_>) -> impl Future<Output = Result<usize, NetworkError>> + 'b {
        async {
            map_result(connection.write(buf).await)
        }
    }

    fn try_write(buf: &[u8], connection: &mut Self::Connection<'_>) -> Result<usize, NetworkError> {
        match connection.try_write(buf) {
            Err(err) if err.kind() == ErrorKind::WouldBlock => Ok(0),
            Ok(n) => Ok(n),
            Err(err) => Err(map_err(err)),
        }
    }

    fn flush<'b>(connection: &'b mut Self::Connection<'_>) -> impl Future<Output = Result<(), NetworkError>> + 'b {
        async {
            map_result(connection.flush().await)
        }
    }

    fn read<'b>(buf: &'b mut[u8], connection: &'b mut Self::Connection<'_>) -> impl Future<Output = Result<usize, NetworkError>> + 'b {
        async {
            map_result(connection.read(buf).await)
        }
    }

    fn try_read(buf: &mut[u8], connection: &mut Self::Connection<'_>) -> Result<usize, NetworkError> {
        match connection.try_read(buf) {
            Err(err) if err.kind() == ErrorKind::WouldBlock => Ok(0),
            Ok(n) => Ok(n),
            Err(err) => Err(map_err(err)),
        }
    }

    fn close(_connection: Self::Connection<'_>) {}

    async fn connect(&self) -> Result<Self::Connection<'_>, NetworkError> {
        let addr = (self.host, self.port);
        let stream = map_result(TcpStream::connect(addr).await)?;
        Ok(stream)
    }   
}