/// Simple implementation of [`embedded_nal_async`] based on [`tokio`]


use dns_lookup::lookup_host;
use embedded_nal_async::{Dns, TcpConnect};
use tokio::{io::{AsyncReadExt, AsyncWriteExt}, net::{TcpStream}};
use tracing::{error, info, trace};

fn map_std_err(err: std::io::Error) -> embedded_io_async::ErrorKind {

    match err.kind() {
        std::io::ErrorKind::NotFound => embedded_io_async::ErrorKind::NotFound,
        std::io::ErrorKind::PermissionDenied => embedded_io_async::ErrorKind::PermissionDenied,
        std::io::ErrorKind::ConnectionRefused => embedded_io_async::ErrorKind::ConnectionRefused,
        std::io::ErrorKind::ConnectionReset => embedded_io_async::ErrorKind::ConnectionReset,
        std::io::ErrorKind::ConnectionAborted => embedded_io_async::ErrorKind::ConnectionAborted,
        std::io::ErrorKind::NotConnected => embedded_io_async::ErrorKind::NotConnected,
        std::io::ErrorKind::AddrInUse => embedded_io_async::ErrorKind::AddrInUse,
        std::io::ErrorKind::AddrNotAvailable => embedded_io_async::ErrorKind::AddrNotAvailable,
        std::io::ErrorKind::BrokenPipe => embedded_io_async::ErrorKind::BrokenPipe,
        std::io::ErrorKind::AlreadyExists => embedded_io_async::ErrorKind::AlreadyExists,
        std::io::ErrorKind::InvalidInput => embedded_io_async::ErrorKind::InvalidInput,
        std::io::ErrorKind::InvalidData => embedded_io_async::ErrorKind::InvalidData,
        std::io::ErrorKind::TimedOut => embedded_io_async::ErrorKind::TimedOut,
        std::io::ErrorKind::WriteZero => embedded_io_async::ErrorKind::WriteZero,
        std::io::ErrorKind::Interrupted => embedded_io_async::ErrorKind::Interrupted,
        std::io::ErrorKind::Unsupported => embedded_io_async::ErrorKind::Unsupported,
        std::io::ErrorKind::OutOfMemory => embedded_io_async::ErrorKind::OutOfMemory,
        _ => embedded_io_async::ErrorKind::Other,
    }
}

pub struct TestConnection(TcpStream);

impl embedded_io_async::Read for TestConnection {
    async fn read(&mut self, buf: &mut [u8]) -> Result<usize, Self::Error> {
        match self.0.read(buf).await {
            Ok(n) => {
                trace!("read {} bytes from test network", n);
                Ok(n)
            },
            Err(err) => {
                error!("error reading from test network: {}", err);
                Err(map_std_err(err))
            }
        }
    }
}

impl embedded_io_async::Write for TestConnection {
    async fn write(&mut self, buf: &[u8]) -> Result<usize, Self::Error> {
        match self.0.write(buf).await {
            Ok(n) => {
                trace!("wrote {} bytes to test network", n);
                Ok(n)
            },
            Err(err) => {
                error!("error writing to test network: {}", err);
                Err(map_std_err(err))
            }
        }
    }

    async fn flush(&mut self) -> Result<(), Self::Error> {
        match self.0.flush().await {
            Ok(()) => {
                trace!("flushed test network");
                Ok(())
            },
            Err(err) => {
                error!("error flushing test network: {}", err);
                Err(map_std_err(err))
            }
        }
    }
}

impl embedded_io_async::ErrorType for TestConnection {
    type Error = embedded_io_async::ErrorKind;
}

#[derive(Clone)]
pub struct TestNetwork;

impl Dns for TestNetwork {
    type Error = embedded_io_async::ErrorKind;

    async fn get_host_by_name(
            &self,
            host: &str,
            addr_type: embedded_nal_async::AddrType,
        ) -> Result<std::net::IpAddr, Self::Error> {
        
        tracing::info!("looking up addr for {}", host);
        let host = String::from(host);
        tokio::task::spawn_blocking(move || {
            // This should better be an async lookup but it is fine for testing
            let addrs = lookup_host(&host)
                .map_err(map_std_err)?;

            for addr in addrs {
                let is_ok = match addr_type {
                    embedded_nal_async::AddrType::IPv4 => addr.is_ipv4(),
                    embedded_nal_async::AddrType::IPv6 => addr.is_ipv6(),
                    embedded_nal_async::AddrType::Either => true,
                };
                if is_ok {
                    info!("resolved {} to {}", &host, &addr);
                    return Ok(addr);
                } else {
                    tracing::warn!("ignoring addr {} for {}", addr, host);
                }
            }
            error!("could not resolve {}", host);
            Err(embedded_io_async::ErrorKind::NotFound)
        }).await.unwrap()
    }

    async fn get_host_by_address(
            &self,
            _addr: std::net::IpAddr,
            _result: &mut [u8],
        ) -> Result<usize, Self::Error> {
        // not needed for testing, so left unimplemented
        unimplemented!()
    }
}

impl TcpConnect for TestNetwork {
    type Error = embedded_io_async::ErrorKind;

    type Connection<'a> = TestConnection;

    async fn connect<'a>(&'a self, remote: std::net::SocketAddr) -> Result<Self::Connection<'a>, Self::Error> {
        let stream = TcpStream::connect(remote).await
            .map_err(map_std_err)?;
        Ok(TestConnection(stream))
    }
}


