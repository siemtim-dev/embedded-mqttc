use core::{mem, ops::Deref};
use std::sync::{Arc, Mutex};

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

