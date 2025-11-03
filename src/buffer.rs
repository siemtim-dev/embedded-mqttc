use core::{cell::{Cell, UnsafeCell}, ops::{Deref, DerefMut}};



pub struct StackBufferCell<const N: usize> {
    buffer: UnsafeCell<StackBuffer<N>>,
    borrows: Cell<usize>
}

impl<const N: usize> StackBufferCell<N> {

    pub fn new() -> Self {
        Self {
            borrows: Cell::new(0),
            buffer: UnsafeCell::new(StackBuffer::new())
        }
    }

    pub fn borrow(&self) -> BufferRef<'_, N> {
        if self.borrows.get() > 0 {
            panic!("borrowing buffer cell while borrowed");
        }

        self.borrows.update(|value| value + 1);
        BufferRef {
            cell: self,
            value: self.buffer.get(),
            execute_drop: true
        }
    }

}

pub struct BufferRef<'a, const N: usize> {
    cell: &'a StackBufferCell<N>,
    value: *mut StackBuffer<N>,
    execute_drop: bool
}

impl<'a, const N: usize> BufferRef<'a, N> {

    #[allow(dead_code)]
    pub fn map<T: 'a, F>(mut self, f: F) -> MappedBufferRef<'a, T, N> 
    where F: Fn(&'a [u8]) -> (usize, T) {

        self.execute_drop = false;

        let value = unsafe {
            (*self.value).reaable_data()
        };

        let (bytes_read, value) = f(value);

        MappedBufferRef{
            cell: self.cell,
            value,
            bytes_read
        }
    }

    // pub fn try_map<T: 'a, F, E>(mut self, f: F) -> Result<MappedBufferRef<'a, T, N>, E>
    // where F: Fn(&'a [u8]) -> Result<(usize, T), E> {

    //     let value = unsafe {
    //         (*self.value).reaable_data()
    //     };

    //     match f(value) {
    //         Ok((bytes_read, value)) => {
    //             self.execute_drop = false;
    //             Ok(MappedBufferRef{
    //                 cell: self.cell,
    //                 value,
    //                 bytes_read
    //             })
    //         },
    //         Err(err) => Err(err)
    //     }
    // }

    pub fn try_map_maybe<T: 'a, F, E>(mut self, f: F) -> Result<Option<MappedBufferRef<'a, T, N>>, E>
    where F: Fn(&'a [u8]) -> Result<Option<(usize, T)>, E> {

        let value = unsafe {
            (*self.value).reaable_data()
        };

        match f(value) {
            Ok(Some((bytes_read, value))) => {
                self.execute_drop = false;
                Ok(Some(MappedBufferRef{
                    cell: self.cell,
                    value,
                    bytes_read
                }))
            },
            Ok(None) => Ok(None),
            Err(err) => Err(err)
        }
    }

}

impl<'a, const N: usize> Drop for BufferRef<'a, N> {
    fn drop(&mut self) {
        if self.execute_drop {
            self.cell.borrows.update(|value| value - 1);
        }
    }
}

impl<'a, const N: usize> DerefMut for BufferRef<'a, N> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        unsafe {
            &mut (*self.value)
        }
    }
}

impl<'a, const N: usize> Deref for BufferRef<'a, N> {
    type Target = StackBuffer<N>;

    fn deref(&self) -> &Self::Target {
        unsafe {
            &(*self.value)
        }
    }
}

pub struct MappedBufferRef<'a, T, const N: usize> where T: 'a {
    cell: &'a StackBufferCell<N>,
    value: T,
    bytes_read: usize
}

impl<'a, T, const N: usize> Drop for MappedBufferRef<'a, T, N>
where T: 'a
{
    fn drop(&mut self) {
        unsafe {
            let buffer = self.cell.buffer.get();
            let buffer = &mut (*buffer);
            buffer.add_bytes_read(self.bytes_read).unwrap()
        }
        self.cell.borrows.update(|value| value - 1);
    }
}

impl<'a, T, const N: usize> Deref for MappedBufferRef<'a, T, N>
where T: 'a
{
    type Target = T;

    fn deref(&self) -> &Self::Target {
        &self.value
    }
}


#[derive(Debug, thiserror::Error)]
pub enum BufferError {
    #[error("the given operation ecceeds the buffers capatity")]
    OutOfBounds
}

pub struct StackBuffer<const N: usize> {
    data: [u8; N],
    read_position: usize,
    write_position: usize,
}

impl<const N: usize> StackBuffer<N> {

    pub fn new() -> Self {
        Self {
            data: [0; N],
            read_position: 0,
            write_position: 0
        }
    }

    pub fn reaable_data(&self) -> &[u8] {
        &self.data[self.read_position..self.write_position]
    }

    pub fn writeable_data(&mut self) -> &mut [u8] {
        if self.write_position == self.data.len() {
            self.flip();
        }

        &mut self.data[self.write_position..]
    }

    pub fn add_bytes_read(&mut self, bytes_read: usize) -> Result<(), BufferError> {
        if self.read_position + bytes_read > self.write_position {
            Err(BufferError::OutOfBounds)
        } else {
            self.read_position += bytes_read;
            Ok(())
        }
    }

    pub fn commit_bytes_written(&mut self, bytes_written: usize) -> Result<(), BufferError> {
        if self.write_position + bytes_written > self.data.len() {
            Err(BufferError::OutOfBounds)
        } else {
            self.write_position += bytes_written;
            Ok(())
        }
    }

    pub fn flip(&mut self) {
        if self.read_position > 0 {
            self.data.rotate_left(self.read_position);
            self.write_position -= self.read_position;
            self.read_position = 0;
        }
    }

    #[allow(dead_code)]
    pub fn push(&mut self, data: &[u8]) -> Result<(), BufferError> {
        self.flip();
        let writeable = self.writeable_data();
        if writeable.len() >= data.len() {
            writeable[..data.len()].copy_from_slice(data);
            self.commit_bytes_written(data.len())
        } else {
            Err(BufferError::OutOfBounds)   
        }
    }

    #[allow(dead_code)]
    pub fn capacity(&self) -> usize {
        self.data.len()
    }

    /// returns if the max capacity can be written to
    pub fn is_max_capacity(&self) -> bool {
        self.write_position == 0
    }

    /// returns if the max len can be read from
    pub fn is_max_len(&self) -> bool {
        self.read_position == 0 && self.write_position == self.data.len()
    }

    /// returns if there is data to read
    pub fn has_remaining_len(&self) -> bool {
        self.read_position < self.write_position
    }

    pub fn reset(&mut self) {
        self.write_position = 0;
        self.read_position = 0;
    }
}

#[cfg(test)]
mod tests {
    use crate::buffer::{StackBufferCell, MappedBufferRef, StackBuffer};



    #[test]
    fn test_write_to_end() {
        let mut buffer = StackBuffer::<4>::new();
        buffer.push(&[1, 2, 3, 4]).unwrap();

        assert_eq!(buffer.reaable_data(), &[1, 2, 3, 4]);

        assert_eq!(buffer.writeable_data().len(), 0);
    }

    #[test]
    fn test_buffer_cell_string_parsing() {
        let buffer = StackBufferCell::<32>::new();
        buffer.borrow().push("ich bin ein string".as_bytes()).unwrap();

        fn parse_string<const N: usize>(buf: &StackBufferCell<N>) -> MappedBufferRef<'_, &'_ str, N> {
            buf.borrow().map(|bytes| {
                let s = str::from_utf8(bytes).unwrap();
                let bytes_read = bytes.len();
                (bytes_read, s)
            })
        }

        let result = parse_string(&buffer);
        assert_eq!(*result, "ich bin ein string");
        drop(result);

        assert!(buffer.borrow().reaable_data().is_empty());
    }

    #[test]
    fn test_buffer_write_all() {
        let mut buffer = StackBuffer::<4>::new();
        let writeable = buffer.writeable_data();
        let writeable_len = writeable.len();
        buffer.commit_bytes_written(writeable_len).unwrap();
    }



}