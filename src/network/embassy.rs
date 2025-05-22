

use core::mem;

use embassy_futures::join::join;
use embassy_net::{dns::{DnsQueryType, DnsSocket}, tcp::TcpSocket, IpAddress, IpEndpoint, Stack};
use embedded_io_async::{ErrorKind, ErrorType, Read, ReadReady, Write, WriteReady};

use crate::{NetworkConnection, NetworkError, TryRead, TryWrite};

/// struct that contains the rx buffer and tx buffer for the tcp connection
/// 
/// the buffers are of size N
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

    /// borrow the rx buffer and tx buffer
    pub fn borrow<'a>(&'a mut self) -> EmbassyConnectionResources<'a> {
        EmbassyConnectionResources{
            rx_buffer: &mut self.rx_buffer[..],
            tx_buffer: &mut self.tx_buffer[..]
        }
    }

    /*
    pub fn borrow2<'a>(&'a mut self) -> EmbassyConnectionResources2<'a, N> {
        EmbassyConnectionResources2{
            rx_buffer: &mut self.rx_buffer as *mut u8,
            tx_buffer: &mut self.tx_buffer as *mut u8,
            _phantom_data: PhantomData
        }
    }
    */
}

/*
pub struct EmbassyConnectionResources2<'a, const N: usize> {
    rx_buffer: *mut u8,
    tx_buffer: *mut u8,
    _phantom_data: PhantomData<&'a mut u8>
}

impl <'a, const N: usize> EmbassyConnectionResources2<'a, N> {

    pub fn unwrap(&'a mut self) -> (&'a mut [u8], &'a mut [u8]) {
        unsafe {
            (
                slice::from_raw_parts_mut(self.rx_buffer, N),
                slice::from_raw_parts_mut(self.tx_buffer, N)
            )
        }
    }

    unsafe fn unwrap_static(&self) -> (&'static mut [u8], &'static mut [u8]) {
        (
            slice::from_raw_parts_mut(self.rx_buffer, N),
            slice::from_raw_parts_mut(self.tx_buffer, N)
        )

    }
}
*/

/// struct that contains the pointers to the rx buffer and tx buffer
pub struct EmbassyConnectionResources<'a> {
    rx_buffer: &'a mut [u8],
    tx_buffer: &'a mut [u8]
}

impl <'a> EmbassyConnectionResources<'a> {

    fn unwrap<'b>(&'b mut self) -> (&'b mut [u8], &'b mut [u8]) {
        (self.rx_buffer, self.tx_buffer)
    }

    /// unwrap the rx buffer and tx buffer as static references
    /// 
    /// # Safety
    /// 
    /// Concurrent write operations are possible. The user of this function must avoid them.
    unsafe fn unwrap_unsafe(&mut self) -> (&'a mut [u8], &'a mut [u8]) {
        let ( rx_buffer, tx_buffer ) = self.unwrap();
            
        (
            mem::transmute(rx_buffer),
            mem::transmute(tx_buffer)
        )
    }
}

pub struct EmbassyNetworkConnection<'a> {
    socket: Option<TcpSocket<'a>>,
    stack: Stack<'a>,
    resources: EmbassyConnectionResources<'a>,
    host: &'a str,
    port: u16
}

impl <'a> EmbassyNetworkConnection<'a> {

    pub fn new(host: &'a str, port: u16, stack: Stack<'a>, resources: EmbassyConnectionResources<'a>) -> Self {
        Self {
            socket: None,
            stack, resources, 
            host, port
        }
    }

    async fn dns_resolve(&self, hostname: &str) -> Result<IpAddress, NetworkError> {
        let dns_client = DnsSocket::new(self.stack);

        let ip_v6_future = async {
            let result = dns_client.query(hostname, DnsQueryType::Aaaa).await;

            match result {
                Ok(addrs) => {
                    if let Some(addr) = addrs.into_iter().next() {
                        info!("dns aaaa: {} -> {}", hostname, addr);
                        Ok(Some(addr))
                    } else {
                        info!("dns aaaa: {} -> nothing", hostname);
                        Ok(None)
                    }
                },
                Err(embassy_net::dns::Error::InvalidName) => {
                    info!("dns aaaa: {} -> invalid name", hostname);
                    Ok(None)
                },
                Err(e) => Err(e)
            }
        };

        let ip_v4_future = async {
            let result = dns_client.query(hostname, DnsQueryType::A).await;

            match result {
                Ok(addrs) => {
                    if let Some(addr) = addrs.into_iter().next() {
                        info!("dns a: {} -> {}", hostname, addr);
                        Ok(Some(addr))
                    } else {
                        info!("dns a: {} -> nothing", hostname);
                        Ok(None)
                    }
                }
                Err(embassy_net::dns::Error::InvalidName) => {
                    info!("dns a: {} -> invalid name", hostname);
                    Ok(None)
                },
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
        socket.connect(endpoint).await.map_err(|e| {
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
        let ( rx_buffer, tx_buffer ) = unsafe {
            self.resources.unwrap_unsafe()
        };
        self.connect_intern(rx_buffer, tx_buffer).await
    }
}
