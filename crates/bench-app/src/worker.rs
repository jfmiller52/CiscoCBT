//! The thread that carries out a bench run on the device and reports it to
//! the window.

use std::cell::RefCell;
use std::fs;
use std::io;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, RecvTimeoutError};
use std::sync::Arc;
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use bench_core::{
    export_text, server_address, CheckStatus, Phase, RunFiles, RunOptions, Runner, Session,
};

use crate::awake::KeepAwake;
use crate::file_name;
use crate::nic;
use crate::serial::{self, SerialConsole};
use crate::tftp;
use crate::view::{UiCommand, ViewState};

const MINUTE: Duration = Duration::from_secs(60);
const HOUR: Duration = Duration::from_secs(60 * 60);
const TFTP_PORT: u16 = 69;

/// Starts the thread that carries out each `Start` on the device. It sends
/// `updates` a whole `ViewState` whenever the run changes.
pub fn spawn(commands: Receiver<UiCommand>, updates: impl Fn(ViewState) + Send + 'static) {
    thread::spawn(move || Worker::new(Box::new(updates)).serve(commands));
}

/// The last `ViewState` sent to the window. Console output that arrives
/// during a long read is added to its transcript and sent again.
struct Window {
    updates: Box<dyn Fn(ViewState)>,
    latest: RefCell<ViewState>,
}

impl Window {
    fn send(&self, view: ViewState) {
        (self.updates)(view.clone());
        *self.latest.borrow_mut() = ViewState {
            export_text: None,
            ..view
        };
    }

    fn echo(&self, text: &str) {
        let mut latest = self.latest.borrow_mut();
        latest.transcript.push_str(text);
        (self.updates)(latest.clone());
    }
}

struct Worker {
    window: Rc<Window>,
    serial_devices: Vec<String>,
    ethernet_ports: Vec<String>,
    run: Option<Run>,
    start_failure: Option<String>,
    export: Option<String>,
}

impl Worker {
    fn new(updates: Box<dyn Fn(ViewState)>) -> Self {
        Self {
            window: Rc::new(Window {
                updates,
                latest: RefCell::new(ViewState::new()),
            }),
            serial_devices: serial::serial_devices(),
            ethernet_ports: nic::ethernet_ports(),
            run: None,
            start_failure: None,
            export: None,
        }
    }

    /// Handles commands between steps of the run, and returns once the
    /// window has gone.
    fn serve(mut self, commands: Receiver<UiCommand>) {
        loop {
            let received = match self.run.as_ref().and_then(Run::wait) {
                Some(wait) => commands.recv_timeout(wait),
                None => commands.recv().map_err(|_| RecvTimeoutError::Disconnected),
            };
            match received {
                Ok(command) => self.handle(command),
                Err(RecvTimeoutError::Timeout) => {
                    if let Some(run) = &mut self.run {
                        run.step();
                    }
                }
                Err(RecvTimeoutError::Disconnected) => return,
            }
            self.publish();
        }
    }

    fn handle(&mut self, command: UiCommand) {
        match command {
            UiCommand::Start(options) => self.start(options),
            UiCommand::ConfirmCable => {
                if let Some(run) = &mut self.run {
                    run.runner.confirm_cable();
                    run.stalled = false;
                }
            }
            UiCommand::CheckPort(name) => {
                if let Some(run) = &mut self.run {
                    run.port_error = run
                        .runner
                        .check_port(&name)
                        .err()
                        .map(|err| err.to_string());
                }
            }
            UiCommand::Export => {
                self.export = self
                    .run
                    .as_ref()
                    .map(|run| export_text(&run.runner.export_input()));
            }
        }
    }

    /// A run in progress keeps the console, so `Start` waits for it to end.
    fn start(&mut self, options: RunOptions) {
        if self.run.as_ref().is_some_and(|run| !run.ended()) {
            return;
        }
        self.run = None;
        self.start_failure = None;
        match Run::open(options, &self.window) {
            Ok(run) => self.run = Some(run),
            Err(reason) => self.start_failure = Some(reason),
        }
    }

    fn publish(&mut self) {
        let mut view = ViewState::new();
        view.serial_devices = self.serial_devices.clone();
        view.ethernet_ports = self.ethernet_ports.clone();
        view.export_text = self.export.take();
        view.stop_reason = self.start_failure.clone();
        if let Some(run) = &self.run {
            run.describe(&mut view);
        }
        self.window.send(view);
    }
}

/// One run on one device, with what this computer does for it.
struct Run {
    runner: Runner<SerialConsole>,
    options: RunOptions,
    awake: Option<KeepAwake>,
    /// The last `advance` left the phase where it was, so the run waits on
    /// the technician or the burn-in clock.
    stalled: bool,
    burn_in_ends: Option<Instant>,
    /// Problems on this computer, kept for the rest of the run.
    notes: Vec<String>,
    port_error: Option<String>,
}

/// The TFTP server for one image copy.
struct Tftp {
    stop: Arc<AtomicBool>,
    server: JoinHandle<io::Result<u64>>,
    /// The port that was given the server address for this copy.
    address_on: Option<String>,
}

impl Run {
    fn open(options: RunOptions, window: &Rc<Window>) -> Result<Self, String> {
        let config_body = fs::read_to_string(&options.config_path).map_err(|err| {
            format!(
                "The configuration file {} could not be read: {err}",
                options.config_path
            )
        })?;
        let files = RunFiles {
            config_name: file_name(&options.config_path),
            config_body,
            image_name: options
                .firmware
                .as_ref()
                .map(|firmware| file_name(&firmware.image_path)),
        };
        let window = Rc::clone(window);
        let echo = Box::new(move |text: &str| window.echo(text));
        let console = SerialConsole::open(&options.serial_device, echo).map_err(|err| {
            format!(
                "The serial device {} could not be opened: {err}",
                options.serial_device
            )
        })?;
        let mut notes = Vec::new();
        let awake = match KeepAwake::new() {
            Ok(awake) => Some(awake),
            Err(err) => {
                notes.push(format!("This computer may sleep during the run: {err}"));
                None
            }
        };
        Ok(Self {
            runner: Runner::new(Session::new(console), options.clone(), files),
            options,
            awake,
            stalled: false,
            burn_in_ends: None,
            notes,
            port_error: None,
        })
    }

    fn ended(&self) -> bool {
        matches!(self.runner.phase(), Phase::Done | Phase::Stopped)
    }

    /// How long to wait for a command: not at all while the run can advance,
    /// at most a minute while the burn-in clock runs, otherwise until one comes.
    fn wait(&self) -> Option<Duration> {
        if !self.stalled {
            return Some(Duration::ZERO);
        }
        let ends = self.burn_in_ends?;
        (self.runner.phase() == Phase::WaitBurnIn)
            .then(|| ends.saturating_duration_since(Instant::now()).min(MINUTE))
    }

    /// Advances the run one phase, or ends the burn-in wait once its time is up.
    fn step(&mut self) {
        if self.stalled {
            let time_up = self.burn_in_ends.is_some_and(|ends| Instant::now() >= ends);
            if self.runner.phase() == Phase::WaitBurnIn && time_up {
                self.runner.finish_wait();
                self.stalled = false;
            }
            return;
        }
        let before = self.runner.phase();
        let tftp = if before == Phase::CopyImage {
            self.start_tftp()
        } else {
            None
        };
        let after = self.runner.advance();
        if let Some(tftp) = tftp {
            self.finish_tftp(tftp);
        }
        if after == Phase::WaitBurnIn && before != Phase::WaitBurnIn {
            let hours = self.options.burn_in_hours.unwrap_or_default();
            self.burn_in_ends = Instant::now().checked_add(HOUR * hours);
        }
        if self.ended() {
            self.awake = None;
        }
        self.stalled = after == before;
    }

    fn start_tftp(&mut self) -> Option<Tftp> {
        let firmware = self.options.firmware.as_ref()?;
        let port = &firmware.local_ethernet;
        let address_on = match nic::add_server_address(port) {
            Ok(()) => Some(port.clone()),
            Err(err) => {
                self.notes.push(format!(
                    "{port} could not be given the address {}: {err}",
                    server_address()
                ));
                None
            }
        };
        let address = server_address()
            .parse()
            .expect("the server address is an IP address");
        let bind = SocketAddr::new(address, TFTP_PORT);
        let path = PathBuf::from(&firmware.image_path);
        let chosen = file_name(&firmware.image_path);
        let stop = Arc::new(AtomicBool::new(false));
        let flag = Arc::clone(&stop);
        let server = thread::spawn(move || tftp::serve(&path, &chosen, bind, flag));
        Some(Tftp {
            stop,
            server,
            address_on,
        })
    }

    fn finish_tftp(&mut self, tftp: Tftp) {
        tftp.stop.store(true, Ordering::SeqCst);
        match tftp.server.join() {
            Ok(Ok(_)) => {}
            Ok(Err(err)) if err.kind() == io::ErrorKind::Interrupted => {}
            Ok(Err(err)) => self
                .notes
                .push(format!("The TFTP server on this computer failed: {err}")),
            Err(_) => self
                .notes
                .push("The TFTP server on this computer stopped unexpectedly.".into()),
        }
        if let Some(port) = tftp.address_on {
            if let Err(err) = nic::remove_server_address(&port) {
                self.notes.push(format!(
                    "The address {} could not be removed from {port}: {err}",
                    server_address()
                ));
            }
        }
    }

    fn describe(&self, view: &mut ViewState) {
        let runner = &self.runner;
        let session = runner.session();
        view.phase = Some(runner.phase());
        view.record = runner.record().clone();
        view.reminder = runner.reminder().cloned();
        view.tftp_port = runner.tftp_port().map(String::from);
        view.ports = runner.ports().to_vec();
        view.current_port = current_port(runner.ports());
        view.burn_in_remaining = self
            .burn_in_ends
            .filter(|_| runner.phase() == Phase::WaitBurnIn)
            .map(|ends| ends.saturating_duration_since(Instant::now()));
        view.transcript = format!("{}{}", session.transcript(), session.console().unread());
        view.hostname = Some(session.hostname())
            .filter(|name| !name.is_empty())
            .map(String::from);
        let reasons: Vec<&str> = runner
            .stop_reason()
            .into_iter()
            .chain(self.notes.iter().map(String::as_str))
            .chain(self.port_error.as_deref())
            .collect();
        view.stop_reason = (!reasons.is_empty()).then(|| reasons.join("\n\n"));
    }
}

/// The first port of the walk that has not passed.
fn current_port(ports: &[(String, CheckStatus)]) -> Option<String> {
    ports
        .iter()
        .find(|(_, status)| *status != CheckStatus::Passed)
        .map(|(name, _)| name.clone())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc;

    fn options(serial_device: &str, config_path: &str) -> RunOptions {
        RunOptions {
            serial_device: serial_device.into(),
            config_path: config_path.into(),
            firmware: None,
            burn_in_hours: None,
            port_testing: false,
        }
    }

    /// Sends `command` to a new worker and returns the first view it publishes.
    fn first_view(command: UiCommand) -> ViewState {
        let (commands, receiver) = mpsc::channel();
        let (updates, views) = mpsc::channel();
        spawn(receiver, move |view| {
            let _ = updates.send(view);
        });
        commands.send(command).unwrap();
        views.recv_timeout(Duration::from_secs(10)).unwrap()
    }

    #[test]
    fn an_unreadable_configuration_file_stops_the_start_with_a_reason() {
        let path = "/no/such/closet-access.cfg";
        let view = first_view(UiCommand::Start(options("/dev/cu.bench-test", path)));
        assert_eq!(view.phase, None);
        let reason = view.stop_reason.unwrap();
        assert!(
            reason.starts_with(
                "The configuration file /no/such/closet-access.cfg could not be read:"
            ),
            "{reason}"
        );
    }

    #[test]
    fn a_serial_device_that_cannot_be_opened_stops_the_start_with_a_reason() {
        let dir = std::env::temp_dir().join("bench-worker-test");
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join("closet-access.cfg");
        fs::write(&path, "hostname closet-2-sw1\n").unwrap();
        let start = options("/dev/cu.no-such-console", &path.display().to_string());
        let view = first_view(UiCommand::Start(start));
        assert_eq!(view.phase, None);
        let reason = view.stop_reason.unwrap();
        assert!(
            reason.starts_with("The serial device /dev/cu.no-such-console could not be opened:"),
            "{reason}"
        );
    }

    #[test]
    fn console_output_extends_the_last_view_without_repeating_the_export() {
        let sent = Rc::new(RefCell::new(Vec::new()));
        let log = Rc::clone(&sent);
        let window = Window {
            updates: Box::new(move |view| log.borrow_mut().push(view)),
            latest: RefCell::new(ViewState::new()),
        };
        let mut view = ViewState::new();
        view.phase = Some(Phase::Login);
        view.transcript = "\r\nSwitch>".into();
        view.export_text = Some("Host name: closet-2-sw1\n".into());
        window.send(view);
        window.echo("enable\r\nSwitch#");
        let sent = sent.borrow();
        assert_eq!(sent.len(), 2);
        assert_eq!(
            sent[0].export_text.as_deref(),
            Some("Host name: closet-2-sw1\n")
        );
        assert_eq!(sent[1].phase, Some(Phase::Login));
        assert_eq!(sent[1].transcript, "\r\nSwitch>enable\r\nSwitch#");
        assert_eq!(sent[1].export_text, None);
    }

    #[test]
    fn the_current_port_is_the_first_that_has_not_passed() {
        let failed = CheckStatus::Failed {
            detail: "down".into(),
        };
        let ports = vec![
            ("GigabitEthernet1/0/1".to_string(), CheckStatus::Passed),
            ("GigabitEthernet1/0/2".to_string(), failed),
            ("GigabitEthernet1/0/3".to_string(), CheckStatus::NotFinished),
        ];
        assert_eq!(current_port(&ports), Some("GigabitEthernet1/0/2".into()));
        assert_eq!(current_port(&ports[..1]), None);
        assert_eq!(current_port(&[]), None);
    }
}
