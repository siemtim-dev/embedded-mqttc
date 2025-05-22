use embytes_buffer::new_stack_buffer;
use embedded_io::{Read, Write};



fn main () {

    // Create a new buffern with an array as byte source on the stack
    let mut buffer = new_stack_buffer::<1024>();

    // Write some bytes to buffer
    buffer.write_all("hello world".as_bytes()).unwrap();

    // read the bytes again
    let mut buf = [0; 128];
    let bytes_read = buffer.read(&mut buf).unwrap();

    assert_eq!("hello world".as_bytes(), &buf[..bytes_read]);

}

