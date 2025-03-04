extern crate std;

// #[cfg(all(feature = "defmt", feature = "std"))]
// compile_error!("std cannot be logged with defmt!");

use embedded_io_async::Write;
use embedded_io_async::{ErrorKind, ErrorType, Read};
use tokio::net::{TcpStream, ToSocketAddrs};
use tokio::io::AsyncReadExt;
use tokio::io::AsyncWriteExt;

use crate::Debug2Format;

use super::NetworkError;

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

impl <T: ToSocketAddrs> Unpin for StdNetworkConnection<T> {}

impl <T: ToSocketAddrs> ErrorType for StdNetworkConnection<T> {
    type Error = embedded_io_async::ErrorKind;
}

impl <T: ToSocketAddrs> super::TryRead for StdNetworkConnection<T> {
    async fn try_read(&mut self, buf: &mut [u8]) -> Result<usize, Self::Error> {
        let result = self.stream.as_mut().ok_or(ErrorKind::Other)?
            .try_read(buf);

        match result {
            Ok(n) => {
                if n == 0 && ! buf.is_empty(){
                    warn!("std net try_read: read 0 bytes");
                    Err(ErrorKind::ConnectionReset)
                } else {
                    Ok(n)
                }
            },
            Err(e) => {
                if e.kind() == tokio::io::ErrorKind::WouldBlock {
                    trace!("try_read: network would block");
                    Ok(0) 
                } else { 
                    error!("error try_reading from std net: {}", Debug2Format(e));
                    Err(ErrorKind::Other) 
                }
            }
        }
    }
}

impl <T: ToSocketAddrs> super::TryWrite for StdNetworkConnection<T> {
    async fn try_write(&mut self, buf: &[u8]) -> Result<usize, Self::Error> {
        let result = self.stream.as_mut().ok_or(ErrorKind::Other)?
            .try_write(buf);

        match result {
            Ok(n) => {
                if n == 0 && ! buf.is_empty(){
                    warn!("std net try_write: 0 written bytes");
                    Err(ErrorKind::ConnectionReset)
                } else {
                    Ok(n)
                }
            },
            Err(e) => {
                if e.kind() == tokio::io::ErrorKind::WouldBlock {
                    trace!("try_write: network would block");
                    Ok(0) 
                } else { 
                    error!("error try_writing to std net: {:?}", Debug2Format(e));
                    Err(ErrorKind::Other) 
                }
            }
        }
    }
}

impl <T: ToSocketAddrs> Read for StdNetworkConnection<T> {
    async fn read(&mut self, buf: &mut [u8]) -> Result<usize, Self::Error> {
        self.stream.as_ref().ok_or(ErrorKind::Other)?.readable().await
        .map_err(|e| {
            error!("error waiting for std net to be readable: {}", Debug2Format(e));
            ErrorKind::Other
        })?;

        let n = self.stream.as_mut().ok_or(ErrorKind::Other)?
            .read(buf).await
            .map_err(|e| {
                error!("error reading from std net: {}", Debug2Format(e));
                ErrorKind::Other
            })?;

        if n == 0 && ! buf.is_empty(){
            warn!("std net read: read 0 bytes");
            return Err(ErrorKind::ConnectionReset);
        }
        Ok(n)
    }
}

impl <T: ToSocketAddrs> Write for StdNetworkConnection<T> {
    async fn write(&mut self, buf: &[u8]) -> Result<usize, Self::Error> {
        let n = self.stream.as_mut().ok_or(ErrorKind::Other)?
            .write(buf).await
            .map_err(|e| {
                error!("error writing to std net: {}", Debug2Format(e));
                ErrorKind::Other
            })?;

        if n == 0 && ! buf.is_empty(){
            warn!("std net write: 0 written bytes");
            return Err(ErrorKind::ConnectionReset);
        }
        Ok(n)
    }
}

impl <T: ToSocketAddrs> super::NetworkConnection for StdNetworkConnection<T> {
    async fn connect(&mut self) -> Result<(), NetworkError> {
        let stream: TcpStream = TcpStream::connect(&self.addr).await
            .map_err(|_| NetworkError::ConnectionFailed)?;
        self.stream = Some(stream);
        Ok(())
    }
}