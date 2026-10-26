//! The local channel between the command and its warm server, and how that server is started apart from the
//! terminal. A Unix socket where the OS has one; on Windows, TCP on the loopback address, guarded by a random
//! token kept in the user's own temporary folder.

#[cfg(unix)]
mod platform {
    use std::io;
    use std::os::unix::net::{UnixListener, UnixStream};
    use std::os::unix::process::CommandExt;
    use std::path::Path;
    use std::process::Command;

    pub type Stream = UnixStream;
    pub struct Listener(UnixListener);

    pub fn connect(address: &Path) -> io::Result<Stream> {
        UnixStream::connect(address)
    }

    pub fn listen(address: &Path) -> io::Result<Listener> {
        let _ = std::fs::remove_file(address);
        UnixListener::bind(address).map(Listener)
    }

    impl Listener {
        pub fn accept(&self) -> io::Result<Stream> {
            self.0.accept().map(|(stream, _)| stream)
        }
    }

    /// Its own process group, so closing the terminal that started it does not stop it.
    pub fn detach(command: &mut Command) {
        command.process_group(0);
    }
}

#[cfg(windows)]
mod platform {
    use std::collections::hash_map::RandomState;
    use std::hash::{BuildHasher, Hasher};
    use std::io::{self, Read, Write};
    use std::net::{Ipv4Addr, TcpListener, TcpStream};
    use std::os::windows::process::CommandExt;
    use std::path::Path;
    use std::process::Command;

    const DETACHED_PROCESS: u32 = 0x0000_0008;
    const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;

    pub type Stream = TcpStream;
    pub struct Listener {
        inner: TcpListener,
        token: String,
    }

    fn random_token() -> String {
        (0..2).map(|_| format!("{:016x}", RandomState::new().build_hasher().finish())).collect()
    }

    /// The address file holds `port token`.
    pub fn connect(address: &Path) -> io::Result<Stream> {
        let content = std::fs::read_to_string(address)?;
        let (port, token) = content.trim().split_once(' ').ok_or_else(|| io::Error::other("malformed address file"))?;
        let port: u16 = port.parse().map_err(io::Error::other)?;
        let mut stream = TcpStream::connect((Ipv4Addr::LOCALHOST, port))?;
        writeln!(stream, "{token}")?;
        Ok(stream)
    }

    pub fn listen(address: &Path) -> io::Result<Listener> {
        let inner = TcpListener::bind((Ipv4Addr::LOCALHOST, 0))?;
        let token = random_token();
        std::fs::write(address, format!("{} {token}", inner.local_addr()?.port()))?;
        Ok(Listener { inner, token })
    }

    impl Listener {
        /// Connections that do not open with the token are dropped.
        pub fn accept(&self) -> io::Result<Stream> {
            loop {
                let (mut stream, _) = self.inner.accept()?;
                // Byte by byte: a buffered reader would swallow the request that follows the token.
                let mut line = Vec::new();
                let mut byte = [0u8; 1];
                while line.len() <= self.token.len() && stream.read(&mut byte)? == 1 && byte[0] != b'\n' {
                    line.push(byte[0]);
                }
                if line == self.token.as_bytes() {
                    return Ok(stream);
                }
            }
        }
    }

    pub fn detach(command: &mut Command) {
        command.creation_flags(DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP);
    }
}

pub use platform::{connect, detach, listen, Stream};

/// Whether the asker hung up (a client stopped by a timeout): a closed connection reads as its end, one still
/// waiting has nothing to read yet (it sends nothing after its question, so reading takes nothing from it).
pub fn hung_up(stream: &Stream) -> bool {
    if stream.set_nonblocking(true).is_err() {
        return false;
    }
    let gone = matches!(std::io::Read::read(&mut &*stream, &mut [0u8; 1]), Ok(0));
    let _ = stream.set_nonblocking(false);
    gone
}
