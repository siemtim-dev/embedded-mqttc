#![cfg_attr(not(feature = "std"), no_std)]

use thiserror::Error;

mod write;
pub use write::*;


mod read;
pub use read::*;

#[cfg(feature = "serde")]
pub mod json;



/// Error enum 
#[derive(Error, Debug, PartialEq)]
#[cfg_attr(feature = "defmt", derive(defmt::Format))]
pub enum BufferError {

    /// This error is returned if there is not enaugh remaining space
    #[error("Error writing to buffer: no remaining capacity")]
    NoCapacity,

    /// A provided slice wa empty
    /// Is used for trait implementations like [`embedded_io::Read`], [`embedded_io::Write`], [`std::io::Read`], [`std::io::Write`]
    #[error("The provided slice to read from or write to has a len = 0")]
    ProvidedSliceEmpty,

    /// If you try to read data from an empty buffer this errror is returned
    #[error("Error reading from buffer: no remaining data")]
    NoData,


    #[cfg(feature = "serde")]
    #[error("Error while deserializing JSON")]
    JsonDeserialize(serde_json_core::de::Error)
}


/// Trait that allows to create a reader and a writer for a buffer.
/// See [`BufferReader`] adn [`BufferWriter`]
///
pub trait ReadWrite {
    /// Creates a reader to read from the buffer
    fn create_reader<'a>(&'a mut self) -> impl BufferReader + 'a;

    /// Creates a writer to write to the buffer
    fn create_writer<'a>(&'a mut self) -> impl BufferWriter + 'a;
}

/// A buffer that allows reading and writuing bytes [`u8`] from / to an underlying generic source
#[derive(Debug)]
pub struct Buffer<T: AsMut<[u8]> + AsRef<[u8]>> {
    pub(crate) source: T,
    pub(crate) write_position: usize,
    pub(crate) read_position: usize,
}

/// Creates a new [`Buffer`] that is backed by an owned [`u8`] array with size `N`
pub fn new_stack_buffer<const N: usize>() -> Buffer<[u8; N]> {
    Buffer::<[u8; N]> {
        source: [0; N],
        read_position: 0,
        write_position: 0,
    }
}

#[cfg(feature = "defmt")]
impl <T: AsMut<[u8]> + AsRef<[u8]>> defmt::Format for Buffer<T> {
    fn format(&self, fmt: defmt::Formatter) {
        defmt::write!(fmt, 
            "Buffer(len = {}, cap = {}, rem_cap = {})",
            self.remaining_len(),
            self.capacity(),
            self.remaining_capacity()
        );
    }
}

impl <T: AsMut<[u8]> + AsRef<[u8]>> Buffer<T> {

    /// Create a new buffer from any source 
    /// 
    /// # Example
    /// 
    /// ```rust
    ///     use embytes_buffer::Buffer;
    /// 
    ///     let mut bytes = [0; 1024];
    ///     let mut buffer = Buffer::new(&mut bytes);
    /// ```
    pub fn new(source: T) -> Self {
        Self {
            source,
            read_position: 0,
            write_position: 0,
        }
    }

    /// Reset the buffer to its initial state
    pub fn reset(&mut self) {
        self.read_position = 0;
        self.write_position = 0;
    }

    /// Returns the length of the undelying buffer
    pub fn capacity(&self) -> usize {
        self.source.as_ref().len()
    }

    /// Returns the remaining space that can be written to. 
    /// This method does not perform a [`Buffer::shift`]
    pub fn remaining_capacity(&self) -> usize {
        self.capacity() - self.write_position
    }

    /// returns `true` if there is remaining capacity to write to. 
    /// is equal to [`Buffer::remaining_capacity`] ` > 0`
    pub fn has_remaining_capacity(&self) -> bool {
        self.capacity() > self.write_position
    }

    /// Returns the remaining bytes to read
    pub fn remaining_len(&self) -> usize {
        self.write_position - self.read_position
    }

    /// returns `true` if there are remainng bytes to read. 
    pub fn has_remaining_len(&self) -> bool {
        self.write_position > self.read_position
    }

    /// Returns `true` if there is dead capacity. 
    /// Dead capacity occures when bytes are read from a buffer.
    /// Dead capacity can be regained by using [`Buffer::shift`]
    pub fn has_dead_capacity(&self) -> bool {
        self.read_position > 0
    }

    /// Shifts the content of the source left to reuse space of read bytes. 
    /// See also [`Buffer::has_dead_capacity`]
    pub fn shift(&mut self) {
        self.source.as_mut().rotate_left(self.read_position);
        self.write_position -= self.read_position;
        self.read_position = 0;
    }

    /// Performa s [`Buffer::shift`] if there is no remianing capacity and 
    /// returns `true` if there is remainig capacity afterwards
    pub fn ensure_remaining_capacity(&mut self) -> bool {
        if ! self.has_remaining_capacity() {
            self.shift();
        }

        self.has_remaining_capacity()
    }

    /// Base function for implementing writers like [`embedded_io::Write`]
    /// Returns the number of bytes writen to the buffer from the provided slice
    /// 
    /// # Errors
    /// 
    /// [`BufferError::ProvidedSliceEmpty`] if the provided slice is empty
    /// [`BufferError::NoCapacity`] if the buffer has no capacity after calling [`Buffer::shift`]
    pub(crate) fn write_base(&mut self, buf: &[u8]) -> Result<usize, BufferError> {
        if buf.is_empty() {
            return Err(BufferError::ProvidedSliceEmpty);
        }

        if ! self.has_remaining_capacity() && self.has_dead_capacity() {
            self.shift();
        }
        
        let cap = self.remaining_capacity();
        if cap == 0 {
            return Err(BufferError::NoCapacity);
        }

        let tgt = self.source.as_mut();
        let tgt = &mut tgt[self.write_position..];
        
        if cap < buf.len() {
            tgt.copy_from_slice(&buf[0..cap]);
            self.write_position += cap;
            Ok(cap)
        } else {
            let tgt = &mut tgt[0..buf.len()];
            tgt.copy_from_slice(buf);
            self.write_position += buf.len();
            Ok(buf.len())
        }
    }

    /// Base function for implementing readers like [`embedded_io::Read`]
    /// Returns the number of bytes read from the buffer to the provided slice
    /// 
    /// # Errors
    /// 
    /// [`BufferError::ProvidedSliceEmpty`] if the provided slice is empty
    /// [`BufferError::NoData`] if there ae no bytes to read
    pub(crate) fn read_base(&mut self, buf: &mut[u8]) -> Result<usize, BufferError> {
        if buf.is_empty() {
            return Err(BufferError::ProvidedSliceEmpty);
        }

        let src = self.source.as_ref();
        let src = &src[self.read_position..self.write_position];

        if src.is_empty() {
            return Err(BufferError::NoData);
        }
        else if src.len() > buf.len() {
            buf.copy_from_slice(&src[0..buf.len()]);
            self.read_position += buf.len();
            Ok(buf.len())
        } else {
            let buf = &mut buf[0..src.len()];
            buf.copy_from_slice(src);
            self.read_position += src.len();

            Ok(src.len())
        }
    }

    /// Creates a reader that ready at most `max_bytes`
    pub fn create_reader_with_max(&mut self, max_bytes: usize) -> Reader<'_, T> {
        Reader::new_with_max(self, max_bytes)
    }

    /// Returns a slice containing the readable data
    pub fn data(&self) -> &[u8] {
        let src = self.source.as_ref();
        &src[self.read_position..self.write_position]
    }

    /// Skips `n` readable bytes
    /// 
    /// # Errors
    /// 
    /// [`BufferError::NoData`] if `n < self.remaining_len()`
    pub fn skip(&mut self, n: usize) -> Result<(), BufferError> {
        if self.remaining_len() >= n {
            self.read_position += n;
            Ok(())
        } else {
            Err(BufferError::NoData)
        }
    }

    /// Appends the provided slice to the buffer a a whole
    /// 
    /// # Error
    /// 
    /// [`BufferError::NoCapacity`] if `buf.len() > self.remaining_capacity()`
    pub fn push(&mut self, buf: &[u8]) -> Result<(), BufferError> {
        if self.remaining_capacity() < buf.len() && self.has_dead_capacity() {
            self.shift();
        }
        
        if self.remaining_capacity() >= buf.len() {
            let tgt = self.source.as_mut();
            let tgt = &mut tgt[self.write_position..];
            let tgt = &mut tgt[0..buf.len()];
            tgt.copy_from_slice(buf);
            self.write_position += buf.len();
            Ok(())
        } else {
            Err(BufferError::NoCapacity)
        }
    }

}

impl <T: AsMut<[u8]> + AsRef<[u8]>> ReadWrite for Buffer<T> {
    fn create_reader<'a>(&'a mut self) -> impl BufferReader + 'a {
        Reader::new(self)
    }

    fn create_writer<'a>(&'a mut self) -> impl BufferWriter + 'a {
        self.shift();
        Write::new(self)

    }
}

#[cfg(feature = "std")]
impl <T: AsMut<[u8]> + AsRef<[u8]>> std::io::Write for Buffer<T> {

    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        use std::io::ErrorKind;
        match self.write_base(buf) {
            Ok(n) => Ok(n),
            Err(BufferError::ProvidedSliceEmpty) => Ok(0),
            Err(BufferError::NoCapacity) => Err(ErrorKind::WouldBlock.into()),
            Err(e) => {
                panic!("unexpected error writing to buffer: {}", e);
            }
        }
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

#[cfg(feature = "std")]
impl <T: AsMut<[u8]> + AsRef<[u8]>> std::io::Read for Buffer<T> {
    
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        use std::io::ErrorKind;

        match self.read_base(buf) {
            Ok(n) => Ok(n),
            Err(BufferError::ProvidedSliceEmpty) => Ok(0),
            Err(BufferError::NoData) => Err(ErrorKind::WouldBlock.into()),
            Err(e) => {
                panic!("unexpected error reading from buffer: {}", e);
            }
        }
    }
}

#[cfg(feature = "embedded")]
impl <T: AsMut<[u8]> + AsRef<[u8]>> embedded_io::ErrorType for Buffer<T> {
    type Error = embedded_io::ErrorKind;
}

#[cfg(feature = "embedded")]
impl <T: AsMut<[u8]> + AsRef<[u8]>> embedded_io::Write for Buffer<T> {
    
    fn write(&mut self, buf: &[u8]) -> Result<usize, Self::Error> {
        use embedded_io::ErrorKind;

        match self.write_base(buf) {
            Ok(n) => Ok(n),
            Err(BufferError::ProvidedSliceEmpty) => Ok(0),
            Err(BufferError::NoCapacity) => Err(ErrorKind::Other),
            Err(e) => {
                panic!("unexpected error writing to buffer: {}", e);
            }
        }
    }

    fn flush(&mut self) -> Result<(), Self::Error> {
        Ok(())
    }
}

#[cfg(feature = "embedded")]
impl <T: AsMut<[u8]> + AsRef<[u8]>> embedded_io::Read for Buffer<T> {
    fn read(&mut self, buf: &mut [u8]) -> Result<usize, Self::Error> {
        use embedded_io::ErrorKind;

        match self.read_base(buf) {
            Ok(n) => Ok(n),
            Err(BufferError::ProvidedSliceEmpty) => Ok(0),
            Err(BufferError::NoData) => Err(ErrorKind::Other),
            Err(e) => {
                panic!("unexpected error reading from buffer: {}", e);
            }
        }
    }
}

impl <T: AsMut<[u8]> + AsRef<[u8]> + Clone> Clone for Buffer<T> {
    fn clone(&self) -> Self {
        Self { 
            source: self.source.clone(), 
            write_position: self.write_position, 
            read_position: self.read_position
        }
    }
}

#[cfg(test)]
mod tests {

    use crate::{new_stack_buffer, Buffer, BufferError};

    #[test]
    fn test_std_write_high_cap() {
        let mut b = [0u8; 8];
        let mut buf = Buffer::new(&mut b);

        let n = buf.write_base(&[3, 4, 5]).unwrap();
        assert_eq!(n, 3);

        assert_eq!(buf.read_position, 0);
        assert_eq!(buf.write_position, 3);
        drop(buf);

        assert_eq!(&b, &[3, 4, 5, 0, 0, 0, 0, 0])
    }

    #[test]
    fn test_std_write_low_cap() {
        let mut b = [0u8; 4];
        let mut buf = Buffer::new(&mut b);

        let n = buf.write_base(&[1, 2, 3, 4, 5, 6]).unwrap();

        assert_eq!(n, 4);

        assert_eq!(buf.read_position, 0);
        assert_eq!(buf.write_position, 4);
        drop(buf);

        assert_eq!(&b, &[1, 2, 3, 4])
    }

    #[test]
    fn test_shift() {
        let mut b = [0, 1, 2, 3, 4, 5, 6, 7];
        let mut buf = Buffer::new(&mut b);
        buf.read_position = 4;
        buf.write_position = 8;

        buf.shift();

        assert_eq!(buf.read_position, 0);
        assert_eq!(buf.write_position, 4);

        drop(buf);
        assert_eq!(&b[0..4], &[4, 5, 6, 7]);
    }

    #[test]
    fn test_write_dead_cap() {
        let mut b = [0u8; 8];
        let mut buf = Buffer::new(&mut b);

        // Write the buffer full
        let n = buf.write_base(&[1, 2, 3, 4, 5, 6, 7, 8]).unwrap();
        assert_eq!(n, 8);

        // Read half
        let mut tgt = [0u8; 4];
        let n = buf.read_base(&mut tgt[..]).unwrap();
        assert_eq!(n, 4);
        assert_eq!(&tgt, &[1, 2, 3, 4]);
        assert_eq!(buf.read_position, 4);
        assert_eq!(buf.write_position, 8);

        // Now 4 Bytes write should be ok
        let n = buf.write_base(&[10, 20, 30, 40]).unwrap();
        assert_eq!(n, 4);
        assert_eq!(buf.read_position, 0);
        assert_eq!(buf.write_position, 8);

        drop(buf);
        assert_eq!(&b, &[5, 6, 7, 8, 10, 20, 30, 40]);

    }

    #[test]
    fn test_multi_write() {

        let mut b = [0u8; 4];
        let mut buf = Buffer::new(&mut b);

        buf.write_base(&[0]).unwrap();
        buf.write_base(&[1]).unwrap();
        buf.write_base(&[2]).unwrap();
        buf.write_base(&[3]).unwrap();

        drop(buf);
        assert_eq!(&b, &[0, 1, 2, 3]);
    }

    #[test]
    fn test_stack_buffer() {

        let mut buf = new_stack_buffer::<4>();

        buf.write_base(&[1, 2, 3, 4]).unwrap();

    }

    #[test]
    fn test_skip_success() {

        let mut b = [0u8; 4];
        let mut buf = Buffer::new(&mut b);

        buf.write_base(&[4, 4, 4, 4]).unwrap();
        assert_eq!(buf.read_position, 0);
        assert_eq!(buf.write_position, 4);

        buf.skip(2).unwrap();
        assert_eq!(buf.read_position, 2);
        assert_eq!(buf.write_position, 4);
    }

    #[test]
    fn test_skip_fail() {

        let mut b = [0u8; 8];
        let mut buf = Buffer::new(&mut b);

        buf.write_base(&[4, 4, 4, 4]).unwrap();
        assert_eq!(buf.read_position, 0);
        assert_eq!(buf.write_position, 4);

        let res = buf.skip(5);
        assert_eq!(res, Err(BufferError::NoData));
    }
}