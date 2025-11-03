use core::{future::Future, pin::Pin, sync::atomic::{AtomicUsize, Ordering}, task::{Context, Poll}};
use std::sync::Mutex;

use embassy_sync::waitqueue::WakerRegistration;
use mqttrs2::{Packet, decode_slice_with_len, encode_slice};

use crate::network::PlattformNetwork;


extern crate std;

pub struct TestConnection<'a> {
    written: &'a Mutex<Vec<u8>>,
    connections_closed: &'a AtomicUsize,
    readable: &'a Mutex<Vec<u8>>,
    read_waker: &'a Mutex<WakerRegistration>,
}

pub struct TestNetwork {
    pub written: Mutex<Vec<u8>>,
    pub connections_created: AtomicUsize,
    pub connections_closed: AtomicUsize,

    pub readable: Mutex<Vec<u8>>,
    read_waker: Mutex<WakerRegistration>,
}

impl TestNetwork {

    pub fn new() -> Self {
        Self {
            written: Mutex::new(Vec::new()),
            connections_created: AtomicUsize::new(0),
            connections_closed: AtomicUsize::new(0),
            readable: Mutex::new(Vec::new()),
            read_waker: Mutex::new(WakerRegistration::new()),
        }
    }

    pub fn assert_packet_written<F, U>(&self, f: F) -> U where F: FnOnce(Packet<'_>) -> U {
        let mut lock = self.written.lock().unwrap();
        let (bytes_read, packet) = decode_slice_with_len(&lock).unwrap()
            .expect("expected a packet written, but there is none");

        let result = f(packet);
        lock.drain(0..bytes_read);
        result
    }

    pub fn assert_nothing_written(&self) {
        let lock = self.written.lock().unwrap();
        assert!(lock.is_empty(), "asseet nothing written");
    }

    pub fn add_packet_to_receive(&self, packet: &Packet<'_>) {
        let mut lock = self.readable.lock().unwrap();
        let mut buf = [0; 1024];
        let packet_len = encode_slice(packet, &mut buf).unwrap();
        lock.extend_from_slice(&buf[0..packet_len]);
        self.read_waker.lock().unwrap().wake();
    }

}

pub struct TestReadFuture<'a, 'b> {
    buf: &'a mut [u8],
    connection: &'a mut TestConnection<'b>
}

impl<'a, 'b> TestReadFuture<'a, 'b> {
    fn borrow<'c>(&'c mut self) -> (&'c mut [u8], &'c mut TestConnection<'b>) where 'a: 'c{
        (self.buf, self.connection)
    }
}

impl<'a, 'b> Future for TestReadFuture<'a, 'b> {
    type Output = Result<usize, super::NetworkError>;

    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {

        let (buf, connection) = self.borrow();

        match TestNetwork::try_read(buf, connection) {
            Ok(0) => {
                self.connection.read_waker.lock().unwrap().register(cx.waker());
                Poll::Pending
            }
            Ok(n) => Poll::Ready(Ok(n)),
            Err(err) => Poll::Ready(Err(err)),
        }
    }
}


impl PlattformNetwork for TestNetwork {
    type Connection<'c> = TestConnection<'c>;

    fn write<'a>(buf: &'a [u8], connection: &'a mut Self::Connection<'_>) -> impl Future<Output = Result<usize, super::NetworkError>> + 'a {
        async {
            connection.written.lock().unwrap().extend_from_slice(buf);
            Ok(buf.len())
        }
    }

    fn try_write(buf: &[u8], connection: &mut Self::Connection<'_>) -> Result<usize, super::NetworkError> {
        connection.written.lock().unwrap().extend_from_slice(buf);
        Ok(buf.len())
    }

    fn flush<'a>(_connection: &'a mut Self::Connection<'_>) -> impl Future<Output = Result<(), super::NetworkError>> + 'a {
        async { Ok(()) }
    }

    fn read<'a>(buf: &'a mut[u8], connection: &'a mut Self::Connection<'_>) -> impl Future<Output = Result<usize, super::NetworkError>> + 'a {
        TestReadFuture{
            buf,
            connection
        }
    }

    fn try_read(buf: &mut[u8], connection: &mut Self::Connection<'_>) -> Result<usize, super::NetworkError> {
        let mut lock = connection.readable.lock().unwrap();

        if buf.len() > lock.len() {
            buf[..lock.len()].copy_from_slice(&lock);
            let n = lock.len();
            lock.clear();
            Ok(n)
        } else {
            buf.copy_from_slice(&lock[..buf.len()]);
            let n = buf.len();
            lock.drain(0..n);
            Ok(n)
        }
    }

    fn close(connection: Self::Connection<'_>) {
        connection.connections_closed.fetch_add(1, Ordering::AcqRel);
    }

    async fn connect<'a>(&'a self) -> Result<Self::Connection<'a>, super::NetworkError> {
        self.connections_created.fetch_add(1, Ordering::AcqRel);

        Ok(TestConnection { 
            written: &self.written, 
            connections_closed: &self.connections_closed,
            readable: &self.readable,
            read_waker: &self.read_waker,
        })
    }
}


