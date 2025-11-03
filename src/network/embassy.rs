
use core::{cell::UnsafeCell, future::Future, mem, pin::Pin, ptr, sync::atomic::{AtomicBool, Ordering}, task::{Context, Poll, RawWaker, RawWakerVTable, Waker}};

use embassy_futures::join::join;
use embassy_net::{dns::{DnsQueryType, DnsSocket}, tcp::TcpSocket, IpAddress, IpEndpoint, Stack};

use crate::network::NetworkError;

/// --- Dummy-Waker (no-op) --------------------------------------------------
/// Ein sicherer, no-op RawWaker / Waker, ausreichend um ein Future einmal zu pollen.
unsafe fn raw_waker_clone(_: *const ()) -> RawWaker {
    RawWaker::new(ptr::null(), &RAW_WAKER_VTABLE)
}
unsafe fn raw_waker_wake(_: *const ()) {}
unsafe fn raw_waker_wake_by_ref(_: *const ()) {}
unsafe fn raw_waker_drop(_: *const ()) {}

static RAW_WAKER_VTABLE: RawWakerVTable =
    RawWakerVTable::new(
        raw_waker_clone,
        raw_waker_wake,
        raw_waker_wake_by_ref,
        raw_waker_drop,
    );

fn dummy_waker() -> Waker {
    // RawWaker trägt keine Daten; pointer ist null, VTable verweist auf no-op-ops.
    let raw = RawWaker::new(ptr::null(), &RAW_WAKER_VTABLE);
    // Safety: raw erfüllt die invariants unserer no-op-Implementierung.
    unsafe { Waker::from_raw(raw) }
}

/// --- Variante 1: Für Unpin-Futures (einfach) ------------------------------
pub fn poll_once_unpin<F>(mut fut: F) -> Poll<F::Output>
where
    F: Future + Unpin,
{
    let waker = dummy_waker();
    let mut cx = Context::from_waker(&waker);
    // Pin::new ist sicher, da F: Unpin
    Pin::new(&mut fut).poll(&mut cx)
}

/// --- Variante 2: Für bereits gepinnte Futures ------------------------------
pub fn poll_once_pin<F>(fut: Pin<&mut F>) -> Poll<F::Output>
where
    F: Future,
{
    let waker = dummy_waker();
    let mut cx = Context::from_waker(&waker);
    fut.poll(&mut cx)
}

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
}

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

pub struct EmbassyNetworkConnection<'a>{
    resources_in_use: &'a AtomicBool,
    socket: TcpSocket<'a>
}

impl <'a> Drop for EmbassyNetworkConnection<'a> {
    fn drop(&mut self) {
        self.resources_in_use.store(false, Ordering::Release);
    }
}

pub struct EmbassyNetwork<'a> {
    host: &'a str,
    port: u16,
    stack: Stack<'a>,
    
    resources: UnsafeCell<EmbassyConnectionResources<'a>>,
    resources_in_use: AtomicBool
}

impl <'a> EmbassyNetwork<'a> {

    pub fn new(host: &'a str, port: u16, stack: Stack<'a>, resources: EmbassyConnectionResources<'a>) -> Self {
        Self {
            host,
            port,
            stack,
            resources: UnsafeCell::new(resources),
            resources_in_use: AtomicBool::new(false)
        }
    }

    async fn dns_resolve(&self) -> Result<IpAddress, NetworkError> {
        let dns_client = DnsSocket::new(self.stack);

        let ip_v6_future = async {
            let result = dns_client.query(self.host, DnsQueryType::Aaaa).await;

            match result {
                Ok(addrs) => {
                    if let Some(addr) = addrs.into_iter().next() {
                        info!("dns aaaa: {} -> {}", self.host, addr);
                        Ok(Some(addr))
                    } else {
                        info!("dns aaaa: {} -> nothing", self.host);
                        Ok(None)
                    }
                },
                Err(embassy_net::dns::Error::InvalidName) => {
                    info!("dns aaaa: {} -> invalid name", self.host);
                    Ok(None)
                },
                Err(e) => Err(e)
            }
        };

        let ip_v4_future = async {
            let result = dns_client.query(self.host, DnsQueryType::A).await;

            match result.map(|addrs| addrs.into_iter().next()) {
                Ok(Some(addr)) => {
                    info!("dns a: {} -> {}", self.host, addr);
                    Ok(Some(addr))
                },
                Ok(None) => {
                    info!("dns a: {} -> nothing", self.host);
                        Ok(None)
                },
                Err(embassy_net::dns::Error::InvalidName) => {
                    info!("dns a: {} -> invalid name", self.host);
                    Ok(None)
                },
                Err(e) => Err(e)
            }
        };
        
        let (r_v4, r_v6) = join(ip_v4_future, ip_v6_future).await;
        match (r_v4, r_v6) {
            (_, Ok(Some(v6))) => Ok(v6),
            (Ok(Some(v4)), _) => Ok(v4),
            (_, Ok(None)) |
            (Ok(None), _) => Err(NetworkError::HostNotFound),
            (Err(err), _) => Err(err.into()),
        }
    }

}

impl <'a> super::PlattformNetwork for EmbassyNetwork<'a> {
    type Connection<'r> = EmbassyNetworkConnection<'r>;

    fn write<'c>(buf: &'c [u8], connection: &'c mut Self::Connection<'_>) -> impl Future<Output = Result<usize, NetworkError>> + 'c {
        async {
            connection.socket.write(buf).await.map_err(|err| err.into())
        }
    }

    fn try_write(buf: &[u8], connection: &mut Self::Connection<'_>) -> Result<usize, NetworkError> {
        let mut write_future = connection.socket.write(buf);
        let write_future = Pin::new(&mut write_future);
        match poll_once_pin(write_future) {
            Poll::Ready(result) => result.map_err(|err| err.into()),
            Poll::Pending => Ok(0),
        } 
    }

    fn flush<'c>(connection: &'c mut Self::Connection<'_>) -> impl Future<Output = Result<(), super::NetworkError>> + 'c {
        async {
            connection.socket.flush().await.map_err(|err| err.into())
        }
    }

    fn read<'c>(buf: &'c mut[u8], connection: &'c mut Self::Connection<'_>) -> impl Future<Output = Result<usize, NetworkError>> + 'c {
        async {
            connection.socket.read(buf).await.map_err(|err| err.into())
        }
    }

    fn try_read(buf: &mut[u8], connection: &mut Self::Connection<'_>) -> Result<usize, NetworkError> {
        let mut read_future = connection.socket.read(buf);
        let read_future = Pin::new(&mut read_future);
        match poll_once_pin(read_future) {
            Poll::Ready(result) => result.map_err(|err| err.into()),
            Poll::Pending => Ok(0),
        }
    }

    fn close(mut connection: Self::Connection<'_>) {
        connection.socket.close();
    }

    async fn connect<'r>(&'r self) -> Result<Self::Connection<'r>, NetworkError> {
        let ( rx_buffer, tx_buffer ) = unsafe {
            let resources = self.resources.get();
            (*resources).unwrap_unsafe() 
        };

        let was_in_use = self.resources_in_use.swap(true, Ordering::AcqRel);
        assert!(!was_in_use);

        let addr = self.dns_resolve().await?;

        let mut socket = TcpSocket::new(self.stack, rx_buffer, tx_buffer);

        let endpoint = IpEndpoint {
            addr,
            port: self.port
        };
        
        info!("connecting...");
        socket.connect(endpoint).await?;
        info!("connected");

        Ok(Self::Connection{
            socket,
            resources_in_use: &self.resources_in_use
        })
    }
}