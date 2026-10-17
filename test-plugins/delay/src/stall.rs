//! The barrier the stall variants hold a call at, which the test releases.

use std::io::{Read, Write};
use std::net::{SocketAddr, TcpStream};

/// Announces entry to the test's barrier at `address` and holds up the calling thread until the
/// test explicitly releases or disconnects. No timer can end it prematurely.
pub fn stall(address: Option<SocketAddr>) {
    let address = address.expect(crate::errors::STALL_ADDRESS);
    let mut connection = TcpStream::connect(address).expect(crate::errors::STALL_CONNECT);
    connection
        .write_all(&[1])
        .expect(crate::errors::STALL_CONNECT);
    let _ = connection.read_exact(&mut [0]);
}
