use core::{future::Future, mem, net::IpAddr, ops::Deref, task::Poll};
use std::{collections::HashMap, sync::{Arc, Mutex}};

use embassy_sync::waitqueue::WakerRegistration;
use embedded_io_async::{ErrorType, Read, Write};
use embedded_nal_async::{AddrType, Dns, TcpConnect};
use mqttrs2::{Packet, decode_slice, encode_slice};

use crate::state::connection::{ConnectionState, ConnectionStateValue};


extern crate std;

pub struct TestPacket<'a> {
    _data: Vec<u8>,
    packet: Packet<'a>
}

impl <'a> Deref for TestPacket<'a> {
    type Target = Packet<'a>;

    fn deref(&self) -> &Self::Target {
        &self.packet
    }
}

impl <'a> TestPacket<'a> {
    fn new(data: Vec<u8>) -> Self {
        let packet = decode_slice(&data).unwrap().unwrap();
        let packet = unsafe {
            mem::transmute(packet)
        };

        Self {
            _data: data,
            packet
        }
    }
}



#[derive(Debug)]
pub struct DummyConnectionState {
    pub next_packet: Arc<Mutex<Option<Vec<u8>>>>,
    pub packet_written: Arc<Mutex<Option<Vec<u8>>>>,
}

impl DummyConnectionState {

    pub fn new() -> Self {
        Self {
            next_packet: Arc::new(Mutex::new(None)),
            packet_written: Arc::new(Mutex::new(None))
        }
    }

    pub fn assert_packet_written<F, U>(&self, f: F) -> U
    where F: FnOnce(Packet<'_>) -> U{
        let data = self.packet_written.lock().unwrap().take()
            .expect("expect a packet written, but there is none");
        let packet = decode_slice(&data).unwrap()
            .expect("expect the data to contain a packet");
        f(packet)
    }

    pub fn set_next_packet(&self, packet: Packet<'_>) {
        let mut buf = [0; 1024];
        let n = encode_slice(&packet, &mut buf).unwrap();
        let buf = Vec::from(&buf[..n]);
        let _ = self.next_packet.lock().unwrap().insert(buf);
    }

}

impl ConnectionState for DummyConnectionState {
    async fn connect(&self) -> Result<(), crate::MqttError> {
        panic!("DummyConnectionState not made for connecting")
    }

    async fn disconnect(&self) -> Result<(), crate::MqttError> {
        panic!("DummyConnectionState not made for disconnecting")
    }

    fn get_state(&self) -> Option<super::ConnectionStateValue> {
        Some(ConnectionStateValue::Connected)
    }

    async fn on_state_change(&self) -> super::ConnectionStateValue {
        loop {
            tokio::time::sleep(std::time::Duration::from_secs(60)).await;
        }
    }

    async fn await_connected(&self) {}

    fn set_error(&self) {
        panic!("DummyConnectionState set_error called")
    }

    fn try_write_packet(&self, packet: &mqttrs2::Packet<'_>) -> Result<bool, crate::MqttError> {
        let mut lock = self.packet_written.lock().unwrap();
        if lock.is_none() {
            let mut buf = [0; 1024];
            let n = encode_slice(packet, &mut buf).unwrap();
            *lock = Some(Vec::from(&buf[0..n]));
            Ok(true)
        } else {
            Ok(false)
        }
    } 

    async fn write_packet(&self, packet: &mqttrs2::Packet<'_>) -> Result<(), crate::MqttError> {
        while ! self.try_write_packet(packet)? {
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }

        Ok(())
    }

    async fn run_io(&self) -> Result<impl core::ops::Deref<Target = mqttrs2::Packet<'_>>, crate::MqttError> {
        loop {
            if let Some(packet_data) = self.next_packet.lock().unwrap().take() {
                return Ok(TestPacket::new(packet_data));
            }

            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
    }

    async fn run_io_nonblocking(&self) -> Result<Option<impl core::ops::Deref<Target = mqttrs2::Packet<'_>>>, crate::MqttError> {
        loop {
            if let Some(packet_data) = self.next_packet.lock().unwrap().take() {
                return Ok(Some(TestPacket::new(packet_data)));
            }

            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
    }
}

impl Clone for DummyConnectionState {
    fn clone(&self) -> Self {
        Self { 
            next_packet: Arc::clone(&self.next_packet), 
            packet_written: Arc::clone(&self.packet_written)
        }
    }
}

pub struct TestTcpConnection<'a> {
    bytes_sent: &'a Mutex<Vec<u8>>,
    bytes_received: &'a Mutex<Vec<u8>>,
    read_wakers: &'a Mutex<WakerRegistration>,
}

pub struct TestConnectionReadFuture<'a, 'b> {
    bytes_received: &'a Mutex<Vec<u8>>,
    read_wakers: &'a Mutex<WakerRegistration>,
    buf: &'b mut [u8]
}

impl<'a, 'b> Future for TestConnectionReadFuture<'a, 'b> {
    type Output = Result<usize, embedded_io_async::ErrorKind>;

    fn poll(mut self: core::pin::Pin<&mut Self>, cx: &mut core::task::Context<'_>) -> Poll<Self::Output> {
        let mut bytes_received = self.bytes_received.lock().unwrap();
        let mut read_wakers = self.read_wakers.lock().unwrap();

        if bytes_received.is_empty() {
            read_wakers.register(cx.waker());
            Poll::Pending
        } else if bytes_received.len() > self.buf.len() {
            let buf = &mut self.buf;
            buf.copy_from_slice(&bytes_received[..buf.len()]);
            bytes_received.drain(..self.buf.len());
            Poll::Ready(Ok(self.buf.len()))
        } else {
            let bytes_read = bytes_received.len();
            self.buf[..bytes_read].copy_from_slice(&bytes_received);
            bytes_received.clear();
            Poll::Ready(Ok(bytes_read))
        }
    }
}

impl<'a> Read for TestTcpConnection<'a> {
    fn read(&mut self, buf: &mut [u8]) -> impl Future<Output = Result<usize, Self::Error>> {
        TestConnectionReadFuture{
            bytes_received: self.bytes_received,
            read_wakers: self.read_wakers,
            buf
        }
    }
}

impl<'a> ErrorType for TestTcpConnection<'a> {
    type Error = embedded_io_async::ErrorKind;
}

impl<'a> Write for TestTcpConnection<'a> {
    async fn write(&mut self, buf: &[u8]) -> Result<usize, Self::Error> {
        let mut bytes_sent = self.bytes_sent.lock().unwrap();
        bytes_sent.extend_from_slice(buf);
        Ok(buf.len())
    }
    
    async fn flush(&mut self) -> Result<(), Self::Error> {
        Ok(())
    }
}

pub struct TestTcpConnect {
    pub bytes_sent: Mutex<Vec<u8>>,
    bytes_received: Mutex<Vec<u8>>,
    read_wakers: Mutex<WakerRegistration>,
}

impl TestTcpConnect {

    pub fn new() -> Self {
        Self {
            bytes_received: Mutex::new(Vec::new()),
            bytes_sent: Mutex::new(Vec::new()),
            read_wakers: Mutex::new(WakerRegistration::new())
        }
    }

    pub fn add_bytes_to_receive(&self, buf: &[u8]) {
        let mut bytes_received = self.bytes_received.lock().unwrap();
        let mut read_wakers = self.read_wakers.lock().unwrap();
        bytes_received.extend_from_slice(buf);
        read_wakers.wake();
    }

}

impl TcpConnect for TestTcpConnect {
    type Error = embedded_io_async::ErrorKind;

    type Connection<'a> = TestTcpConnection<'a>;

    async fn connect<'a>(&'a self, _remote: core::net::SocketAddr) -> Result<Self::Connection<'a>, Self::Error> {
        Ok(TestTcpConnection { 
            bytes_sent: &self.bytes_sent, 
            bytes_received: &self.bytes_received,
            read_wakers: &self.read_wakers
        })
    }
}



pub struct TestDns {
    hosts: HashMap<String, IpAddr>
}

impl Dns for TestDns {
    type Error = &'static str;

    async fn get_host_by_name(
            &self,
            host: &str,
            addr_type: AddrType,
        ) -> Result<IpAddr, Self::Error> {
        if let Some(ip) = self.hosts.get(host){
            match (ip, addr_type) {
                (ip, AddrType::Either) => Ok(ip.clone()),
                (IpAddr::V4(v4), AddrType::IPv4) => Ok(IpAddr::V4(v4.clone())),
                (IpAddr::V6(v6), AddrType::IPv6) => Ok(IpAddr::V6(v6.clone())),
                _ => Err("host not found")
            }
        } else {
            Err("host not found")
        }
    }

    async fn get_host_by_address(
            &self,
            _addr: IpAddr,
            _result: &mut [u8],
        ) -> Result<usize, Self::Error> {
        unimplemented!()
    }
}

impl TestDns {
    pub fn new(hosts: HashMap<String, IpAddr>) -> Self {
        Self {
            hosts
        }
    }

    pub fn new_single(name: impl Into<String>, ip: IpAddr) -> Self {
        Self {
            hosts: {
                let mut map = HashMap::new();
                map.insert(name.into(), ip);
                map
            }
        }
    }
}
