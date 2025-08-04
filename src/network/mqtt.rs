use core::future::Future;

use embytes_buffer::{BufferReader, BufferWriter};
use mqttrs2::{decode_slice_with_len, encode_slice, Packet};
use thiserror::Error;

use super::fake::{BufferedStream, ServerConnection};

#[derive(Debug, PartialEq, Clone, Error)]
#[cfg_attr(feature = "defmt", derive(defmt::Format))]
pub enum MqttPacketError {

    #[error("error whie encoding / decoding packet")]
    CodecError,
    
    #[error("error whie writing / reading packet")]
    IoError(embedded_io_async::ErrorKind),

    #[error("something wrong with the network")]
    NetworkError(super::NetworkError),

    #[error("buffer has not enaugh space left to write packet")]
    NotEnaughBufferSpace
}

pub trait WriteMqttPacketMut {
    fn write_mqtt_packet_sync(&mut self, packet: &Packet<'_>) -> Result<(), MqttPacketError>;
}

impl <T> WriteMqttPacketMut for T where T: BufferWriter {
    fn write_mqtt_packet_sync(&mut self, packet: &Packet<'_>) -> Result<(), MqttPacketError> {
        

        let result = encode_slice(packet, self);

        match result {
            Ok(n) => {
                self.commit(n).unwrap();
                Ok(())
            },
            Err(mqttrs2::Error::WriteZero) => {
                Err(MqttPacketError::NotEnaughBufferSpace)
            }
            Err(e) => {
                error!("cannot write mqtt packet: {}", e);
                Err(MqttPacketError::CodecError)
            },
        }
    }
}

pub trait WriteMqttPacket {
    fn write_mqtt_packet(&self, packet: &Packet<'_>) -> impl Future<Output = Result<(), MqttPacketError>>;
}

impl <const N: usize> WriteMqttPacket for  BufferedStream<N> {
    async fn write_mqtt_packet(&self, packet: &Packet<'_>) -> Result<(), MqttPacketError> {
        let mut bytes = [0; 256];
        let n = encode_slice(packet, &mut bytes)
            .map_err(|_| MqttPacketError::CodecError)?;

        let mut to_write = &bytes[..n];

        while ! to_write.is_empty() {
            let n = self.write_async(&bytes[..n]).await 
                .map_err(|e| MqttPacketError::IoError(e))?;

            to_write = &to_write[n..];
        }

        Ok(())
    }
}

impl <'a, const N: usize> WriteMqttPacket for ServerConnection<'a, N> {
    fn write_mqtt_packet(&self, packet: &Packet<'_>) -> impl Future<Output = Result<(), MqttPacketError>>{
        self.out_stream.write_mqtt_packet(packet)
    }
}

pub trait ReadMqttPacket {
    fn read_packet<'a>(&'a self) -> Result<Option<Packet<'a>>, MqttPacketError>;
}

impl <T> ReadMqttPacket for T where T: BufferReader + ?Sized {
    fn read_packet<'a>(&'a self) -> Result<Option<Packet<'a>>, MqttPacketError> {
        let nop = decode_slice_with_len(&self)
            .map_err(|_| MqttPacketError::CodecError)?;

        if let Some((n, p)) = nop {
            self.add_bytes_read(n);
            Ok(Some(p))
        } else {
            Ok(None)
        }
    }
}

