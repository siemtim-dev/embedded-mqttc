use core::{cell::RefCell, cmp::min, future::Future, pin::Pin, task::{Context, Poll}};

use embytes_buffer::{new_stack_buffer, Buffer, BufferReader, BufferWriter, ReadWrite};
use embassy_sync::{blocking_mutex::{raw::CriticalSectionRawMutex, Mutex}, waitqueue::WakerRegistration};
use embedded_io_async::{ErrorKind, ErrorType, Read, Write};
use crate::network::{TryRead, TryWrite};
pub struct BufferedStream<const N: usize> {
    pub(crate) inner: Mutex<CriticalSectionRawMutex, RefCell<BufferedStreamInner<N>>>
}

impl <const N: usize> BufferedStream<N> {
    pub fn new() -> Self {
        Self {
            inner: Mutex::new(RefCell::new(BufferedStreamInner::new()))
        }
    }

    pub fn read_async<'a, 'b>(&'b self, buf: &'a mut [u8]) -> ReadFuture<'a, 'b, N> {
        ReadFuture { 
            connection: self, 
            buf 
        }
    }

    pub fn try_read_sync(&self, buf: &mut [u8]) -> Result<usize, ErrorKind> {
        self.inner.lock(|inner| {
            inner.borrow_mut().try_read_sync(buf)
        })
    }

    pub fn write_async<'a, 'b>(&'b self, buf: &'a [u8]) -> WriteFuture<'a, 'b, N> {
        WriteFuture { 
            connection: self, 
            buf
        }
    }

    pub fn try_write_sync(&self, buf: &[u8]) -> Result<usize, ErrorKind> {
        self.inner.lock(|inner| {
            inner.borrow_mut().try_write_sync(buf)
        })
    }

    pub fn with_reader<F, R>(&self, f: F) -> R where F: FnOnce(&dyn BufferReader) -> R {
        self.inner.lock(|inner|{
            let mut inner = inner.borrow_mut();
            let reader = inner.buffer.create_reader();
            f(&reader)
        })
    }
}

pub(crate) struct BufferedStreamInner<const N: usize> {
    pub(crate) buffer: Buffer<[u8; N]>,
    pub(crate) read_waker: WakerRegistration,
    pub(crate) write_waker: WakerRegistration
}

impl <const N: usize> BufferedStreamInner<N> {
    fn new() -> Self {
        Self {
            buffer: new_stack_buffer(),
            read_waker: WakerRegistration::new(),
            write_waker: WakerRegistration::new()
        }
    }

    fn try_read_sync(&mut self, buf: &mut [u8]) -> Result<usize, ErrorKind> {


        if buf.len() == 0 {
            return Ok(0);
        }

        if ! self.buffer.has_remaining_len() {
            return Ok(0);
        }

        let n = min(
            self.buffer.remaining_len(),
            buf.len()
        );

        let reader = self.buffer.create_reader();
        let buf = &mut buf[0..n];
        buf.copy_from_slice(&reader[..n]);
        reader.add_bytes_read(n);

        // Sigals wakers that bytes were read
        if n > 0 {
            self.write_waker.wake();
        }

        Ok(n)
    }

    fn try_write_sync(&mut self, buf: &[u8]) -> Result<usize, ErrorKind> {

        if buf.len() == 0 {
            return Ok(0);
        }

        let mut writer = self.buffer.create_writer();

        if writer.remaining_capacity() == 0 {
            return Ok(0);
        }

        let n = min(
            buf.len(),
            writer.remaining_capacity()
        );

        let target = &mut writer[..n];
        target.copy_from_slice(&buf[..n]);
        writer.commit(n).unwrap();

        // Sigals wakers that bytes were written
        if n > 0 {
            self.read_waker.wake();
        }

        Ok(n)
    }

    fn poll_read(&mut self, buf: &mut [u8], cx: &mut Context<'_>) -> Poll<Result<usize, ErrorKind>>  {
        if buf.len() == 0 {
            return Poll::Ready(Ok(0))
        }

        let result = self.try_read_sync(buf);

        match result {
            Ok(n) => {
                if n == 0 {
                    self.read_waker.register(cx.waker());
                    Poll::Pending
                } else {
                    Poll::Ready(Ok(n))
                }
            },
            Err(e) => Poll::Ready(Err(e)),
        }
    }

    fn poll_write(&mut self, buf: &[u8], cx: &mut Context<'_>) -> Poll<Result<usize, ErrorKind>> {
        if buf.len() == 0 {
            return Poll::Ready(Ok(0));
        }
        
        let result = self.try_write_sync(buf);

        match result {
            Ok(n) => {
                if n == 0 {
                        self.write_waker.register(cx.waker());
                    Poll::Pending
                } else {
                    Poll::Ready(Ok(n))
                }
            },
            Err(e) => Poll::Ready(Err(e)),
        }
    }
}

impl <const N: usize> ErrorType for BufferedStream<N> {
    type Error = ErrorKind;
}

impl <const N: usize> TryRead for BufferedStream<N> {
    async fn try_read(&mut self, buf: &mut [u8]) -> Result<usize, Self::Error> {
        self.try_read_sync(buf)
    }
}

impl <const N: usize> TryWrite for BufferedStream<N> {
    async fn try_write(&mut self, buf: &[u8]) -> Result<usize, Self::Error> {
        self.try_write_sync(buf)
    }
}

pub struct ReadFuture<'a, 'b, const N: usize> {
    connection: &'b BufferedStream<N>,
    buf: &'a mut [u8]
}

impl <'a, 'b, const N: usize> Future for ReadFuture<'a, 'b, N> {
    type Output = Result<usize, ErrorKind>;

    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        self.connection.inner.lock(|inner|{
            inner.borrow_mut().poll_read(self.buf, cx)
        })
    }
}

impl <'a, 'b, const N: usize> Unpin for ReadFuture<'a, 'b, N>{}

impl <const N: usize> Read for BufferedStream<N> {
    async fn read(&mut self, buf: &mut [u8]) -> Result<usize, Self::Error> {
        let future = ReadFuture{
            connection: self,
            buf
        };

        future.await
    }
}

pub struct WriteFuture<'a, 'b, const N: usize> {
    connection: &'b BufferedStream<N>,
    buf: &'a [u8]
}

impl <'a, 'b, const N: usize> Future for WriteFuture<'a, 'b, N>  {
    type Output = Result<usize, ErrorKind>;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        self.connection.inner.lock(|inner| {
            inner.borrow_mut().poll_write(self.buf, cx)
        })
    }
}

impl <const N: usize> Write for BufferedStream<N> {
    async fn write(&mut self, buf: &[u8]) -> Result<usize, Self::Error> {
        let future = WriteFuture{
            connection: self,
            buf
        };

        future.await
    }
}

#[cfg(all(test, feature = "std"))]
mod stream_tests {
    use super::BufferedStream;


    #[tokio::test]
    async fn test_stream () {
        let stream = BufferedStream::<4>::new();

        let write_future = async {

            for i in 0..128 {
                stream.write_async(&[i]).await.unwrap();
            }

        };

        let read_future = async {
            let mut remaining = 128;
            let mut received = Vec::new();

            while remaining > 0 {
                let mut buf = [0; 8];
                let n = stream.read_async(&mut buf).await.unwrap();
                remaining -= n;
                received.extend_from_slice(&buf[0..n]); 
            }

            assert_eq!(received[8], 8);
            assert_eq!(received[0], 0);

        };

        tokio::join!(read_future, write_future);
    }

}