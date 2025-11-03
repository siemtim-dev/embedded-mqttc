// use core::usize;

// use mqttrs2::{Connack, Packet, Pid, Publish, QosPid, Suback};

// use crate::MqttError;

// use heapless::{String, Vec};

// pub struct OwnedPublish<const BUFFER: usize, const TOPIC_SIZE: usize> {
//     pub dup: bool,
//     pub qospid: QosPid,
//     pub retain: bool,
//     pub topic_name: String<TOPIC_SIZE>,
//     pub payload: Vec<u8, BUFFER>
// }

// impl<const BUFFER: usize, const TOPIC_SIZE: usize> OwnedPublish<BUFFER, TOPIC_SIZE> {

//     pub fn borrow<'a>(&'a self) -> Packet<'a> {
//         Packet::Publish(Publish{
//             dup: self.dup,
//             qospid: self.qospid,
//             retain: self.retain,
//             topic_name: &self.topic_name,
//             payload: &self.payload
//         })
//     }
// }

// impl <'a, const BUFFER: usize, const TOPIC_SIZE: usize> TryFrom<&Publish<'a>> for OwnedPublish<BUFFER, TOPIC_SIZE> {
//     type Error = MqttError;

//     fn try_from(value: &Publish<'a>) -> Result<Self, Self::Error> {
//         Ok(Self {
//             dup: value.dup,
//             qospid: value.qospid,
//             retain: value.retain,
//             topic_name: String::try_from(value.topic_name)
//                 .map_err(|_| MqttError::BufferError(embytes_buffer_async::BufferError::NoCapacity))?, // TODO map another error
//             payload: Vec::try_from(value.payload)
//                 .map_err(|_| MqttError::BufferError(embytes_buffer_async::BufferError::NoCapacity))?, // TODO map another error
//         })
//     }
// }

// impl <'a, const BUFFER: usize, const TOPIC_SIZE: usize> TryFrom<Publish<'a>> for OwnedPublish<BUFFER, TOPIC_SIZE> {
//     type Error = MqttError;

//     fn try_from(value: Publish<'a>) -> Result<Self, Self::Error> {
//         Self::try_from(&value)
//     }
// }

// pub enum ReceivedPacket<const BUFFER: usize, const TOPIC_SIZE: usize> {
//     Connack(Connack),
//     Publish(OwnedPublish<BUFFER, TOPIC_SIZE>),
//     Puback(Pid),
//     Pubrec(Pid),
//     Pubrel(Pid),
//     Pubcomp(Pid),
//     Suback(Suback),
//     Unsuback(Pid),
//     Pingresp,
//     Disconnect,
// }

// impl <'a, const BUFFER: usize, const TOPIC_SIZE: usize> TryFrom<Packet<'_>> for ReceivedPacket<BUFFER, TOPIC_SIZE> {
//     type Error = MqttError;

//     fn try_from(value: Packet<'_>) -> Result<Self, Self::Error> {
//         match value {
//             Packet::Connack(connack) => Ok(Self::Connack(connack)),
//             Packet::Publish(publish) => Ok(Self::Publish(OwnedPublish::try_from(publish)?)),
//             Packet::Puback(pid) => Ok(Self::Puback(pid)),
//             Packet::Pubrec(pid) => Ok(Self::Pubrec(pid)),
//             Packet::Pubrel(pid) => Ok(Self::Pubrel(pid)),
//             Packet::Pubcomp(pid) => Ok(Self::Pubcomp(pid)),
//             Packet::Suback(suback) => Ok(Self::Suback(suback)),
//             Packet::Unsuback(pid) => Ok(Self::Unsuback(pid)),
//             Packet::Pingresp => Ok(Self::Pingresp),
//             Packet::Disconnect => Ok(Self::Disconnect),

//             // These packets cannot be received
//             // Packet::Connect(connect) => todo!(),
//             // Packet::Subscribe(subscribe) => todo!(),
//             // Packet::Unsubscribe(unsubscribe) => todo!(),
//             // Packet::Pingreq => todo!(),
//             _ => Err(MqttError::InternalError)
//         }
//     }
// }
