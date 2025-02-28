

use core::mem;

use embassy_futures::join::join;
use embassy_net::{dns::{DnsQueryType, DnsSocket}, tcp::TcpSocket, IpAddress, IpEndpoint, Stack};
use embedded_io_async::{ErrorKind, ErrorType, Read, ReadReady, Write, WriteReady};
use heapless::String;

use crate::{NetworkConnection, NetworkError, TryRead, TryWrite};

pub struct EmbassyConnectionStackResources<const N: usize> {
    rx_buffer: [u8; N],
    tx_buffer: [u8; N]
}

impl <const N: usize> EmbassyConnectionStackResources<N> {
    pub fn new() -> Self {
        Self {
            rx_buffer: [0; N],
            tx_buffer: [0; N]
        }
    }

    pub fn borrow<'a>(&'a mut self) -> EmbassyConnectionResources<'a> {
        EmbassyConnectionResources{
            rx_buffer: &mut self.rx_buffer[..],
            tx_buffer: &mut self.tx_buffer[..]
        }
    }
}

pub struct EmbassyConnectionResources<'a> {
    rx_buffer: &'a mut [u8],
    tx_buffer: &'a mut [u8]
}

impl <'a> EmbassyConnectionResources<'a> {

    fn borrow<'b>(&'b mut self) -> (&'b mut [u8], &'b mut [u8]) {
        (self.rx_buffer, self.tx_buffer)
    }

    fn borrowx(&mut self) -> (&'static mut [u8], &'static mut [u8]) {
        let ( rx_buffer, tx_buffer ) = self.borrow();
        unsafe {
            
            (
                mem::transmute(rx_buffer),
                mem::transmute(tx_buffer)
            )
        }
    }
}

pub struct EmbassyNetworkConnection<'a> {
    socket: Option<TcpSocket<'a>>,
    stack: Stack<'a>,
    resources: EmbassyConnectionResources<'a>,
    host: String<64>,
    port: u16
}

impl <'a> EmbassyNetworkConnection<'a> {

    async fn dns_resolve(&self, hostname: &str) -> Result<IpAddress, NetworkError> {
        let dns_client = DnsSocket::new(self.stack);

        let ip_v6_future = async {
            let result = dns_client.query(hostname, DnsQueryType::Aaaa).await;

            match result.to_owned() {
                Ok(addrs) => Ok(addrs.into_iter().next()),
                Err(embassy_net::dns::Error::InvalidName) => Ok(None),
                Err(e) => Err(e)
            }
        };

        let ip_v4_future = async {
            let result = dns_client.query(hostname, DnsQueryType::A).await;

            match result {
                Ok(addrs) => Ok(addrs.into_iter().next()),
                Err(embassy_net::dns::Error::InvalidName) => Ok(None),
                Err(e) => Err(e)
            }
        };
        
        let (r_v4, r_v6) = join(ip_v4_future, ip_v6_future).await;
        let r_v4 = r_v4.map_err(|e| {
            error!("DNS A request failed: {}", e);
            NetworkError::DnsFailed
        })?;

        let r_v6 = r_v6.map_err(|e| {
            error!("DNS AAAA request failed: {}", e);
            NetworkError::DnsFailed
        })?;

        r_v4.or(r_v6).ok_or(NetworkError::HostNotFound)
    }

    async fn connect_intern(&mut self, rx_buffer: &'a mut [u8], tx_buffer: &'a mut [u8]) -> Result<(), NetworkError> {
        let addr = self.dns_resolve(&self.host).await?;

        if let Some(mut socket) = self.socket.take() {
            trace!("closing existing socket");
            socket.close();
        }

        let mut socket = TcpSocket::new(self.stack, rx_buffer, tx_buffer);
        // socket.set_keep_alive(Some(Duration::from_secs(10)));

        let endpoint = IpEndpoint{
            addr,
            port: self.port
        };
        
        info!("connecting...");
        let _connection = socket.connect(endpoint).await.map_err(|e| {
            error!("tcp connection failed: {}", e);
            NetworkError::from(e)
        })?;

        self.socket = Some(socket);
        
        Ok(())
    }

}

impl <'a> Unpin for EmbassyNetworkConnection<'a> {}

impl <'a> ErrorType for EmbassyNetworkConnection<'a> {
    type Error = ErrorKind;
}

impl <'a> Write for EmbassyNetworkConnection<'a> {
    async fn write(&mut self, buf: &[u8]) -> Result<usize, Self::Error> {
        let socket = self.socket.as_mut().ok_or(ErrorKind::ConnectionAborted)?;
        socket.write(buf).await
            .map_err(|_| ErrorKind::ConnectionReset)
    }
}

impl <'a> Read for EmbassyNetworkConnection<'a> {
    async fn read(&mut self, buf: &mut [u8]) -> Result<usize, Self::Error> {
        let socket = self.socket.as_mut().ok_or(ErrorKind::ConnectionAborted)?;
        socket.read(buf).await
            .map_err(|_| ErrorKind::ConnectionReset)
    }
}

impl <'a> TryRead for EmbassyNetworkConnection<'a> {
    async fn try_read(&mut self, buf: &mut [u8]) -> Result<usize, Self::Error> {
        let socket = self.socket.as_mut().ok_or(ErrorKind::ConnectionAborted)?;
        if socket.read_ready().map_err(|_| ErrorKind::ConnectionReset)? {
            self.read(buf).await
        } else {
            Ok(0)
        }
    }
}

impl <'a> TryWrite for EmbassyNetworkConnection<'a> {
    async fn try_write(&mut self, buf: &[u8]) -> Result<usize, Self::Error> {
        let socket = self.socket.as_mut().ok_or(ErrorKind::ConnectionAborted)?;
        if socket.write_ready().map_err(|_| ErrorKind::ConnectionReset)? {
            self.write(buf).await
        } else {
            Ok(0)
        }
    }
}

impl <'a> NetworkConnection for EmbassyNetworkConnection<'a> {
    async fn connect(&mut self) -> Result<(), crate::NetworkError> {
        let ( rx_buffer, tx_buffer ) = self.resources.borrowx();
        self.connect_intern(rx_buffer, tx_buffer).await
    }
}
