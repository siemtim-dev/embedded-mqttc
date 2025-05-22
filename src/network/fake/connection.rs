use core::{future::Future, pin::Pin, task::{Context, Poll}};

use embytes_buffer::{BufferReader, ReadWrite};
use embedded_io_async::{ErrorKind, ErrorType, Read, Write};
use mqttrs::Packet;
use crate::network::{mqtt::MqttPacketError, mqtt::ReadMqttPacket, NetworkConnection, NetworkError, TryRead, TryWrite};

use super::BufferedStream;

pub struct ServerConnection<'a, const N: usize> {
    pub(crate) out_stream: &'a BufferedStream<N>,
    pub(crate) in_stream: &'a BufferedStream<N>
}

impl <'a, const N: usize> ServerConnection <'a, N>{
    pub fn with_reader<F, R>(&self, f: F) -> R where F: FnOnce(&dyn BufferReader) -> R {
        self.in_stream.with_reader(f)
    }
}

impl <'a, const N: usize> ErrorType for ServerConnection<'a, N>  {
    type Error = ErrorKind;
}

impl <'a, const N: usize> Write for ServerConnection<'a, N>  {
    async fn write(&mut self, buf: &[u8]) -> Result<usize, Self::Error> {
        self.out_stream.write_async(buf).await
    }
}

impl <'a, const N: usize> Read for ServerConnection<'a, N>  {
    async fn read(&mut self, buf: &mut [u8]) -> Result<usize, Self::Error> {
        self.in_stream.read_async(buf).await
    }
}

impl <'a, const N: usize> TryRead for ServerConnection<'a, N>  {
    async fn try_read(&mut self, buf: &mut [u8]) -> Result<usize, Self::Error> {
        self.in_stream.try_read_sync(buf)
    }
}

impl <'a, const N: usize> TryWrite for ServerConnection<'a, N>  {
    async fn try_write(&mut self, buf: &[u8]) -> Result<usize, Self::Error> {
        self.out_stream.try_write_sync(buf)
    }
}

pub struct ClientConnection<'a, const N: usize>{
    out_stream: &'a BufferedStream<N>,
    in_stream: &'a BufferedStream<N>
}

impl <'a, const N: usize> ErrorType for ClientConnection<'a, N>  {
    type Error = ErrorKind;
}

impl <'a, const N: usize> Unpin for ClientConnection<'a, N> {}

impl <'a, const N: usize> Write for ClientConnection<'a, N>  {
    async fn write(&mut self, buf: &[u8]) -> Result<usize, Self::Error> {
        self.out_stream.write_async(buf).await
    }
}

impl <'a, const N: usize> Read for ClientConnection<'a, N>  {
    async fn read(&mut self, buf: &mut [u8]) -> Result<usize, Self::Error> {
        self.in_stream.read_async(buf).await
    }
}

impl <'a, const N: usize> TryRead for ClientConnection<'a, N>  {
    async fn try_read(&mut self, buf: &mut [u8]) -> Result<usize, Self::Error> {
        self.in_stream.try_read_sync(buf)
    }
}

impl <'a, const N: usize> TryWrite for ClientConnection<'a, N>  {
    async fn try_write(&mut self, buf: &[u8]) -> Result<usize, Self::Error> {
        self.out_stream.try_write_sync(buf)
    }
}

impl <'a, const N: usize> NetworkConnection for ClientConnection<'a, N> {
    async fn connect(&mut self) -> Result<(), NetworkError> {
        Ok(())
    }
}

pub struct ConnectionRessources<const N: usize> {
    client_to_server: BufferedStream<N>,
    server_to_client: BufferedStream<N>
}

impl <const N: usize> ConnectionRessources<N> {
    pub fn new() -> Self {
        Self {
            client_to_server: BufferedStream::new(),
            server_to_client: BufferedStream::new()
        }
    }
}

pub fn new_connection<'a, const N: usize>(resources: &'a ConnectionRessources<N>) 
    -> (ClientConnection<'a, N>, ServerConnection<'a, N>) {
    
    let client = ClientConnection{
        out_stream: &resources.client_to_server,
        in_stream: &resources.server_to_client
    };

    let server = ServerConnection{
        out_stream: &resources.server_to_client,
        in_stream: &resources.client_to_server
    };

    (client, server)

}


pub trait ReadAtomic: ErrorType {

    fn read_atomic<T, F>(&self, f: F) -> impl Future<Output = Result<T, MqttPacketError>>
        where F: Fn(&dyn BufferReader) -> Result<Option<T>, MqttPacketError>;

    fn read_mqtt_packet<O, R>(&self, o: O) -> impl Future<Output = Result<R, MqttPacketError>>
        where O: Fn(&Packet<'_>) -> R {
        async move {
            self.read_atomic(|reader|{
                let packet = reader.read_packet()?;
    
                if let Some(packet) = packet {
                    let result = o(&packet);
                    Ok(Some(result))
    
                } else {
                    Ok(None)
                }
            })
            .await
        }
    }

}

pub struct ReadAtomicFuture<'b, T, F, const N: usize> where F: Fn(&dyn BufferReader) -> Result<Option<T>, MqttPacketError> {
    f: F,
    stream: &'b BufferedStream<N>
}

impl <'b, T, F, const N: usize> Future for ReadAtomicFuture<'b, T, F, N> 
    where F: Fn(&dyn BufferReader) -> Result<Option<T>, MqttPacketError> {
    
    type Output = Result<T, MqttPacketError>;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        
        self.stream.inner.lock(|inner| {
            let mut inner = inner.borrow_mut();
            let reader = inner.buffer.create_reader();

            let result = (self.f)(&reader);
            drop(reader);

            match result {
                Ok(op) => {
                    if let Some(atomic) = op {
                        Poll::Ready(Ok(atomic))
                    } else {
                        inner.read_waker.register(cx.waker());
                        Poll::Pending
                    }
                },
                Err(e) => Poll::Ready(Err(e)),
            }
        })
    }
}

impl <'b, T, F, const N: usize> Unpin for ReadAtomicFuture<'b, T, F, N> where F: Fn(&dyn BufferReader) -> Result<Option<T>, MqttPacketError> {}

impl <const N: usize> ReadAtomic for BufferedStream<N> {
    fn read_atomic<T, F>(&self, f: F) -> impl Future<Output = Result<T, MqttPacketError>>
        where F: Fn(&dyn BufferReader) -> Result<Option<T>, MqttPacketError> {
            ReadAtomicFuture{
                f,
                stream: self
            }
    }
}

impl <'a, const N: usize> ReadAtomic for ServerConnection<'a, N> {
    async fn read_atomic<T, F>(&self, f: F) -> Result<T, MqttPacketError> where F: Fn(&dyn BufferReader) -> Result<Option<T>, MqttPacketError>{
            self.in_stream.read_atomic(f).await
    }
}

#[cfg(test)]
mod connection_tests {
    use core::time::Duration;

    use embedded_io_async::{Read, Write};

    use super::{new_connection, ConnectionRessources};

    #[tokio::test]
    async fn test_connection() {

        let resources = ConnectionRessources::<4>::new();

        let (mut client, mut server) = new_connection(&resources);

        let client_future = async {

            let n = client.write(&[0, 1, 2, 3]).await.unwrap();
            assert_eq!(n, 4);

            tokio::time::sleep(Duration::from_millis(100)).await;

            for i in 4..8 {
                client.write(&[i]).await.unwrap();
                tokio::time::sleep(Duration::from_millis(100)).await;
            }

        };

        let server_future = async {

            let mut results = Vec::new();

            for _ in 0..8 {
                let mut buf = [0; 1];
                server.read(&mut buf).await.unwrap();
                results.push(buf[0]);
            }

        };

        tokio::join!(client_future, server_future);

    }
}

