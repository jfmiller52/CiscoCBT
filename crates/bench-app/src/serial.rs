//! The console cable: the serial devices on this computer, and a `Console`
//! over the chosen one.

use std::collections::HashSet;
use std::io::{self, Read, Write};
use std::time::{Duration, Instant};

use bench_core::{cisco_console, Console, SerialSettings};
use regex::Regex;
use serialport::{DataBits, FlowControl, Parity, StopBits};

/// How long one read waits before the deadline is checked again.
const POLL: Duration = Duration::from_millis(100);
/// While the device keeps printing, its output reaches the window this often.
const ECHO_EVERY: Duration = Duration::from_millis(250);

/// The byte stream to the device.
trait Link: Read + Write {}

impl<T: Read + Write> Link for T {}

/// The serial devices on this computer, named as `SerialConsole::open` takes them.
pub fn serial_devices() -> Vec<String> {
    let names = serialport::available_ports()
        .map(|ports| ports.into_iter().map(|port| port.port_name).collect())
        .unwrap_or_default();
    prefer_callout(names)
}

/// macOS lists each serial device twice. The `/dev/tty.*` twin is for
/// answering calls and waits for carrier detect, which a console cable does
/// not raise, so only the `/dev/cu.*` twin is kept.
pub fn prefer_callout(names: Vec<String>) -> Vec<String> {
    let callouts: HashSet<String> = names
        .iter()
        .filter_map(|name| name.strip_prefix("/dev/cu."))
        .map(String::from)
        .collect();
    names
        .into_iter()
        .filter(|name| {
            name.strip_prefix("/dev/tty.")
                .is_none_or(|suffix| !callouts.contains(suffix))
        })
        .collect()
}

/// A `Console` over a serial device at the Cisco console settings. Device
/// output goes to `echo` as it arrives, so the technician can watch a boot.
pub struct SerialConsole {
    link: Box<dyn Link>,
    unread: String,
    unechoed: String,
    echoed_at: Instant,
    echo: Box<dyn FnMut(&str)>,
}

impl SerialConsole {
    pub fn open(path: &str, echo: Box<dyn FnMut(&str)>) -> io::Result<Self> {
        let settings = cisco_console();
        let (data_bits, parity, stop_bits) = line_settings(settings)?;
        let port = serialport::new(path, settings.baud)
            .data_bits(data_bits)
            .parity(parity)
            .stop_bits(stop_bits)
            .flow_control(FlowControl::None)
            .timeout(POLL)
            .open()?;
        Ok(Self::over(Box::new(port), echo))
    }

    fn over(link: Box<dyn Link>, echo: Box<dyn FnMut(&str)>) -> Self {
        Self {
            link,
            unread: String::new(),
            unechoed: String::new(),
            echoed_at: Instant::now(),
            echo,
        }
    }

    /// Device output that no `read_until` has consumed yet.
    pub fn unread(&self) -> &str {
        &self.unread
    }

    fn flush_echo(&mut self) {
        if !self.unechoed.is_empty() {
            (self.echo)(&self.unechoed);
            self.unechoed.clear();
        }
        self.echoed_at = Instant::now();
    }
}

impl Console for SerialConsole {
    fn write(&mut self, text: &str) -> io::Result<()> {
        self.link.write_all(text.as_bytes())?;
        self.link.flush()
    }

    fn read_until(&mut self, patterns: &[Regex], timeout: Duration) -> io::Result<(usize, String)> {
        let deadline = Instant::now() + timeout;
        let mut buf = [0u8; 1024];
        loop {
            if let Some((index, end)) = first_match(patterns, &self.unread) {
                self.flush_echo();
                return Ok((index, self.unread.drain(..end).collect()));
            }
            if Instant::now() >= deadline {
                self.flush_echo();
                return Err(io::Error::new(
                    io::ErrorKind::TimedOut,
                    "the device output matched nothing in time",
                ));
            }
            match self.link.read(&mut buf) {
                Ok(0) => {
                    return Err(io::Error::new(
                        io::ErrorKind::UnexpectedEof,
                        "the serial device closed",
                    ))
                }
                Ok(len) => {
                    let text = String::from_utf8_lossy(&buf[..len]);
                    self.unread.push_str(&text);
                    self.unechoed.push_str(&text);
                    if self.echoed_at.elapsed() >= ECHO_EVERY {
                        self.flush_echo();
                    }
                }
                Err(err) if is_idle(&err) => self.flush_echo(),
                Err(err) => return Err(err),
            }
        }
    }
}

/// The index of the pattern whose match in `text` ends first, ties going to
/// the lower index, and where that match ends.
fn first_match(patterns: &[Regex], text: &str) -> Option<(usize, usize)> {
    patterns
        .iter()
        .enumerate()
        .filter_map(|(index, pattern)| pattern.find(text).map(|found| (found.end(), index)))
        .min()
        .map(|(end, index)| (index, end))
}

fn is_idle(err: &io::Error) -> bool {
    matches!(
        err.kind(),
        io::ErrorKind::TimedOut | io::ErrorKind::WouldBlock | io::ErrorKind::Interrupted
    )
}

fn line_settings(settings: SerialSettings) -> io::Result<(DataBits, Parity, StopBits)> {
    let unsupported = |what: String| io::Error::new(io::ErrorKind::InvalidInput, what);
    let data_bits = DataBits::try_from(settings.data_bits)
        .map_err(|()| unsupported(format!("{} data bits", settings.data_bits)))?;
    let parity = match settings.parity {
        "none" => Parity::None,
        "odd" => Parity::Odd,
        "even" => Parity::Even,
        other => return Err(unsupported(format!("{other} parity"))),
    };
    let stop_bits = StopBits::try_from(settings.stop_bits)
        .map_err(|()| unsupported(format!("{} stop bits", settings.stop_bits)))?;
    Ok((data_bits, parity, stop_bits))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;
    use std::collections::VecDeque;
    use std::rc::Rc;

    /// A serial line that delivers one chunk per read, then times out.
    struct Chunks(VecDeque<&'static str>);

    impl Read for Chunks {
        fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
            let Some(chunk) = self.0.pop_front() else {
                return Err(io::ErrorKind::TimedOut.into());
            };
            buf[..chunk.len()].copy_from_slice(chunk.as_bytes());
            Ok(chunk.len())
        }
    }

    impl Write for Chunks {
        fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
            Ok(buf.len())
        }

        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    fn console(chunks: &[&'static str]) -> (SerialConsole, Rc<RefCell<String>>) {
        let echoed = Rc::new(RefCell::new(String::new()));
        let sink = Rc::clone(&echoed);
        let link = Chunks(chunks.iter().copied().collect());
        let console = SerialConsole::over(
            Box::new(link),
            Box::new(move |text| sink.borrow_mut().push_str(text)),
        );
        (console, echoed)
    }

    fn patterns(sources: &[&str]) -> Vec<Regex> {
        sources.iter().map(|s| Regex::new(s).unwrap()).collect()
    }

    #[test]
    fn callout_devices_replace_tty_twins() {
        let names = vec![
            "/dev/tty.usbserial-1420".to_string(),
            "/dev/cu.usbserial-1420".to_string(),
            "COM3".to_string(),
        ];
        let kept = prefer_callout(names);
        assert!(kept.contains(&"/dev/cu.usbserial-1420".to_string()));
        assert!(!kept.contains(&"/dev/tty.usbserial-1420".to_string()));
        assert!(kept.contains(&"COM3".to_string()));
    }

    #[test]
    fn a_tty_device_without_a_callout_twin_stays() {
        let names = vec!["/dev/tty.Bluetooth-Modem".to_string()];
        assert_eq!(prefer_callout(names.clone()), names);
    }

    #[test]
    fn a_prompt_split_across_reads_matches_the_whole_unread_output() {
        let (mut console, echoed) = console(&["\r\nPass", "word: "]);
        let found = console
            .read_until(
                &patterns(&[r"(?i)password\s*:\s*$"]),
                Duration::from_secs(1),
            )
            .unwrap();
        assert_eq!(found, (0, "\r\nPassword: ".to_string()));
        assert_eq!(*echoed.borrow(), "\r\nPassword: ");
    }

    #[test]
    fn the_match_that_ends_first_wins_and_the_rest_stays_unread() {
        let (mut console, _) = console(&["xab"]);
        let set = patterns(&["b", "ab", "x"]);
        let wait = Duration::from_secs(1);
        assert_eq!(console.read_until(&set, wait).unwrap(), (2, "x".into()));
        assert_eq!(console.unread(), "ab");
        assert_eq!(console.read_until(&set, wait).unwrap(), (0, "ab".into()));
        assert_eq!(console.unread(), "");
    }

    #[test]
    fn nothing_matching_times_out_and_keeps_and_echoes_the_output() {
        let (mut console, echoed) = console(&["boot ", "log"]);
        let err = console
            .read_until(&patterns(&["login:"]), Duration::from_millis(50))
            .unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::TimedOut);
        assert_eq!(console.unread(), "boot log");
        assert_eq!(*echoed.borrow(), "boot log");
    }
}
