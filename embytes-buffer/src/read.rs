use core::{cell::Cell, ops::Deref};

use crate::Buffer;

/// A Reader to read from a buffer like from a byte slice
pub trait BufferReader: Deref<Target = [u8]> {

    /// Tells the reader that `n` bytes were read
    fn add_bytes_read(&self, n: usize);
}

/// An implementation of [`BufferReader`] for [`Buffer`]
pub struct Reader <'a, T: AsMut<[u8]> + AsRef<[u8]>> {
    buffer: &'a mut Buffer<T>,
    bytes_read: Cell<usize>,
    max_bytes: Option<usize>
}

impl <'a, T: AsMut<[u8]> + AsRef<[u8]>> Reader<'a, T> {

    pub(crate) fn new(buf: &'a mut Buffer<T>) -> Self {
        Self {
            buffer: buf,
            bytes_read: Cell::new(0),
            max_bytes: None
        }
    }

    pub(crate) fn new_with_max(buf: &'a mut Buffer<T>, max_bytes: usize) -> Self {
        Self {
            buffer: buf,
            bytes_read: Cell::new(0),
            max_bytes: Some(max_bytes)
        }
    }

    #[cfg(test)]
    pub(crate) fn get_bytes_read(&self) -> usize {
        self.bytes_read.get()
    }
}

impl <'a, T: AsMut<[u8]> + AsRef<[u8]>> BufferReader for Reader<'a, T> {
    fn add_bytes_read(&self, n: usize) {
        self.bytes_read.set(
            self.bytes_read.get() + n
        );
    }
}

impl <'a, T: AsMut<[u8]> + AsRef<[u8]>> Drop for Reader<'a, T> {
    fn drop(&mut self) {
        let bytes_read = self.bytes_read.get();
        self.buffer.skip(bytes_read)
            .expect("Reader: bytes_read must not be grater than the bytes skippable");
    }
}

impl <'a, T: AsMut<[u8]> + AsRef<[u8]>> Deref for Reader<'a, T> {
    type Target = [u8];

    fn deref(&self) -> &Self::Target {
        let src = self.buffer.data();
        match self.max_bytes {
            Some(max) => &src[..max],
            None => src,
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::{Buffer, BufferReader};
    use super::Reader;


    #[test]
    fn test_read_skip() {
        let mut b = [0u8; 8];
        let mut buf = Buffer::new(&mut b);

        let n = buf.write_base(&[1, 2, 3, 4]).unwrap();
        assert_eq!(n, 4);

        let reader = Reader::new(&mut buf);
        assert_eq!(&reader[..], &[1, 2, 3, 4]);

        reader.add_bytes_read(3);
        assert_eq!(reader.get_bytes_read(), 3);
        drop(reader);

        assert_eq!(buf.read_position, 3);
        assert_eq!(buf.write_position, 4);
    }

}