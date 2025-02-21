

use buffer::{BufferReader, BufferWriter};
use mqttrs::{decode_slice_with_len, encode_slice, Packet};

use crate::MqttError;

#[derive(Debug, thiserror::Error, PartialEq, Clone)]
pub enum WritePacketError {
    
    #[error("")]
    NotEnaughSpace,

    #[error("")]
    Other(#[from] MqttError)

}

impl From<mqttrs::Error> for WritePacketError {
    fn from(value: mqttrs::Error) -> Self {
        match value {
            mqttrs::Error::WriteZero => WritePacketError::NotEnaughSpace,
            _ => WritePacketError::Other(MqttError::CodecError)
        }
    }
}

pub trait MqttPacketWriter {

    fn write_packet(&mut self, packet: &Packet<'_>) -> Result<(), WritePacketError>;

}

impl <T> MqttPacketWriter for T where T: BufferWriter {
    fn write_packet(&mut self, packet: &Packet<'_>) -> Result<(), WritePacketError> {
        
        let n = encode_slice(packet, self)
            .map_err(|e| WritePacketError::from(e))?;

        self.commit(n)
            .expect("unexpected error result: commiting more bytes than written");

        Ok(())
    }
}


pub trait MqttPacketReader {

    fn read_packet<'a>(&'a self) -> Result<Option<Packet<'a>>, MqttError>; 

}

impl <T> MqttPacketReader for T where T: BufferReader + ?Sized {
    
    
    fn read_packet<'a>(&'a self) -> Result<Option<Packet<'a>>, MqttError> {
        let result = decode_slice_with_len(self)
            .map_err(|_| MqttError::CodecError)?;

        if let Some((n, packet)) = result {
            self.add_bytes_read(n);
            Ok(Some(packet))
        } else {
            Ok(None)
        }
    }
}