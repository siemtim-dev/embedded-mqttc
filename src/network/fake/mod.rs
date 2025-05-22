//! Fake Network
//! 
//! Provides an implementation of network connection that does not do anything network-realated
//! This can be used in tests.

mod stream;
pub use stream::*;

mod connection;
pub use connection::*;