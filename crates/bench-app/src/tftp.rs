//! Serves the firmware image to the device over TFTP (RFC 1350, with the
//! block size option of RFC 2348).

use std::fs::{self, File};
use std::io::{self, BufReader, Read};
use std::net::{SocketAddr, UdpSocket};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};

use bench_core::{
    block_count, data_packet, error_packet, negotiated_blksize, oack_packet, parse_ack, parse_rrq,
    server_address, serves, wire_block,
};

/// How often a wait checks whether it has been told to stop.
const POLL: Duration = Duration::from_millis(200);
const RETRANSMIT: Duration = Duration::from_secs(1);
const TRIES: u32 = 5;
const READ_BUFFER: usize = 64 * 1024;
/// Duplicate-address detection finishes in a few seconds. Longer than this,
/// the new address is not coming up.
const BIND_WAIT: Duration = Duration::from_secs(15);

/// UDP 69 at the server address. The device copy URL has no port, and 69 is
/// privileged, so only the elevated helper binds it.
pub fn device_bind() -> SocketAddr {
    let ip = server_address()
        .parse()
        .expect("the server address is an IP address");
    SocketAddr::new(ip, 69)
}

/// The image file and a socket already bound to the server address.
pub struct Prepared {
    file: File,
    size: u64,
    listener: UdpSocket,
}

/// Opens `path` and binds `bind`. Retries while the address is not assigned
/// yet, for a few seconds, then gives up. Returns only once the socket is
/// bound, so the caller can start the device copy after this succeeds.
pub fn prepare(path: &Path, bind: SocketAddr, stop: &AtomicBool) -> io::Result<Prepared> {
    let file = File::open(path)?;
    let size = file.metadata()?.len();
    let listener = bind_when_available(bind, stop, BIND_WAIT, bind_udp)?;
    Ok(Prepared {
        file,
        size,
        listener,
    })
}

/// Runs inside the elevated helper. Adds the laptop address, binds `bind`,
/// and serves until `job/stop` appears. Removes the address again on the
/// way out, including when the bind gives up.
pub fn run_server_job(bind: SocketAddr, job: &Path) -> io::Result<()> {
    let port = job_text(job.join("port"))?;
    let image = job_text(job.join("image"))?;
    let chosen = job_text(job.join("chosen"))?;
    if let Err(err) = crate::nic::apply_server_address(&port) {
        write_failure(job, "address", &err);
        return Err(err);
    }
    let watch = StopWatch::start(job.join("stop"));
    let stop = Arc::clone(watch.flag());
    let _added = AddedAddress { port: &port };
    let prepared = match prepare(Path::new(&image), bind, &stop) {
        Ok(prepared) => prepared,
        Err(err) => {
            write_failure(job, "bind", &err);
            return Err(err);
        }
    };
    fs::write(job.join("ready"), b"")?;
    let served = serve_prepared(prepared, &chosen, stop);
    match served {
        Err(err) if err.kind() == io::ErrorKind::Interrupted => Ok(()),
        other => other.map(|_| ()),
    }
}

/// The process entry for `--tftp-serve <bind> <job>`. `None` when those are
/// not the arguments, so the window can start.
pub fn elevated_main() -> Option<i32> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.first().map(String::as_str) != Some("--tftp-serve") {
        return None;
    }
    let code = match parse_serve_args(&args).and_then(|(bind, job)| run_server_job(bind, &job)) {
        Ok(()) => 0,
        Err(err) => {
            eprintln!("{err}");
            1
        }
    };
    Some(code)
}

fn parse_serve_args(args: &[String]) -> io::Result<(SocketAddr, PathBuf)> {
    if args.len() != 3 {
        return Err(io::Error::other(
            "usage: --tftp-serve <bind-address> <job-directory>",
        ));
    }
    let bind = args[1]
        .parse()
        .map_err(|_| io::Error::other(format!("{} is not the socket address to bind", args[1])))?;
    Ok((bind, PathBuf::from(&args[2])))
}

fn job_text(path: PathBuf) -> io::Result<String> {
    Ok(fs::read_to_string(path)?.trim().to_string())
}

fn write_failure(job: &Path, kind: &str, err: &io::Error) {
    let _ = fs::write(job.join("error"), format!("{kind}\n{err}"));
}

/// Removes the address this job added, when the server stops or the bind fails.
struct AddedAddress<'a> {
    port: &'a str,
}

impl Drop for AddedAddress<'_> {
    fn drop(&mut self) {
        let _ = crate::nic::release_server_address(self.port);
    }
}

/// Sets the stop flag when the parent creates the stop file.
struct StopWatch {
    stop: Arc<AtomicBool>,
    watcher: Option<thread::JoinHandle<()>>,
}

impl StopWatch {
    fn start(path: PathBuf) -> Self {
        let stop = Arc::new(AtomicBool::new(false));
        let flag = Arc::clone(&stop);
        let watcher = thread::spawn(move || {
            while !flag.load(Ordering::SeqCst) {
                if path.exists() {
                    flag.store(true, Ordering::SeqCst);
                    return;
                }
                thread::sleep(POLL);
            }
        });
        Self {
            stop,
            watcher: Some(watcher),
        }
    }

    fn flag(&self) -> &Arc<AtomicBool> {
        &self.stop
    }
}

impl Drop for StopWatch {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        if let Some(watcher) = self.watcher.take() {
            let _ = watcher.join();
        }
    }
}

/// Serves one read of `chosen` on a socket from [`prepare`] and returns the
/// bytes sent. A request for any other name gets File not found. Ends with
/// `Interrupted` once `stop` is set.
pub fn serve_prepared(prepared: Prepared, chosen: &str, stop: Arc<AtomicBool>) -> io::Result<u64> {
    let Prepared {
        file,
        size,
        listener,
    } = prepared;
    let ip = listener.local_addr()?.ip();
    listener.set_read_timeout(Some(POLL))?;
    let mut buf = [0u8; 1024];
    loop {
        check(&stop)?;
        let (len, peer) = match listener.recv_from(&mut buf) {
            Ok(received) => received,
            Err(err) if is_idle(&err) => continue,
            Err(err) => return Err(err),
        };
        let Some(request) = parse_rrq(&buf[..len]) else {
            continue;
        };
        let transfer = UdpSocket::bind(SocketAddr::new(ip, 0))?;
        transfer.connect(peer)?;
        if !serves(&request, chosen) {
            // The device asks again or gives up whether or not this arrives.
            let _ = transfer.send(&error_packet(1, "File not found"));
            continue;
        }
        let file = BufReader::with_capacity(READ_BUFFER, file);
        return send_file(&transfer, file, size, request.blksize, &stop);
    }
}

/// Windows can use a newly added address only once duplicate-address
/// detection has finished and the link is up. That wait is bounded: past
/// `wait`, the address is not coming up and the bind has failed.
fn bind_udp(bind: &SocketAddr) -> io::Result<UdpSocket> {
    UdpSocket::bind(bind)
}

fn bind_when_available(
    bind: SocketAddr,
    stop: &AtomicBool,
    wait: Duration,
    bind_socket: impl Fn(&SocketAddr) -> io::Result<UdpSocket>,
) -> io::Result<UdpSocket> {
    let deadline = Instant::now() + wait;
    loop {
        match bind_socket(&bind) {
            Err(err) if err.kind() == io::ErrorKind::AddrNotAvailable => {
                check(stop)?;
                if Instant::now() >= deadline {
                    return Err(err);
                }
                let pause = POLL.min(deadline.saturating_duration_since(Instant::now()));
                if !pause.is_zero() {
                    thread::sleep(pause);
                }
            }
            bound => return bound,
        }
    }
}

fn send_file(
    socket: &UdpSocket,
    mut file: impl Read,
    size: u64,
    requested: Option<u16>,
    stop: &AtomicBool,
) -> io::Result<u64> {
    let blksize = negotiated_blksize(requested);
    if requested.is_some() {
        deliver(socket, &oack_packet(blksize), 0, stop)?;
    }
    let mut payload = Vec::with_capacity(usize::from(blksize));
    let mut sent = 0;
    for index in 1..=block_count(size, blksize) {
        payload.clear();
        file.by_ref()
            .take(u64::from(blksize))
            .read_to_end(&mut payload)?;
        let block = wire_block(index);
        deliver(socket, &data_packet(block, &payload), block, stop)?;
        sent += payload.len() as u64;
    }
    Ok(sent)
}

/// Sends `packet` until the device acknowledges `block`, sending it again
/// after a second without that acknowledgement.
fn deliver(socket: &UdpSocket, packet: &[u8], block: u16, stop: &AtomicBool) -> io::Result<()> {
    let mut reply = [0u8; 1024];
    for _ in 0..TRIES {
        check(stop)?;
        socket.send(packet)?;
        let deadline = Instant::now() + RETRANSMIT;
        while let Some(left) = deadline
            .checked_duration_since(Instant::now())
            .filter(|left| !left.is_zero())
        {
            socket.set_read_timeout(Some(left))?;
            match socket.recv(&mut reply) {
                Ok(len) if parse_ack(&reply[..len]) == Some(block) => return Ok(()),
                Ok(len) if reply[..len].starts_with(&[0, 5]) => {
                    let message = String::from_utf8_lossy(&reply[4.min(len)..len]);
                    return Err(io::Error::other(format!(
                        "the device ended the transfer: {}",
                        message.trim_end_matches('\0')
                    )));
                }
                // A repeated acknowledgement of an earlier block needs no reply.
                Ok(_) => {}
                Err(err) if is_idle(&err) => break,
                Err(err) => return Err(err),
            }
        }
    }
    Err(io::Error::new(
        io::ErrorKind::TimedOut,
        format!("the device did not acknowledge block {block}"),
    ))
}

fn check(stop: &AtomicBool) -> io::Result<()> {
    if stop.load(Ordering::SeqCst) {
        Err(io::Error::new(
            io::ErrorKind::Interrupted,
            "the TFTP server was stopped",
        ))
    } else {
        Ok(())
    }
}

fn is_idle(err: &io::Error) -> bool {
    matches!(
        err.kind(),
        io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut | io::ErrorKind::Interrupted
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    use std::sync::atomic::AtomicUsize;
    use std::thread::JoinHandle;

    #[test]
    fn the_device_listens_on_udp_69() {
        assert_eq!(device_bind().to_string(), "192.0.2.1:69");
    }

    #[test]
    fn an_address_that_never_appears_stops_the_bind_wait() {
        let bind = "192.0.2.1:69".parse().unwrap();
        let stop = AtomicBool::new(false);
        let calls = AtomicUsize::new(0);
        let started = Instant::now();
        let err = bind_when_available(bind, &stop, Duration::from_millis(400), |_| {
            calls.fetch_add(1, Ordering::SeqCst);
            Err(io::Error::new(
                io::ErrorKind::AddrNotAvailable,
                "Can't assign requested address",
            ))
        })
        .unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::AddrNotAvailable);
        assert!(calls.load(Ordering::SeqCst) >= 2);
        let elapsed = started.elapsed();
        assert!(elapsed >= Duration::from_millis(350), "{elapsed:?}");
        assert!(elapsed < Duration::from_secs(2), "{elapsed:?}");
    }

    #[test]
    fn a_refused_bind_returns_without_waiting() {
        let bind = "192.0.2.1:69".parse().unwrap();
        let stop = AtomicBool::new(false);
        let calls = AtomicUsize::new(0);
        let started = Instant::now();
        let err = bind_when_available(bind, &stop, Duration::from_secs(30), |_| {
            calls.fetch_add(1, Ordering::SeqCst);
            Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "Permission denied",
            ))
        })
        .unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::PermissionDenied);
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        assert!(started.elapsed() < Duration::from_secs(1));
    }

    type Server = (u16, Arc<AtomicBool>, JoinHandle<io::Result<u64>>);

    fn image(name: &str, len: u32) -> (PathBuf, Vec<u8>) {
        let dir = std::env::temp_dir().join("bench-tftp-test");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join(name);
        let bytes: Vec<u8> = (0..len).map(|i| (i % 251) as u8).collect();
        std::fs::write(&path, &bytes).unwrap();
        (path, bytes)
    }

    /// Binds a free loopback port and serves `chosen` on it. The socket stays
    /// open, so another test cannot take the port before the server binds.
    fn start(path: &Path, chosen: &str) -> Server {
        let stop = Arc::new(AtomicBool::new(false));
        let prepared = prepare(path, SocketAddr::from(([127, 0, 0, 1], 0)), &stop).unwrap();
        let port = prepared.listener.local_addr().unwrap().port();
        let flag = Arc::clone(&stop);
        let chosen = chosen.to_string();
        let server = thread::spawn(move || serve_prepared(prepared, &chosen, flag));
        (port, stop, server)
    }

    fn client() -> UdpSocket {
        let client = UdpSocket::bind("127.0.0.1:0").unwrap();
        client
            .set_read_timeout(Some(Duration::from_millis(100)))
            .unwrap();
        client
    }

    fn rrq(name: &str, blksize: Option<u16>) -> Vec<u8> {
        let mut packet = vec![0, 1];
        packet.extend_from_slice(name.as_bytes());
        packet.extend_from_slice(b"\0octet\0");
        if let Some(size) = blksize {
            packet.extend_from_slice(format!("blksize\0{size}\0").as_bytes());
        }
        packet
    }

    /// The next packet, waiting up to three seconds.
    fn receive(client: &UdpSocket) -> (Vec<u8>, SocketAddr) {
        let mut buf = [0u8; 9000];
        for _ in 0..30 {
            if let Ok((len, from)) = client.recv_from(&mut buf) {
                return (buf[..len].to_vec(), from);
            }
        }
        panic!("the server sent nothing");
    }

    /// Sends `request` until the server answers, since it may not have bound yet.
    fn ask(client: &UdpSocket, port: u16, request: &[u8]) -> (Vec<u8>, SocketAddr) {
        let mut buf = [0u8; 9000];
        for _ in 0..50 {
            client.send_to(request, ("127.0.0.1", port)).unwrap();
            if let Ok((len, from)) = client.recv_from(&mut buf) {
                return (buf[..len].to_vec(), from);
            }
        }
        panic!("the server never answered the request");
    }

    fn ack(block: [u8; 2]) -> [u8; 4] {
        [0, 4, block[0], block[1]]
    }

    fn fetch(path: &Path, name: &str, blksize: Option<u16>) -> Vec<u8> {
        let (port, _, server) = start(path, name);
        let client = client();
        let (mut packet, from) = ask(&client, port, &rrq(name, blksize));
        assert_ne!(from.port(), port, "the transfer needs its own transfer ID");
        let size = usize::from(blksize.unwrap_or(512));
        let mut received = Vec::new();
        let mut expected: u16 = 1;
        loop {
            match packet[1] {
                6 if blksize.is_some() => {
                    assert_eq!(packet, oack_packet(size as u16));
                    client.send_to(&ack([0, 0]), from).unwrap();
                }
                3 => {
                    assert_eq!(u16::from_be_bytes([packet[2], packet[3]]), expected);
                    received.extend_from_slice(&packet[4..]);
                    client.send_to(&ack([packet[2], packet[3]]), from).unwrap();
                    if packet.len() - 4 < size {
                        break;
                    }
                    expected = expected.wrapping_add(1);
                }
                _ => panic!("unexpected packet {packet:?}"),
            }
            let (next, sender) = receive(&client);
            assert_eq!(sender, from);
            packet = next;
        }
        assert_eq!(
            server.join().unwrap().unwrap(),
            received.len() as u64,
            "serve returns the bytes sent"
        );
        received
    }

    fn fetch_over_loopback(path: &Path, name: &str, blksize: u16) -> Vec<u8> {
        fetch(path, name, Some(blksize))
    }

    #[test]
    fn serve_sends_the_whole_file_over_loopback() {
        let dir = std::env::temp_dir().join("bench-tftp-test");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("cat9k_iosxe.17.12.05.SPA.bin");
        let bytes: Vec<u8> = (0..1500u32).map(|i| (i % 251) as u8).collect();
        std::fs::write(&path, &bytes).unwrap();
        let received = fetch_over_loopback(&path, "cat9k_iosxe.17.12.05.SPA.bin", 512);
        assert_eq!(received, bytes);
    }

    #[test]
    fn without_blksize_the_data_starts_at_once_in_512_byte_blocks() {
        let (path, bytes) = image("default-blocks.17.12.05.bin", 1024);
        assert_eq!(fetch(&path, "default-blocks.17.12.05.bin", None), bytes);
    }

    #[test]
    fn another_name_gets_file_not_found_and_stop_ends_the_wait() {
        let (path, _) = image("chosen.17.12.05.bin", 10);
        let (port, stop, server) = start(&path, "chosen.17.12.05.bin");
        let (reply, _) = ask(&client(), port, &rrq("other.bin", None));
        assert_eq!(reply, error_packet(1, "File not found"));
        stop.store(true, Ordering::SeqCst);
        let err = server.join().unwrap().unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::Interrupted);
    }

    #[test]
    fn a_block_without_an_ack_is_sent_again_after_a_second() {
        let (path, bytes) = image("resend.17.12.05.bin", 100);
        let (port, _, server) = start(&path, "resend.17.12.05.bin");
        let client = client();
        let (first, from) = ask(&client, port, &rrq("resend.17.12.05.bin", None));
        let (again, _) = receive(&client);
        assert_eq!(again, first);
        assert_eq!(&first[4..], &bytes[..]);
        client.send_to(&ack([0, 1]), from).unwrap();
        assert_eq!(server.join().unwrap().unwrap(), 100);
    }
}
