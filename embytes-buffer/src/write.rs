use core::{cell::Cell, ops::{Deref, DerefMut}};

use crate::{Buffer, BufferError};

/// A Writer to write to a [`Buffer`] as it is a writeable slice
pub trait BufferWriter: DerefMut<Target = [u8]> {

    /// After writing th bytes to `self` the user must tell teh buffer how many bytes have bee written. 
    /// This increases the write_position of the buffer by `n`
    fn commit(&self, n: usize) -> Result<(), BufferError>;

    /// Return the number of bytes that can be written without an error
    fn remaining_capacity(&self) -> usize;

    /// Returns `true` if the writer has remaining capacity to write to
    fn has_remaining_capacity(&self) -> bool {
        self.remaining_capacity() > 0
    }
}

/// An implementation of [`BufferWriter`] for [`Buffer`]
pub struct Write<'a, T: AsMut<[u8]> + AsRef<[u8]>> {
    buffer: &'a mut Buffer<T>,
    bytes_written: Cell<usize>
}

impl <'a, T: AsMut<[u8]> + AsRef<[u8]>> Write<'a, T> {
    pub(crate) fn new(buffer: &'a mut Buffer<T>) -> Self {
        Self {
            buffer,
            bytes_written: Cell::new(0)
        }
    }
}

impl <'a, T: AsMut<[u8]> + AsRef<[u8]>> BufferWriter for Write<'a, T> {

    fn commit(&self, n: usize) -> Result<(), BufferError> {
        if self.remaining_capacity() < n {
            Err(BufferError::NoCapacity)
        } else {
            self.bytes_written.set(
                self.bytes_written.get() + n
            );
            Ok(())
        }
    }

    fn remaining_capacity(&self) -> usize {
        self.buffer.capacity() - self.buffer.write_position - self.bytes_written.get()
    }
}

impl <'a, T: AsMut<[u8]> + AsRef<[u8]>> Drop for Write<'a, T> {
    fn drop(&mut self) {
        
        self.buffer.write_position += self.bytes_written.get();
        if self.buffer.write_position > self.buffer.source.as_ref().len() {
            panic!("illegal state: Write<'a, T> committed more bytes than available!")
        }

    }
}

impl <'a, T: AsMut<[u8]> + AsRef<[u8]>> Deref for Write<'a, T>{
    type Target = [u8];

    fn deref(&self) -> &Self::Target {
        let tgt = self.buffer.source.as_ref();
        let offset = self.buffer.write_position + self.bytes_written.get();
        &tgt[offset..]
    }
}

impl <'a, T: AsMut<[u8]> + AsRef<[u8]>> DerefMut for Write<'a, T>{
    fn deref_mut(&mut self) -> &mut Self::Target {
        let tgt = self.buffer.source.as_mut();
        let offset = self.buffer.write_position + self.bytes_written.get();
        &mut tgt[offset..]
    }
}

#[cfg(test)]
mod tests {
    use crate::{Buffer, BufferWriter, ReadWrite};


    #[test]
    fn test_write() {
        let mut b = [0u8; 8];
        let mut buf = Buffer::new(&mut b);

        buf.write_base(&[1, 2]).unwrap();


        let mut write = buf.create_writer();
        write[0] = 3;
        write[1] = 4;

        write.commit(2).unwrap();
        drop(write);

        assert_eq!(buf.data(), &[1, 2, 3, 4]);
    }

    #[test]
    fn test_multi_write() {
        let mut b = [0u8; 8];
        let mut buf = Buffer::new(&mut b);

        let mut write = buf.create_writer();
        write[0] = 1;
        write[1] = 2;
        write.commit(2).unwrap();
        drop(write);

        let mut write = buf.create_writer();
        write[0] = 3;
        write[1] = 4;
        write.commit(2).unwrap();
        drop(write);

        assert_eq!(buf.data(), &[1, 2, 3, 4]);
        assert_eq!(buf.write_position, 4);
    }

}