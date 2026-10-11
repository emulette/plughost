//! An explicit entered/release barrier for the native main-thread stall fixture.
use std::io::{Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::sync::mpsc::{self, Receiver};
use std::thread::{self, JoinHandle};
use std::time::Duration;

pub struct Stall {
    address: SocketAddr,
    entered: Receiver<(TcpStream, u8)>,
    worker: Option<JoinHandle<()>>,
    connection: Option<TcpStream>,
}

impl Stall {
    pub fn new() -> Self {
        let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let address = listener.local_addr().unwrap();
        let (entered, receiver) = mpsc::channel();
        let worker = thread::spawn(move || {
            let (mut connection, _) = listener.accept().unwrap();
            connection
                .set_read_timeout(Some(Duration::from_secs(10)))
                .unwrap();
            let mut signal = [0];
            let _ = connection.read_exact(&mut signal);
            let _ = entered.send((connection, signal[0]));
        });
        Self {
            address,
            entered: receiver,
            worker: Some(worker),
            connection: None,
        }
    }

    pub fn identity(&self) -> plughost::HostIdentity {
        plughost::HostIdentity {
            name: self.address.to_string(),
            ..Default::default()
        }
    }

    pub fn wait_until_entered(&mut self) {
        let (connection, signal) = self.entered.recv_timeout(Duration::from_secs(10)).unwrap();
        assert_eq!(signal, 1, "fixture did not announce its stall");
        self.connection = Some(connection);
        self.worker.take().unwrap().join().unwrap();
    }
}

impl Drop for Stall {
    fn drop(&mut self) {
        // Also unblock accept if a broken fixture never entered, so failures leave no waiter.
        if let Some(worker) = self.worker.take() {
            if let Ok(mut connection) = TcpStream::connect(self.address) {
                let _ = connection.write_all(&[0]);
            }
            let _ = worker.join();
        }
        if let Some(mut connection) = self.connection.take() {
            let _ = connection.write_all(&[0]);
        }
    }
}
