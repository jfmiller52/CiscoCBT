//! Carries out the run order on a device over its console session and keeps
//! the run record, the reminder, and the port results.

use crate::config_check::missing_commands;
use crate::export::{chassis_serial, ExportInput};
use crate::firmware::{firmware_check, version_token, FirmwareCheck};
use crate::login::{login_after_apply, FactorySecrets, NextLogin};
use crate::phase::{next_phase, port_walk_available, start_phase, Phase, PhaseEvent};
use crate::ports::{first_physical, physical_interfaces, router_port_passed, switch_port_passed};
use crate::run::{CheckStatus, RunOptions, RunRecord};
use crate::session::{Console, Session, SessionError};
use crate::transfer::{assign_address_commands, copy_tftp_command, LinkKind};
use crate::uptime::{burn_in_passes, parse_uptime_lines};

const SWITCH_MARKERS: [&str; 3] = ["cisco c9", "cisco ie-", "switch ports model"];

const NO_IMAGE_PORT: &str = "No physical interface was found for the image cable.";
const NO_VERSION: &str = "The image file name has no version number.";
const REJECTED_ADDRESS: &str = "The device rejected the temporary address for the image copy:";
const UNTYPABLE_LOGIN: &str =
    "The configuration would leave the console behind a password this program cannot type:";
const REJECTED_CONFIG: &str = "The device rejected these configuration lines:";
const MISSING_CONFIG: &str = "The running configuration is missing these lines from the file:";
const SHORT_UPTIME: &str = "The uptime does not reach the burn-in hours entered:";
const NO_PORT_TESTING: &str = "Port testing was not chosen for this run.";
const NO_PORT_WALK: &str = "Ports can be checked only after the configuration has passed validation, and not once the run has stopped.";
const SWITCH_PORT_DOWN: &str = "The port is not connected in show interface status.";
const ROUTER_PORT_DOWN: &str = "The interface or its line protocol is not up.";
const NO_PORTS: &str = "No physical ports were found to test.";
const NO_PORTS_ROW: &str = "No physical ports";
const NO_SERIAL: &str = "Not reported";

/// The files chosen for the run. `image_name` is the bare file name the
/// device copies over TFTP and installs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RunFiles {
    pub config_name: String,
    pub config_body: String,
    pub image_name: Option<String>,
}

pub fn device_kind(show_version: &str) -> LinkKind {
    let text = show_version.to_ascii_lowercase();
    if SWITCH_MARKERS.iter().any(|marker| text.contains(marker)) {
        LinkKind::Switch
    } else {
        LinkKind::Router
    }
}

/// The record row a stop marks failed.
#[derive(Debug, Clone, Copy)]
enum Check {
    Configuration,
    Firmware,
    BurnIn,
}

/// One bench run on one device.
#[derive(Debug)]
pub struct Runner<C: Console> {
    session: Session<C>,
    options: RunOptions,
    files: RunFiles,
    phase: Phase,
    record: RunRecord,
    reminder: Option<FactorySecrets>,
    kind: LinkKind,
    serial: Option<String>,
    tftp_port: Option<String>,
    ports: Vec<(String, CheckStatus)>,
    uptime_line: Option<String>,
    /// `show version` text captured when the firmware check passed.
    running_version: Option<String>,
    cable_confirmed: bool,
    wait_finished: bool,
    stop_reason: Option<String>,
}

impl<C: Console> Runner<C> {
    pub fn new(session: Session<C>, options: RunOptions, files: RunFiles) -> Self {
        let chosen = |yes: bool| {
            if yes {
                CheckStatus::NotFinished
            } else {
                CheckStatus::NotChosen
            }
        };
        let record = RunRecord {
            configuration: CheckStatus::NotFinished,
            firmware: chosen(options.firmware.is_some()),
            burn_in: chosen(options.burn_in_hours.is_some()),
            ports: chosen(options.port_testing),
            capwap: chosen(options.capwap.is_some()),
        };
        Self {
            session,
            phase: start_phase(&options),
            options,
            files,
            record,
            reminder: None,
            kind: LinkKind::Router,
            serial: None,
            tftp_port: None,
            ports: Vec::new(),
            uptime_line: None,
            running_version: None,
            cable_confirmed: false,
            wait_finished: false,
            stop_reason: None,
        }
    }

    pub fn phase(&self) -> Phase {
        self.phase
    }

    pub fn record(&self) -> &RunRecord {
        &self.record
    }

    /// The factory credentials the device accepted on the latest factory
    /// login, if it asked for any.
    pub fn reminder(&self) -> Option<&FactorySecrets> {
        self.reminder.as_ref()
    }

    /// The device interface the image cable goes into.
    pub fn tftp_port(&self) -> Option<&str> {
        self.tftp_port.as_deref()
    }

    pub fn stop_reason(&self) -> Option<&str> {
        self.stop_reason.as_deref()
    }

    pub fn session(&self) -> &Session<C> {
        &self.session
    }

    /// Every physical port in the walk and its latest result.
    pub fn ports(&self) -> &[(String, CheckStatus)] {
        &self.ports
    }

    /// Does the console work for the current phase and returns the new phase.
    /// Waits without console work at `NameTftpPort` until `confirm_cable`, at
    /// `WaitBurnIn` until `finish_wait`, and at `Done` and `Stopped`.
    pub fn advance(&mut self) -> Phase {
        let phase = self.phase;
        if let Err(err) = self.run_phase(phase) {
            let reason = err.to_string();
            let check = self.check_of(phase);
            self.stop(check, reason.clone(), reason);
        }
        self.phase
    }

    pub fn confirm_cable(&mut self) {
        self.cable_confirmed = true;
    }

    pub fn finish_wait(&mut self) {
        self.wait_finished = true;
    }

    /// Stops the run for a reason found off the console, failing the check
    /// the current phase belongs to. Does nothing once the run has ended.
    pub fn abort(&mut self, reason: &str) {
        if matches!(self.phase, Phase::Done | Phase::Stopped) {
            return;
        }
        let check = self.check_of(self.phase);
        self.stop(check, reason.to_string(), reason.to_string());
    }

    /// Checks one port of the walk and returns whether it passed. A retry
    /// replaces that port's earlier result.
    pub fn check_port(&mut self, name: &str) -> Result<bool, SessionError> {
        if !self.options.port_testing {
            return Err(SessionError::Stopped(NO_PORT_TESTING.into()));
        }
        if !port_walk_available(self.phase, self.options.family) {
            return Err(SessionError::Stopped(NO_PORT_WALK.into()));
        }
        let Some(index) = self.ports.iter().position(|(port, _)| port == name) else {
            return Err(SessionError::Stopped(format!(
                "{name} is not one of the ports to check."
            )));
        };
        let (passed, failure) = match self.kind {
            LinkKind::Switch => {
                let status = self.session.exec("show interface status")?;
                (switch_port_passed(&status, name), SWITCH_PORT_DOWN)
            }
            LinkKind::Router => {
                let interface = self.session.exec(&format!("show interfaces {name}"))?;
                (router_port_passed(&interface, name), ROUTER_PORT_DOWN)
            }
        };
        self.ports[index].1 = if passed {
            CheckStatus::Passed
        } else {
            CheckStatus::Failed {
                detail: failure.into(),
            }
        };
        self.record.ports = ports_status(&self.ports);
        Ok(passed)
    }

    pub fn export_input(&self) -> ExportInput {
        let running_version = self.running_version.clone();
        // The export prints no Ports section for an empty list.
        let ports = match &self.record.ports {
            CheckStatus::Failed { .. } if self.ports.is_empty() => {
                vec![(NO_PORTS_ROW.to_string(), self.record.ports.clone())]
            }
            _ => self.ports.clone(),
        };
        ExportInput {
            hostname: self.session.hostname().to_string(),
            serial: self.serial.clone().unwrap_or_else(|| NO_SERIAL.into()),
            config_file: self.files.config_name.clone(),
            configuration: self.record.configuration.clone(),
            firmware_file: self.files.image_name.clone(),
            running_version,
            firmware: self.record.firmware.clone(),
            burn_in_hours: self.options.burn_in_hours,
            uptime_line: self.uptime_line.clone(),
            burn_in: self.record.burn_in.clone(),
            ports,
            capwap: CheckStatus::NotChosen,
            capwap_ip: None,
            capwap_mask: None,
            capwap_gateway: None,
        }
    }

    fn run_phase(&mut self, phase: Phase) -> Result<(), SessionError> {
        match phase {
            Phase::Login => self.login()?,
            Phase::NameTftpPort => {
                if self.cable_confirmed {
                    self.send(PhaseEvent::CableConfirmed);
                }
            }
            Phase::CopyImage => self.copy_image()?,
            Phase::InstallAndReboot => {
                self.session.install(&self.image_name())?;
                self.send(PhaseEvent::Rebooted);
            }
            Phase::ConfirmVersion => self.confirm_version()?,
            Phase::ApplyCapwap | Phase::ValidateCapwap => {}
            Phase::ApplyConfig => self.apply_config()?,
            Phase::SaveAndReboot => {
                self.session.save()?;
                self.send(PhaseEvent::Saved);
                self.session.reload()?;
                self.send(PhaseEvent::Rebooted);
            }
            Phase::ValidateConfig => self.validate_config()?,
            Phase::CleanReboot => {
                let login = self.next_login();
                self.session.reload()?;
                self.session.login_with(&login)?;
                self.send(PhaseEvent::Rebooted);
            }
            Phase::WaitBurnIn => {
                if self.wait_finished {
                    self.send(PhaseEvent::WaitFinished);
                }
            }
            Phase::ReadUptime => self.read_uptime()?,
            Phase::Done | Phase::Stopped => {}
        }
        Ok(())
    }

    fn login(&mut self) -> Result<(), SessionError> {
        self.reminder = self.session.login_factory()?;
        let version = self.session.exec("show version")?;
        self.kind = device_kind(&version);
        let inventory = self.session.exec("show inventory")?;
        self.serial = chassis_serial(&inventory, &version);
        let names = interface_names(&self.session.exec("show ip interface brief")?);
        if self.options.port_testing {
            self.ports = physical_interfaces(&names)
                .into_iter()
                .map(|name| (name, CheckStatus::NotFinished))
                .collect();
            if self.ports.is_empty() {
                self.record.ports = CheckStatus::Failed {
                    detail: NO_PORTS.into(),
                };
            }
        }
        if self.options.firmware.is_some() {
            self.tftp_port = first_physical(&names);
            if self.tftp_port.is_none() {
                self.stop(Check::Firmware, NO_IMAGE_PORT.into(), NO_IMAGE_PORT.into());
                return Ok(());
            }
        }
        self.send(PhaseEvent::LoggedIn);
        Ok(())
    }

    fn copy_image(&mut self) -> Result<(), SessionError> {
        let image = self.image_name();
        if version_token(&image).is_none() {
            self.stop(Check::Firmware, NO_VERSION.into(), NO_VERSION.into());
            return Ok(());
        }
        let port = self.tftp_port.clone().unwrap_or_default();
        let rejected = self
            .session
            .configure(&assign_address_commands(self.kind, &port))?;
        if !rejected.is_empty() {
            let reason = listed(REJECTED_ADDRESS, &rejected);
            self.stop(Check::Firmware, rejected.join("\n"), reason);
            return Ok(());
        }
        self.session.copy_image(&copy_tftp_command(&image))?;
        self.send(PhaseEvent::ImageOnFlash);
        Ok(())
    }

    fn confirm_version(&mut self) -> Result<(), SessionError> {
        self.reminder = self.session.login_factory()?;
        let version = self.session.exec("show version")?;
        let image = self.image_name();
        if firmware_check(&image, &version) == FirmwareCheck::Pass {
            self.running_version = Some(version.trim().to_string());
            self.record.firmware = CheckStatus::Passed;
            self.send(PhaseEvent::VersionMatched);
        } else {
            let text = match version_token(&image) {
                Some(token) => {
                    format!("show version after the install does not report version {token}.")
                }
                None => NO_VERSION.into(),
            };
            self.fail(Check::Firmware, text.clone(), text);
            self.send(PhaseEvent::VersionRejected);
        }
        Ok(())
    }

    fn apply_config(&mut self) -> Result<(), SessionError> {
        if let NextLogin::Stop { lines } = self.next_login() {
            let text = listed(UNTYPABLE_LOGIN, &lines);
            self.stop(Check::Configuration, text.clone(), text);
            return Ok(());
        }
        let lines: Vec<String> = self.files.config_body.lines().map(String::from).collect();
        let rejected = self.session.configure(&lines)?;
        if !rejected.is_empty() {
            let reason = listed(REJECTED_CONFIG, &rejected);
            self.stop(Check::Configuration, rejected.join("\n"), reason);
            return Ok(());
        }
        self.send(PhaseEvent::ConfigApplied);
        Ok(())
    }

    fn validate_config(&mut self) -> Result<(), SessionError> {
        let login = self.next_login();
        self.session.login_with(&login)?;
        self.send(PhaseEvent::LoggedIn);
        let running = self.session.exec("show running-config")?;
        let missing = missing_commands(&self.files.config_body, &running);
        if missing.is_empty() {
            self.record.configuration = CheckStatus::Passed;
            self.send(PhaseEvent::ComparisonPassed);
        } else {
            let reason = listed(MISSING_CONFIG, &missing);
            self.fail(Check::Configuration, missing.join("\n"), reason);
            self.send(PhaseEvent::ComparisonFailed);
        }
        Ok(())
    }

    fn read_uptime(&mut self) -> Result<(), SessionError> {
        let output = self.session.exec("show version | include uptime")?;
        let lines: Vec<String> = output
            .lines()
            .map(str::trim)
            .filter(|line| !line.is_empty())
            .map(String::from)
            .collect();
        self.uptime_line = lines.first().cloned();
        let hours = self.options.burn_in_hours.unwrap_or_default();
        if burn_in_passes(hours, &parse_uptime_lines(&output)) {
            self.record.burn_in = CheckStatus::Passed;
            self.send(PhaseEvent::UptimePassed);
        } else {
            let reason = listed(SHORT_UPTIME, &lines);
            self.fail(Check::BurnIn, lines.join("\n"), reason);
            self.send(PhaseEvent::UptimeFailed);
        }
        Ok(())
    }

    /// How to log in once the file is saved. An open factory console has no
    /// secrets to fall back on.
    fn next_login(&self) -> NextLogin {
        let factory = self.reminder.clone().unwrap_or(FactorySecrets {
            username: String::new(),
            password: String::new(),
            enable_secret: String::new(),
        });
        login_after_apply(&self.files.config_body, &factory)
    }

    fn image_name(&self) -> String {
        self.files.image_name.clone().unwrap_or_default()
    }

    fn check_of(&self, phase: Phase) -> Check {
        match phase {
            Phase::NameTftpPort
            | Phase::CopyImage
            | Phase::InstallAndReboot
            | Phase::ConfirmVersion => Check::Firmware,
            Phase::WaitBurnIn | Phase::ReadUptime => Check::BurnIn,
            Phase::CleanReboot if self.options.burn_in_hours.is_some() => Check::BurnIn,
            _ => Check::Configuration,
        }
    }

    fn send(&mut self, event: PhaseEvent) {
        self.phase = next_phase(self.phase, &self.options, event);
    }

    /// Marks `check` failed with `detail` and keeps `reason` for the
    /// technician.
    fn fail(&mut self, check: Check, detail: String, reason: String) {
        let status = match check {
            Check::Configuration => &mut self.record.configuration,
            Check::Firmware => &mut self.record.firmware,
            Check::BurnIn => &mut self.record.burn_in,
        };
        *status = CheckStatus::Failed { detail };
        self.stop_reason = Some(reason);
    }

    fn stop(&mut self, check: Check, detail: String, reason: String) {
        self.fail(check, detail, reason);
        self.phase = Phase::Stopped;
    }
}

/// The first column of `show ip interface brief`, without its header.
fn interface_names(brief: &str) -> Vec<String> {
    brief
        .lines()
        .filter(|line| !line.starts_with("Interface"))
        .filter_map(|line| line.split_whitespace().next())
        .map(String::from)
        .collect()
}

/// Passed when there are ports and every one passed, failed when any port
/// failed, otherwise not finished.
fn ports_status(ports: &[(String, CheckStatus)]) -> CheckStatus {
    let failed: Vec<&str> = ports
        .iter()
        .filter(|(_, status)| matches!(status, CheckStatus::Failed { .. }))
        .map(|(name, _)| name.as_str())
        .collect();
    if !failed.is_empty() {
        CheckStatus::Failed {
            detail: failed.join("\n"),
        }
    } else if !ports.is_empty()
        && ports
            .iter()
            .all(|(_, status)| *status == CheckStatus::Passed)
    {
        CheckStatus::Passed
    } else {
        CheckStatus::NotFinished
    }
}

fn listed(heading: &str, lines: &[String]) -> String {
    format!("{heading}\n{}", lines.join("\n"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::export::export_text;
    use crate::run::FirmwareChoice;
    use crate::scripted::ScriptedConsole;

    const BODY: &str =
        "hostname closet-2-sw1\ninterface GigabitEthernet1/0/1\n switchport access vlan 20\n";
    const IMAGE: &str = "cat9k_iosxe.17.12.05.SPA.bin";
    const SWITCH_VERSION: &str =
        "Cisco IOS XE Software, Version 17.09.04\r\ncisco C9300-24T (X86) processor with 1336697K/6147K bytes of memory.";
    const ROUTER_VERSION: &str =
        "Cisco IOS XE Software, Version 17.09.04\r\ncisco ISR4331/K9 (1RU) processor with 1795999K/6147K bytes of memory.";
    const INVENTORY: &str =
        "NAME: \"c93xx Stack\", DESCR: \"c93xx Stack\"\r\nPID: C9300-24T         , VID: V02  , SN: FOC2345L0AB";
    const SWITCH_NAMES: [&str; 3] = ["Vlan1", "GigabitEthernet1/0/1", "GigabitEthernet1/0/2"];

    type Steps = Vec<(String, String)>;

    fn step(write: &str, output: &str) -> (String, String) {
        (write.to_string(), output.to_string())
    }

    /// A command the device echoes, answers, and ends with its prompt.
    fn exec(command: &str, output: &str, host: &str) -> (String, String) {
        step(command, &format!("{command}\r\n{output}\r\n{host}#"))
    }

    fn open_factory_login() -> Steps {
        vec![
            step(
                "",
                "\r\nWould you like to enter the initial configuration dialog? [yes/no]: ",
            ),
            step("no", "\r\nSwitch>"),
            step("enable", "\r\nSwitch#"),
            step("terminal length 0", "\r\nSwitch#"),
        ]
    }

    fn enable_secret_factory_login() -> Steps {
        vec![
            step(
                "",
                "\r\nWould you like to enter the initial configuration dialog? [yes/no]: ",
            ),
            step("no", "\r\nEnter enable secret: "),
            step("cisco", "\r\nConfirm enable secret: "),
            step("cisco", "\r\nSwitch>"),
            step("enable", "\r\nPassword: "),
            step("cisco", "\r\nSwitch#"),
            step("terminal length 0", "\r\nSwitch#"),
        ]
    }

    fn inspect(version: &str, inventory: &str, names: &[&str]) -> Steps {
        let mut brief = String::from(
            "Interface              IP-Address      OK? Method Status                Protocol",
        );
        for name in names {
            brief.push_str(&format!(
                "\r\n{name:<23}unassigned      YES unset  down                  down"
            ));
        }
        vec![
            exec("show version", version, "Switch"),
            exec("show inventory", inventory, "Switch"),
            exec("show ip interface brief", &brief, "Switch"),
        ]
    }

    fn apply_and_save() -> Steps {
        vec![
            step(
                "configure terminal",
                "configure terminal\r\nEnter configuration commands, one per line.  End with CNTL/Z.\r\nSwitch(config)#",
            ),
            step("hostname closet-2-sw1", "\r\ncloset-2-sw1(config)#"),
            step(
                "interface GigabitEthernet1/0/1",
                "\r\ncloset-2-sw1(config-if)#",
            ),
            step(" switchport access vlan 20", "\r\ncloset-2-sw1(config-if)#"),
            step("end", "\r\ncloset-2-sw1#"),
            step(
                "copy running-config startup-config",
                "\r\nDestination filename [startup-config]? ",
            ),
            step("", "\r\nBuilding configuration...\r\n[OK]\r\ncloset-2-sw1#"),
        ]
    }

    fn reload_and_login(enable_secret: Option<&str>) -> Steps {
        let mut steps = vec![
            step("reload", "\r\nProceed with reload? [confirm]"),
            step("", "\r\nReload requested\r\n"),
            step("", "\r\nPress RETURN to get started!\r\n"),
            step("", "\r\ncloset-2-sw1>"),
        ];
        match enable_secret {
            Some(secret) => steps.extend([
                step("enable", "\r\nPassword: "),
                step(secret, "\r\ncloset-2-sw1#"),
            ]),
            None => steps.push(step("enable", "\r\ncloset-2-sw1#")),
        }
        steps.push(step("terminal length 0", "\r\ncloset-2-sw1#"));
        steps
    }

    fn ok_running_config() -> &'static str {
        "Building configuration...\r\n\r\nCurrent configuration : 4821 bytes\r\n!\r\nhostname closet-2-sw1\r\n!\r\ninterface GigabitEthernet1/0/1\r\n switchport access vlan 20\r\n!\r\nend"
    }

    fn running_config_without_vlan_line() -> &'static str {
        "Building configuration...\r\n\r\nCurrent configuration : 4790 bytes\r\n!\r\nhostname closet-2-sw1\r\n!\r\ninterface GigabitEthernet1/0/1\r\n switchport mode access\r\n!\r\nend"
    }

    /// Apply, save, reload, validate, and the clean reboot.
    fn config_steps(running: &str, enable_secret: Option<&str>) -> Steps {
        [
            apply_and_save(),
            reload_and_login(enable_secret),
            vec![exec("show running-config", running, "closet-2-sw1")],
            reload_and_login(enable_secret),
        ]
        .concat()
    }

    fn login_and_inspect() -> Steps {
        [
            open_factory_login(),
            inspect(SWITCH_VERSION, INVENTORY, &SWITCH_NAMES),
        ]
        .concat()
    }

    fn address_and_copy(image: &str) -> Steps {
        vec![
            step("configure terminal", "\r\nSwitch(config)#"),
            step("interface GigabitEthernet1/0/1", "\r\nSwitch(config-if)#"),
            step("no switchport", "\r\nSwitch(config-if)#"),
            step(
                "ip address 192.0.2.2 255.255.255.252",
                "\r\nSwitch(config-if)#",
            ),
            step("no shutdown", "\r\nSwitch(config-if)#"),
            step("exit", "\r\nSwitch(config)#"),
            step(
                "ip tftp source-interface GigabitEthernet1/0/1",
                "\r\nSwitch(config)#",
            ),
            step("end", "\r\nSwitch#"),
            step(
                &copy_tftp_command(image),
                &format!("\r\nDestination filename [{image}]? "),
            ),
            step(
                "",
                &format!("\r\nAccessing tftp://192.0.2.1/{image}...\r\n!!!!!!!!\r\n[OK - 1234567 bytes]\r\n\r\n1234567 bytes copied in 812.345 secs (1520 bytes/sec)\r\nSwitch#"),
            ),
        ]
    }

    fn install_command(image: &str) -> String {
        format!("install add file flash:{image} activate commit")
    }

    /// A whole firmware run where `show version` after the install prints
    /// `version_after`.
    fn firmware_steps(image: &str, version_after: &str) -> Steps {
        [
            login_and_inspect(),
            address_and_copy(image),
            vec![
                step(
                    &install_command(image),
                    "\r\ninstall_add_activate_commit: START\r\nThis operation may require a reload of the system. Do you want to proceed? [y/n]",
                ),
                step(
                    "y",
                    "\r\nSUCCESS: install_add_activate_commit Wed Oct 28 09:20:00 UTC 2026\r\n",
                ),
            ],
            enable_secret_factory_login(),
            vec![exec("show version", version_after, "Switch")],
            config_steps(ok_running_config(), Some("cisco")),
        ]
        .concat()
    }

    fn options(image: Option<&str>, burn_in_hours: Option<u32>, port_testing: bool) -> RunOptions {
        RunOptions {
            serial_device: "/dev/cu.usbserial-1420".into(),
            config_path: "/bench/closet-access.cfg".into(),
            family: crate::DeviceFamily::IosXe,
            firmware: image.map(|image| FirmwareChoice {
                image_path: format!("/bench/{image}"),
                local_ethernet: "en5".into(),
            }),
            burn_in_hours,
            port_testing,
            capwap: None,
        }
    }

    fn runner(options: RunOptions, body: &str, steps: Steps) -> Runner<ScriptedConsole> {
        let files = RunFiles {
            config_name: "closet-access.cfg".into(),
            config_body: body.into(),
            image_name: options
                .firmware
                .as_ref()
                .map(|firmware| firmware.image_path.trim_start_matches("/bench/").into()),
        };
        let steps = steps
            .iter()
            .map(|(write, output)| (write.as_str(), output.as_str()))
            .collect();
        Runner::new(Session::new(ScriptedConsole::new(steps)), options, files)
    }

    fn config_only_runner(running: &str) -> Runner<ScriptedConsole> {
        let steps = [login_and_inspect(), config_steps(running, None)].concat();
        runner(options(None, None, false), BODY, steps)
    }

    fn runner_for_body(body: &str) -> Runner<ScriptedConsole> {
        runner(options(None, None, false), body, login_and_inspect())
    }

    fn firmware_runner(image: &str) -> Runner<ScriptedConsole> {
        let version_after =
            "Cisco IOS XE Software, Version 17.12.05\r\ncisco C9300-24T (X86) processor";
        runner(
            options(Some(image), None, false),
            BODY,
            firmware_steps(image, version_after),
        )
    }

    fn burn_in_runner(uptime: &str, port_testing: bool) -> Runner<ScriptedConsole> {
        let steps = [
            login_and_inspect(),
            config_steps(ok_running_config(), None),
            vec![exec(
                "show version | include uptime",
                uptime,
                "closet-2-sw1",
            )],
        ]
        .concat();
        runner(options(None, Some(4), port_testing), BODY, steps)
    }

    /// A port-testing run driven to `Done`, with `after` scripted for the
    /// port checks.
    fn done_runner(version: &str, names: &[&str], after: Steps) -> Runner<ScriptedConsole> {
        let steps = [
            open_factory_login(),
            inspect(version, INVENTORY, names),
            config_steps(ok_running_config(), None),
            after,
        ]
        .concat();
        let mut runner = runner(options(None, None, true), BODY, steps);
        assert_eq!(drive(&mut runner), Phase::Done);
        runner
    }

    fn done_switch_runner_with_ports(ports: &[&str]) -> Runner<ScriptedConsole> {
        let row = |port: &str, name: &str, status: &str, vlan: &str| {
            format!("{port:<22}{name:<19}{status:<13}{vlan}")
        };
        let mut table = row("Port", "Name", "Status", "Vlan");
        for port in ports {
            table.push_str("\r\n");
            table.push_str(&row(port, "", "connected", "1"));
        }
        let names = [&["Vlan1"], ports].concat();
        done_runner(
            SWITCH_VERSION,
            &names,
            vec![exec("show interface status", &table, "closet-2-sw1")],
        )
    }

    /// Advances until the run ends or waits on the technician, at most 40
    /// times.
    fn drive(runner: &mut Runner<ScriptedConsole>) -> Phase {
        for _ in 0..40 {
            let phase = runner.advance();
            if matches!(
                phase,
                Phase::Done | Phase::Stopped | Phase::NameTftpPort | Phase::WaitBurnIn
            ) {
                return phase;
            }
        }
        runner.phase()
    }

    fn reloads(runner: &Runner<ScriptedConsole>) -> usize {
        runner
            .session()
            .console()
            .written()
            .iter()
            .filter(|w| *w == "reload")
            .count()
    }

    fn writes(runner: &Runner<ScriptedConsole>) -> usize {
        runner.session().console().written().len()
    }

    #[test]
    fn config_only_run_passes_and_reloads_twice() {
        let mut runner = config_only_runner(ok_running_config());
        let phase = drive(&mut runner);
        assert_eq!(phase, Phase::Done);
        assert_eq!(runner.record().configuration, CheckStatus::Passed);
        assert_eq!(reloads(&runner), 2);
    }

    #[test]
    fn a_missing_command_stops_before_the_clean_reboot() {
        let mut runner = config_only_runner(running_config_without_vlan_line());
        assert_eq!(drive(&mut runner), Phase::Stopped);
        assert!(
            matches!(&runner.record().configuration, CheckStatus::Failed { detail } if detail.contains("switchport access vlan 20"))
        );
        assert_eq!(reloads(&runner), 1);
    }

    #[test]
    fn a_hashed_enable_secret_stops_before_anything_is_applied() {
        let mut runner = runner_for_body("enable secret 9 $9$abc\n");
        assert_eq!(drive(&mut runner), Phase::Stopped);
        let written = runner.session().console().written();
        assert!(!written
            .iter()
            .any(|w| w == "configure terminal" || w == "reload"));
    }

    #[test]
    fn an_image_without_a_version_stops_before_the_copy() {
        let mut runner = firmware_runner("image.bin");
        runner.advance();
        runner.confirm_cable();
        assert_eq!(drive(&mut runner), Phase::Stopped);
        assert!(!runner
            .session()
            .console()
            .written()
            .iter()
            .any(|w| w.starts_with("copy tftp")));
    }

    #[test]
    fn switch_port_check_uses_interface_status() {
        let mut runner = done_switch_runner_with_ports(&["GigabitEthernet1/0/1"]);
        assert!(runner.check_port("GigabitEthernet1/0/1").unwrap());
        assert_eq!(runner.record().ports, CheckStatus::Passed);
    }

    #[test]
    fn device_kind_reads_the_switch_markers_in_any_case() {
        assert_eq!(device_kind(SWITCH_VERSION), LinkKind::Switch);
        assert_eq!(
            device_kind("Cisco IE-3400-8T2S (ARM64) processor"),
            LinkKind::Switch
        );
        assert_eq!(
            device_kind("Switch Ports Model              SW Version"),
            LinkKind::Switch
        );
        assert_eq!(device_kind(ROUTER_VERSION), LinkKind::Router);
    }

    #[test]
    fn a_firmware_run_copies_before_installing_and_reboots_three_times() {
        let mut runner = firmware_runner(IMAGE);
        assert_eq!(drive(&mut runner), Phase::NameTftpPort);
        assert_eq!(runner.tftp_port(), Some("GigabitEthernet1/0/1"));
        assert_eq!(runner.reminder(), None);
        let before_cable = writes(&runner);
        assert_eq!(runner.advance(), Phase::NameTftpPort);
        assert_eq!(writes(&runner), before_cable);

        runner.confirm_cable();
        assert_eq!(drive(&mut runner), Phase::Done);
        assert_eq!(runner.record().firmware, CheckStatus::Passed);
        assert_eq!(runner.record().configuration, CheckStatus::Passed);
        let written = runner.session().console().written();
        let copy = written.iter().position(|w| w.starts_with("copy tftp"));
        let install = written.iter().position(|w| *w == install_command(IMAGE));
        assert!(copy.unwrap() < install.unwrap());
        let installs = written
            .iter()
            .filter(|w| w.starts_with("install add"))
            .count();
        assert_eq!(installs + reloads(&runner), 3);
        assert_eq!(
            runner.reminder(),
            Some(&FactorySecrets {
                username: String::new(),
                password: "cisco".into(),
                enable_secret: "cisco".into(),
            })
        );
        let input = runner.export_input();
        assert_eq!(input.firmware_file.as_deref(), Some(IMAGE));
        assert_eq!(
            input.running_version.as_deref(),
            Some("Cisco IOS XE Software, Version 17.12.05\r\ncisco C9300-24T (X86) processor")
        );
        assert_eq!(input.firmware, CheckStatus::Passed);
    }

    #[test]
    fn a_trailing_letter_does_not_pass_and_a_pass_quotes_show_version() {
        let short = "cat9k_iosxe.17.09.04.SPA.bin";
        let show = "Cisco IOS XE Software, Version 17.09.04a";
        let mut mismatched = runner(
            options(Some(short), None, false),
            BODY,
            firmware_steps(short, show),
        );
        drive(&mut mismatched);
        mismatched.confirm_cable();
        assert_eq!(drive(&mut mismatched), Phase::Stopped);
        assert!(matches!(
            mismatched.record().firmware,
            CheckStatus::Failed { .. }
        ));
        assert_eq!(mismatched.export_input().running_version, None);

        let image = "cat9k_iosxe.17.09.04a.SPA.bin";
        let mut passed = runner(
            options(Some(image), None, false),
            BODY,
            firmware_steps(image, show),
        );
        drive(&mut passed);
        passed.confirm_cable();
        assert_eq!(drive(&mut passed), Phase::Done);
        assert_eq!(passed.export_input().running_version.as_deref(), Some(show));
    }

    #[test]
    fn another_version_after_the_install_fails_the_firmware_before_the_apply() {
        let steps = firmware_steps(IMAGE, SWITCH_VERSION);
        let mut runner = runner(options(Some(IMAGE), None, false), BODY, steps);
        drive(&mut runner);
        runner.confirm_cable();
        assert_eq!(drive(&mut runner), Phase::Stopped);
        assert!(
            matches!(&runner.record().firmware, CheckStatus::Failed { detail } if detail.contains("17.12.05"))
        );
        assert_eq!(runner.record().configuration, CheckStatus::NotFinished);
        assert!(!runner
            .session()
            .console()
            .written()
            .iter()
            .any(|w| w == "hostname closet-2-sw1"));
        assert_eq!(runner.export_input().running_version, None);
    }

    #[test]
    fn firmware_without_a_physical_interface_stops_at_login() {
        let steps = [
            open_factory_login(),
            inspect(SWITCH_VERSION, INVENTORY, &["Vlan1", "Loopback0"]),
        ]
        .concat();
        let mut runner = runner(options(Some(IMAGE), None, false), BODY, steps);
        assert_eq!(runner.advance(), Phase::Stopped);
        let reason = "No physical interface was found for the image cable.";
        assert_eq!(runner.stop_reason(), Some(reason));
        assert_eq!(
            runner.record().firmware,
            CheckStatus::Failed {
                detail: reason.into()
            }
        );
        assert_eq!(runner.tftp_port(), None);
    }

    #[test]
    fn an_untypable_login_fails_the_configuration_with_its_lines() {
        let mut runner = runner_for_body("enable secret 9 $9$abc\n");
        assert_eq!(drive(&mut runner), Phase::Stopped);
        let text = "The configuration would leave the console behind a password this program cannot type:\nenable secret 9 $9$abc";
        assert_eq!(
            runner.record().configuration,
            CheckStatus::Failed {
                detail: text.into()
            }
        );
        assert_eq!(runner.stop_reason(), Some(text));
    }

    #[test]
    fn rejected_configuration_lines_fail_the_configuration_without_a_reload() {
        let steps = [
            login_and_inspect(),
            vec![
                step("configure terminal", "\r\nSwitch(config)#"),
                step("hostname closet-2-sw1", "\r\ncloset-2-sw1(config)#"),
                step(
                    "interface GigabitEthernet1/0/1",
                    "\r\ncloset-2-sw1(config-if)#",
                ),
                step(
                    " switchport access vlan 20",
                    "\r\n% Invalid input detected at '^' marker.\r\ncloset-2-sw1(config-if)#",
                ),
                step("end", "\r\ncloset-2-sw1#"),
            ],
        ]
        .concat();
        let mut runner = runner(options(None, None, false), BODY, steps);
        assert_eq!(drive(&mut runner), Phase::Stopped);
        assert_eq!(
            runner.record().configuration,
            CheckStatus::Failed {
                detail: " switchport access vlan 20".into()
            }
        );
        assert!(runner
            .stop_reason()
            .is_some_and(|reason| reason.contains(" switchport access vlan 20")));
        let written = runner.session().console().written();
        assert!(!written
            .iter()
            .any(|w| w == "reload" || w == "copy running-config startup-config"));
    }

    #[test]
    fn a_session_error_fails_the_check_its_phase_belongs_to() {
        let failed = "FAILED: install_add_activate_commit exit(1)";
        let steps = [
            login_and_inspect(),
            address_and_copy(IMAGE),
            vec![step(
                &install_command(IMAGE),
                &format!("\r\n{failed}\r\nSwitch#"),
            )],
        ]
        .concat();
        let mut runner = runner(options(Some(IMAGE), None, false), BODY, steps);
        drive(&mut runner);
        runner.confirm_cable();
        assert_eq!(drive(&mut runner), Phase::Stopped);
        let reason = format!("The image install failed: {failed}");
        assert_eq!(runner.stop_reason(), Some(reason.as_str()));
        assert_eq!(
            runner.record().firmware,
            CheckStatus::Failed { detail: reason }
        );
        assert_eq!(runner.record().configuration, CheckStatus::NotFinished);
    }

    #[test]
    fn a_clean_reboot_error_fails_the_burn_in_when_chosen_and_otherwise_the_configuration() {
        let through_validation = || {
            [
                login_and_inspect(),
                apply_and_save(),
                reload_and_login(None),
                vec![exec(
                    "show running-config",
                    ok_running_config(),
                    "closet-2-sw1",
                )],
            ]
            .concat()
        };
        let mut burn_in = runner(options(None, Some(4), false), BODY, through_validation());
        assert_eq!(drive(&mut burn_in), Phase::Stopped);
        let reason = burn_in.stop_reason().unwrap().to_string();
        assert!(reason.starts_with("Console error:") && reason.contains("reload"));
        assert_eq!(
            burn_in.record().burn_in,
            CheckStatus::Failed { detail: reason }
        );
        assert_eq!(burn_in.record().configuration, CheckStatus::Passed);

        let mut config_only = runner(options(None, None, false), BODY, through_validation());
        assert_eq!(drive(&mut config_only), Phase::Stopped);
        assert!(matches!(
            config_only.record().configuration,
            CheckStatus::Failed { .. }
        ));
    }

    #[test]
    fn a_burn_in_waits_for_finish_wait_then_passes_on_enough_uptime() {
        let uptime = "closet-2-sw1 uptime is 4 hours, 6 minutes";
        let mut runner = burn_in_runner(uptime, false);
        assert_eq!(drive(&mut runner), Phase::WaitBurnIn);
        assert_eq!(runner.record().burn_in, CheckStatus::NotFinished);
        let before_wait = writes(&runner);
        assert_eq!(runner.advance(), Phase::WaitBurnIn);
        assert_eq!(writes(&runner), before_wait);

        runner.finish_wait();
        assert_eq!(drive(&mut runner), Phase::Done);
        assert_eq!(runner.record().burn_in, CheckStatus::Passed);
        assert_eq!(runner.export_input().uptime_line.as_deref(), Some(uptime));
        assert_eq!(runner.stop_reason(), None);
    }

    #[test]
    fn too_little_uptime_fails_the_burn_in_and_stops() {
        let mut runner = burn_in_runner("closet-2-sw1 uptime is 3 hours, 58 minutes", false);
        assert_eq!(drive(&mut runner), Phase::WaitBurnIn);
        runner.finish_wait();
        assert_eq!(drive(&mut runner), Phase::Stopped);
        assert!(
            matches!(&runner.record().burn_in, CheckStatus::Failed { detail } if detail.contains("3 hours, 58 minutes"))
        );
        assert!(runner
            .stop_reason()
            .is_some_and(|reason| reason.contains("3 hours, 58 minutes")));
        assert_eq!(runner.record().configuration, CheckStatus::Passed);
    }

    #[test]
    fn the_export_during_the_burn_in_wait_says_the_ports_are_not_finished() {
        let mut runner = burn_in_runner("closet-2-sw1 uptime is 4 hours, 6 minutes", true);
        assert_eq!(drive(&mut runner), Phase::WaitBurnIn);
        let input = runner.export_input();
        assert_eq!(
            input,
            ExportInput {
                hostname: "closet-2-sw1".into(),
                serial: "FOC2345L0AB".into(),
                config_file: "closet-access.cfg".into(),
                configuration: CheckStatus::Passed,
                firmware_file: None,
                running_version: None,
                firmware: CheckStatus::NotChosen,
                burn_in_hours: Some(4),
                uptime_line: None,
                burn_in: CheckStatus::NotFinished,
                ports: vec![
                    ("GigabitEthernet1/0/1".into(), CheckStatus::NotFinished),
                    ("GigabitEthernet1/0/2".into(), CheckStatus::NotFinished),
                ],
                capwap: CheckStatus::NotChosen,
                capwap_ip: None,
                capwap_mask: None,
                capwap_gateway: None,
            }
        );
        let text = export_text(&input);
        assert!(text.contains("GigabitEthernet1/0/2: Not finished yet\n"));
        assert!(text.contains("Burn-in result: Not finished yet\n"));
    }

    #[test]
    fn the_export_says_not_reported_without_a_serial() {
        let steps = [
            open_factory_login(),
            inspect(ROUTER_VERSION, "", &["GigabitEthernet0/0/0"]),
        ]
        .concat();
        let mut runner = runner(options(None, None, false), BODY, steps);
        assert_eq!(runner.advance(), Phase::ApplyConfig);
        assert_eq!(runner.export_input().serial, "Not reported");
    }

    #[test]
    fn a_router_port_check_uses_show_interfaces_and_a_retry_replaces_the_result() {
        let (first, second) = ("GigabitEthernet0/0/0", "GigabitEthernet0/0/1");
        let answer = |name: &str, state: &str| {
            exec(
                &format!("show interfaces {name}"),
                &format!(
                    "{name} is {state}, line protocol is {state}\r\n  Hardware is ISR4331-3x1GE"
                ),
                "closet-2-sw1",
            )
        };
        let mut runner = done_runner(
            ROUTER_VERSION,
            &["Loopback0", first, second],
            vec![
                answer(first, "up"),
                answer(second, "down"),
                answer(second, "up"),
            ],
        );
        assert!(runner.check_port(first).unwrap());
        assert_eq!(runner.record().ports, CheckStatus::NotFinished);
        assert!(!runner.check_port(second).unwrap());
        assert!(
            matches!(&runner.record().ports, CheckStatus::Failed { detail } if detail.contains(second))
        );
        assert!(runner.check_port(second).unwrap());
        assert_eq!(runner.record().ports, CheckStatus::Passed);
        assert_eq!(
            runner.ports(),
            &[
                (first.to_string(), CheckStatus::Passed),
                (second.to_string(), CheckStatus::Passed),
            ]
        );
    }

    #[test]
    fn port_testing_without_physical_ports_fails_the_ports_and_the_run_continues() {
        let runner = done_runner(SWITCH_VERSION, &["Vlan1", "Loopback0"], vec![]);
        let failed = CheckStatus::Failed {
            detail: "No physical ports were found to test.".into(),
        };
        assert_eq!(runner.record().ports, failed);
        assert_eq!(runner.record().configuration, CheckStatus::Passed);
        assert!(!crate::run::ready_to_release(runner.record()));
        assert!(runner.ports().is_empty());
        let input = runner.export_input();
        assert_eq!(input.ports, vec![("No physical ports".to_string(), failed)]);
        assert!(export_text(&input).contains(
            "\nPorts\nNo physical ports: Failed\n    No physical ports were found to test.\n"
        ));
    }

    #[test]
    fn no_ports_is_not_finished_rather_than_passed() {
        assert_eq!(ports_status(&[]), CheckStatus::NotFinished);
    }

    #[test]
    fn abort_fails_the_phases_check_without_console_work_and_not_once_the_run_ended() {
        let mut runner = firmware_runner(IMAGE);
        assert_eq!(drive(&mut runner), Phase::NameTftpPort);
        runner.confirm_cable();
        assert_eq!(runner.advance(), Phase::CopyImage);
        let before = writes(&runner);
        let reason = "The TFTP server could not start on 192.0.2.1:69: Address already in use";
        runner.abort(reason);
        assert_eq!(runner.phase(), Phase::Stopped);
        assert_eq!(
            runner.record().firmware,
            CheckStatus::Failed {
                detail: reason.into()
            }
        );
        assert_eq!(runner.record().configuration, CheckStatus::NotFinished);
        assert_eq!(runner.stop_reason(), Some(reason));
        assert_eq!(writes(&runner), before);
        runner.abort("A later reason.");
        assert_eq!(runner.stop_reason(), Some(reason));

        let mut at_login = runner_for_body(BODY);
        at_login.abort("The serial device closed.");
        assert_eq!(
            at_login.record().configuration,
            CheckStatus::Failed {
                detail: "The serial device closed.".into()
            }
        );
        assert_eq!(writes(&at_login), 0);

        let mut done = config_only_runner(ok_running_config());
        assert_eq!(drive(&mut done), Phase::Done);
        done.abort("Too late.");
        assert_eq!(done.phase(), Phase::Done);
        assert_eq!(done.stop_reason(), None);
        assert_eq!(done.record().configuration, CheckStatus::Passed);
    }

    #[test]
    fn check_port_refuses_without_port_testing_before_validation_or_for_another_port() {
        let mut config_only = config_only_runner(ok_running_config());
        assert_eq!(drive(&mut config_only), Phase::Done);
        let done_writes = writes(&config_only);
        assert!(matches!(
            config_only.check_port("GigabitEthernet1/0/1"),
            Err(SessionError::Stopped(_))
        ));
        assert_eq!(writes(&config_only), done_writes);

        let mut early = runner(options(None, None, true), BODY, login_and_inspect());
        assert!(matches!(
            early.check_port("GigabitEthernet1/0/1"),
            Err(SessionError::Stopped(_))
        ));
        assert_eq!(writes(&early), 0);

        let mut done = done_switch_runner_with_ports(&["GigabitEthernet1/0/1"]);
        let done_writes = writes(&done);
        assert!(matches!(
            done.check_port("GigabitEthernet9/0/9"),
            Err(SessionError::Stopped(_))
        ));
        assert_eq!(writes(&done), done_writes);
    }
}
