
This libray contains a buffer implementation that is made for `no_std` environments. 

The buffer is backed ba any bytes source that satisfies `AsMut<[u8]> + AsRef<[u8]>`.

the buffer implemnts `embedded_io::Read`, `embedded_io::Write`, `std::io::Read` and `std::io::Write`.

# Example

```rust
// Create a new buffern with an array as byte source on the stack
let mut buffer = new_stack_buffer::<1024>();

// Write some bytes to buffer
buffer.write_all("hello world".as_bytes()).unwrap();

// read the bytes again
let mut buf = [0; 128];
let bytes_read = buffer.read(&mut buf).unwrap();

```

See examples directory for more examples

