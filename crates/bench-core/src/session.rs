//! Drives an IOS-XE device over its console: login, commands, configuration,
//! save, reload, image copy, and install.

use std::fmt;
use std::io;
use std::time::Duration;

use regex::Regex;

use crate::login::{FactoryLogin, FactorySecrets, LoginAction, LoginEvent, NextLogin};
use crate::transfer::save_startup_command;

/// The serial line to the device.
pub trait Console {
    fn write(&mut self, text: &str) -> io::Result<()>;
    /// Consumes unread output through the end of the match that ends first
    /// (ties go to the lower index) and returns that pattern's index and the
    /// consumed text. Fails with `ErrorKind::TimedOut` when nothing matches
    /// in time.
    fn read_until(&mut self, patterns: &[Regex], timeout: Duration) -> io::Result<(usize, String)>;
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SessionError {
    Io(String),
    Timeout { waiting_for: String },
    Stopped(String),
}

impl fmt::Display for SessionError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            SessionError::Io(message) => write!(f, "Console error: {message}"),
            SessionError::Timeout { waiting_for } => write!(
                f,
                "The device did not respond while waiting for {waiting_for}."
            ),
            SessionError::Stopped(message) => f.write_str(message),
        }
    }
}

/// The first login read also covers a reboot.
const BOOT_WAIT: Duration = Duration::from_secs(45 * 60);
const LOGIN_STEP_WAIT: Duration = Duration::from_secs(5 * 60);
const PROMPT_WAIT: Duration = Duration::from_secs(2 * 60);
const CONFIG_LINE_WAIT: Duration = Duration::from_secs(30);
const COPY_WAIT: Duration = Duration::from_secs(3 * 60 * 60);
const INSTALL_WAIT: Duration = Duration::from_secs(90 * 60);
/// Every loop that answers prompts gives up after this many reads.
const MAX_PROMPTS: usize = 40;

const PRIVILEGED: &str = r"(?m)^([A-Za-z0-9._-]+)#\s*$";
const CONFIG_PROMPT: &str = r"(?m)^[A-Za-z0-9._-]+\(config[^)]*\)#\s*$";
const USER_EXEC: &str = r"(?m)^[A-Za-z0-9._-]+>\s*$";
const INITIAL_DIALOG: &str = r"(?i)initial configuration dialog\? \[yes/no\]";
const AUTOINSTALL: &str = r"(?i)terminate autoinstall\? \[yes\]";
const PRESS_RETURN: &str = r"(?i)press return to get started";
const USERNAME: &str = r"(?i)(enter\b.*\busername|username)\s*:\s*$";
const CONFIRM_PASSWORD: &str = r"(?i)\b(confirm|re-enter)\b.*\b(password|secret)\b[^:\n]*:\s*$";
const ENABLE_SECRET: &str = r"(?i)\benter\b.*\benable secret\b[^:\n]*:\s*$";
const PASSWORD: &str = r"(?i)password\s*:\s*$";
const PASSWORD_REJECTED: &str = r"(?im)^%.*\b(password|secret)\b.*$";
const BAD_SECRETS: &str = r"(?im)^\s*%\s*bad secrets\b.*$";
const TOO_SHORT: &str = r"(?i)too short|minimum|at least \d+";
const LOGIN_REFUSED: &str =
    r"(?im)^\s*%\s*(login invalid|bad passwords|authentication failed|access denied).*$";
const CONFIG_REJECTION: &str = r"(?i)^\s*%\s*(invalid|incomplete|ambiguous|unknown|unrecognized|error|bad|cannot|not allowed|command rejected)";
const DESTINATION: &str = r"(?i)destination filename \[[^\]\n]*\]\?";
const OVERWRITE: &str = r"(?i)do you want to over write\? \[confirm\]";
/// Nexus copy when the bootflash file already exists. Parentheses, default `n`.
/// An empty line keeps the old file, so the copy answers `y`.
const NXOS_OVERWRITE: &str = r"(?i)\(y/n\)";
const SAVE_QUESTION: &str = r"(?i)save\? \[yes/no\]";
const CONFIRM: &str = r"(?i)\[confirm\]";
/// Nexus reload: `This command will reboot the system. (y/n)? [n]`.
/// Parentheses, not the IOS `[y/n]` / `[confirm]` forms. The default is `n`.
const NXOS_RELOAD: &str = r"(?i)\(y/n\)\?\s*\[n\]";
const PROCEED: &str = r"(?i)\[y/n\]";
const YES_NO: &str = r"(?i)\[yes/no\]";
const INSTALL_FAILED: &str = r"(?im)^.*(FAILED|%Error|Invalid input).*$";
const INSTALL_RELOADING: &str = r"(?i)\breloading\b|\brebooting\b|\breloaded\b|\breboot\b|\bwill reload\b|reload requested|initiating reload|requested system reload|SUCCESS:\s*install_add_activate_commit";

/// What a login read saw.
#[derive(Debug, Clone, Copy)]
enum Prompt {
    InitialDialog,
    Autoinstall,
    PressReturn,
    Rejected,
    Refused,
    ConfirmPassword,
    EnableSecret,
    Username,
    Password,
    Privileged,
    UserExec,
}

/// Earlier entries win when two prompts end at the same place, so
/// `Confirm password:` is not read as `Password:`.
const FACTORY_PROMPTS: [(Prompt, &str); 11] = [
    (Prompt::InitialDialog, INITIAL_DIALOG),
    (Prompt::Autoinstall, AUTOINSTALL),
    (Prompt::PressReturn, PRESS_RETURN),
    (Prompt::Rejected, PASSWORD_REJECTED),
    (Prompt::Rejected, BAD_SECRETS),
    (Prompt::ConfirmPassword, CONFIRM_PASSWORD),
    (Prompt::EnableSecret, ENABLE_SECRET),
    (Prompt::Username, USERNAME),
    (Prompt::Password, PASSWORD),
    (Prompt::Privileged, PRIVILEGED),
    (Prompt::UserExec, USER_EXEC),
];

const USER_PROMPTS: [(Prompt, &str); 8] = [
    (Prompt::InitialDialog, INITIAL_DIALOG),
    (Prompt::PressReturn, PRESS_RETURN),
    (Prompt::Refused, LOGIN_REFUSED),
    (Prompt::Refused, BAD_SECRETS),
    (Prompt::Username, USERNAME),
    (Prompt::Password, PASSWORD),
    (Prompt::Privileged, PRIVILEGED),
    (Prompt::UserExec, USER_EXEC),
];

const LOGIN_AWAITED: &str = "a login prompt or the device prompt";

/// A conversation with the device over its console.
#[derive(Debug)]
pub struct Session<C: Console> {
    console: C,
    transcript: String,
    hostname: String,
}

impl<C: Console> Session<C> {
    pub fn new(console: C) -> Self {
        Self {
            console,
            transcript: String::new(),
            hostname: String::new(),
        }
    }

    pub fn console(&self) -> &C {
        &self.console
    }

    /// Everything written to and read from the console, in order.
    pub fn transcript(&self) -> &str {
        &self.transcript
    }

    /// The name in the most recent privileged prompt.
    pub fn hostname(&self) -> &str {
        &self.hostname
    }

    /// Logs into a device without a saved configuration, declining setup and
    /// creating whatever account and enable secret it asks for. Returns the
    /// credentials the technician must remember, if any were created.
    pub fn login_factory(&mut self) -> Result<Option<FactorySecrets>, SessionError> {
        let patterns = compile(FACTORY_PROMPTS.map(|(_, source)| source));
        let too_short = regex(TOO_SHORT);
        let mut login = FactoryLogin::new();
        let mut wait = BOOT_WAIT;
        self.send("")?;
        for _ in 0..MAX_PROMPTS {
            let (index, text) = self.read(&patterns, wait, LOGIN_AWAITED)?;
            wait = LOGIN_STEP_WAIT;
            let event = match FACTORY_PROMPTS[index].0 {
                Prompt::Autoinstall => {
                    self.send("yes")?;
                    continue;
                }
                Prompt::PressReturn => {
                    self.send("")?;
                    continue;
                }
                Prompt::InitialDialog => LoginEvent::InitialDialog,
                Prompt::Rejected => {
                    let raw = last_line(&text).to_string();
                    LoginEvent::Rejected {
                        too_short: too_short.is_match(&raw),
                        raw,
                    }
                }
                Prompt::ConfirmPassword => LoginEvent::ConfirmPassword,
                Prompt::EnableSecret => LoginEvent::EnableSecretPrompt,
                Prompt::Username => LoginEvent::UsernamePrompt,
                Prompt::Password => LoginEvent::PasswordPrompt,
                Prompt::UserExec => LoginEvent::UserExec,
                Prompt::Privileged => LoginEvent::Privileged,
                Prompt::Refused => unreachable!("not a factory login prompt"),
            };
            match login.on_event(event) {
                LoginAction::Send(answer) => self.send(answer)?,
                LoginAction::Wait => {}
                LoginAction::Stop { reason } => return Err(SessionError::Stopped(reason)),
                LoginAction::InPrivileged { reminder } => {
                    self.exec("terminal length 0")?;
                    return Ok(reminder);
                }
            }
        }
        Err(too_many_prompts("the privileged prompt"))
    }

    /// Logs in once the technician's file has been saved and the device
    /// rebooted with it.
    pub fn login_with(&mut self, login: &NextLogin) -> Result<(), SessionError> {
        let (username, password, enable_secret) = match login {
            NextLogin::Stop { lines } => {
                return Err(SessionError::Stopped(format!(
                    "The console login after the reboot cannot be typed:\n{}",
                    lines.join("\n")
                )))
            }
            NextLogin::Factory(secrets) => {
                (&secrets.username, &secrets.password, &secrets.enable_secret)
            }
            NextLogin::User {
                username,
                password,
                enable_secret,
            } => (username, password, enable_secret),
        };
        let patterns = compile(USER_PROMPTS.map(|(_, source)| source));
        let mut sent_enable = false;
        let mut wait = BOOT_WAIT;
        self.send("")?;
        for _ in 0..MAX_PROMPTS {
            let (index, text) = self.read(&patterns, wait, LOGIN_AWAITED)?;
            wait = LOGIN_STEP_WAIT;
            match USER_PROMPTS[index].0 {
                Prompt::InitialDialog => {
                    return Err(SessionError::Stopped(
                        "The device booted without the saved configuration.".into(),
                    ))
                }
                Prompt::Refused => {
                    return Err(SessionError::Stopped(format!(
                        "The device refused the console login: {}",
                        last_line(&text)
                    )))
                }
                Prompt::PressReturn => self.send("")?,
                Prompt::Username => self.send(username)?,
                Prompt::Password if sent_enable => self.send(enable_secret)?,
                Prompt::Password => self.send(password)?,
                Prompt::UserExec => {
                    self.send("enable")?;
                    sent_enable = true;
                }
                Prompt::Privileged => {
                    self.exec("terminal length 0")?;
                    return Ok(());
                }
                _ => unreachable!("not a post-reboot login prompt"),
            }
        }
        Err(too_many_prompts("the privileged prompt"))
    }

    /// Runs `command` and returns its output without the echoed command line
    /// and without the prompt.
    pub fn exec(&mut self, command: &str) -> Result<String, SessionError> {
        let prompt = compile([PRIVILEGED]);
        self.send(command)?;
        let waiting_for = format!("the device prompt after `{command}`");
        let (_, text) = self.read(&prompt, PROMPT_WAIT, &waiting_for)?;
        let end = prompt[0]
            .find_iter(&text)
            .last()
            .map_or(text.len(), |found| found.start());
        let output = text[..end].split_once('\n').map_or("", |(_, rest)| rest);
        Ok(output.to_string())
    }

    /// Applies `lines` in configuration mode and returns the lines the device
    /// rejected.
    pub fn configure(&mut self, lines: &[String]) -> Result<Vec<String>, SessionError> {
        let prompts = compile([CONFIG_PROMPT, PRIVILEGED]);
        let rejection = regex(CONFIG_REJECTION);
        self.send("configure terminal")?;
        self.read(&prompts[..1], CONFIG_LINE_WAIT, "the configuration prompt")?;
        let mut rejected = Vec::new();
        // The device shows no prompt while it takes banner text.
        let mut open_banner: Option<(&String, &str)> = None;
        for line in lines {
            let command = match open_banner {
                Some((opening, delimiter)) => {
                    self.send(line)?;
                    if !line.contains(delimiter) {
                        continue;
                    }
                    open_banner = None;
                    opening
                }
                None if is_navigation(line) => continue,
                None => {
                    self.send(line)?;
                    match banner_start(line) {
                        Some((delimiter, text)) if !text.contains(delimiter) => {
                            open_banner = Some((line, delimiter));
                            continue;
                        }
                        _ => line,
                    }
                }
            };
            let waiting_for = format!("the configuration prompt after `{command}`");
            let (_, output) = self.read(&prompts, CONFIG_LINE_WAIT, &waiting_for)?;
            if output.lines().any(|out| rejection.is_match(out)) {
                rejected.push(command.clone());
            }
        }
        self.send("end")?;
        self.read(
            &prompts[1..],
            CONFIG_LINE_WAIT,
            "the device prompt after `end`",
        )?;
        Ok(rejected)
    }

    pub fn save(&mut self) -> Result<(), SessionError> {
        self.save_startup(save_startup_command())
    }

    /// Saves with `command` (the family's startup-config copy) and returns
    /// once the device reports `[OK]` or `Copy complete`.
    ///
    /// IOS asks for a destination filename. NX-OS does not: it prints a
    /// progress bar and `Copy complete.`
    pub fn save_startup(&mut self, command: &str) -> Result<(), SessionError> {
        self.send(command)?;
        let (_, output) = self.answer_until(
            &[PRIVILEGED],
            &[(DESTINATION, "")],
            PROMPT_WAIT,
            "the device prompt after saving",
        )?;
        if let Some(line) = line_with(&output, &["%Error"]) {
            return Err(SessionError::Stopped(format!(
                "Saving the configuration failed: {line}\n{output}"
            )));
        }
        if !save_succeeded(&output) {
            return Err(SessionError::Stopped(format!(
                "Saving the configuration did not report [OK] or Copy complete.\n{output}"
            )));
        }
        Ok(())
    }

    /// Reloads without saving and returns once the reload is confirmed. The
    /// caller logs in again.
    ///
    /// IOS `[confirm]` takes an empty line. NX-OS asks `(y/n)? [n]`; an empty
    /// line accepts the default `n` and cancels the reload, so the answer is `y`.
    pub fn reload(&mut self) -> Result<(), SessionError> {
        self.send("reload")?;
        let (index, _) = self.answer_until(
            &[CONFIRM, NXOS_RELOAD],
            &[(SAVE_QUESTION, "no")],
            PROMPT_WAIT,
            "the reload confirmation",
        )?;
        let answer = if index == 0 { "" } else { "y" };
        self.send(answer)
    }

    /// Copies an image. IOS asks for a destination filename and
    /// `Do you want to over write? [confirm]`; both take an empty line.
    /// NX-OS asks `(y/n)` with default `n` when that bootflash name already
    /// exists. An empty line keeps the old file, so the answer is `y`.
    pub fn copy_image(&mut self, command: &str) -> Result<(), SessionError> {
        self.send(command)?;
        let (_, output) = self.answer_until(
            &[PRIVILEGED],
            &[(DESTINATION, ""), (OVERWRITE, ""), (NXOS_OVERWRITE, "y")],
            COPY_WAIT,
            "the device prompt after the image copy",
        )?;
        if let Some(line) = line_with(&output, &["%Error", "Error opening"]) {
            return Err(SessionError::Stopped(format!(
                "The image copy failed: {line}\n{output}"
            )));
        }
        if !copy_succeeded(&output) {
            return Err(SessionError::Stopped(format!(
                "The image copy did not report bytes copied.\n{output}"
            )));
        }
        Ok(())
    }

    /// Installs an IOS-XE image from flash and returns once the device starts
    /// its reload. The command is `install add file flash:{image} activate commit`.
    pub fn install(&mut self, image: &str) -> Result<(), SessionError> {
        self.run_install(&format!("install add file flash:{image} activate commit"))
    }

    /// Sends one install command and returns once the device starts its reload.
    ///
    /// `[yes/no]` is declined (the IOS-XE save question). `[y/n]` is accepted.
    /// `[confirm]` is accepted with an empty line. A reload is `reloading`,
    /// `reloaded`, `reboot`, `will reload` (the access-point
    /// `archive download-sw` line, as in "device will reload"), or the IOS-XE
    /// `SUCCESS: install_add_activate_commit` line. `reload` alone is not
    /// enough: the IOS-XE proceed question says "may require a reload" before
    /// the technician has answered, and that sentence does not contain
    /// `will reload`.
    pub fn run_install(&mut self, command: &str) -> Result<(), SessionError> {
        self.send(command)?;
        let (index, output) = self.answer_until(
            &[INSTALL_FAILED, INSTALL_RELOADING],
            &[(PROCEED, "y"), (YES_NO, "no"), (CONFIRM, "")],
            INSTALL_WAIT,
            "the install to reload the device",
        )?;
        if index == 0 {
            return Err(install_failed(&output));
        }
        Ok(())
    }

    fn send(&mut self, text: &str) -> Result<(), SessionError> {
        let line = format!("{text}\r");
        self.console
            .write(&line)
            .map_err(|err| session_error(err, "the console to take input"))?;
        self.transcript.push_str(&line);
        Ok(())
    }

    fn read(
        &mut self,
        patterns: &[Regex],
        wait: Duration,
        waiting_for: &str,
    ) -> Result<(usize, String), SessionError> {
        let (index, text) = self
            .console
            .read_until(patterns, wait)
            .map_err(|err| session_error(err, waiting_for))?;
        self.transcript.push_str(&text);
        let pattern = &patterns[index];
        if pattern.as_str() == PRIVILEGED {
            if let Some(found) = pattern.captures_iter(&text).last() {
                self.hostname = found[1].to_string();
            }
        }
        Ok((index, text))
    }

    /// Answers each `answers` prompt until a `finish` pattern matches, and
    /// returns that pattern's index and everything read.
    fn answer_until(
        &mut self,
        finish: &[&str],
        answers: &[(&str, &str)],
        wait: Duration,
        waiting_for: &str,
    ) -> Result<(usize, String), SessionError> {
        let sources = finish
            .iter()
            .chain(answers.iter().map(|(source, _)| source));
        let patterns = compile(sources.copied());
        let mut output = String::new();
        for _ in 0..MAX_PROMPTS {
            let (index, text) = self.read(&patterns, wait, waiting_for)?;
            output.push_str(&text);
            match index.checked_sub(finish.len()) {
                None => return Ok((index, output)),
                Some(answer) => self.send(answers[answer].1)?,
            }
        }
        Err(too_many_prompts(waiting_for))
    }
}

fn regex(source: &str) -> Regex {
    Regex::new(source).expect("session patterns are valid")
}

fn compile<'a>(sources: impl IntoIterator<Item = &'a str>) -> Vec<Regex> {
    sources.into_iter().map(regex).collect()
}

fn session_error(err: io::Error, waiting_for: &str) -> SessionError {
    if err.kind() == io::ErrorKind::TimedOut {
        SessionError::Timeout {
            waiting_for: waiting_for.to_string(),
        }
    } else {
        SessionError::Io(err.to_string())
    }
}

fn too_many_prompts(goal: &str) -> SessionError {
    SessionError::Stopped(format!(
        "The console showed {MAX_PROMPTS} prompts without reaching {goal}."
    ))
}

fn install_failed(output: &str) -> SessionError {
    SessionError::Stopped(format!("The image install failed: {}", last_line(output)))
}

/// IOS prints `[OK]`. NX-OS prints a progress bar and `Copy complete.`
fn save_succeeded(output: &str) -> bool {
    output.contains("[OK]") || output.contains("Copy complete")
}

/// IOS prints `bytes copied`. NX-OS prints `TFTP get operation was successful`
/// and `Copy complete, now saving to disk`.
fn copy_succeeded(output: &str) -> bool {
    output.contains("bytes copied")
        || output.contains("TFTP get operation was successful")
        || output.contains("Copy complete, now saving to disk")
}

/// The last line of text that was read through the end of a line pattern.
fn last_line(text: &str) -> &str {
    text.rsplit('\n').next().unwrap_or_default().trim()
}

fn line_with<'a>(text: &'a str, needles: &[&str]) -> Option<&'a str> {
    text.lines()
        .map(str::trim)
        .find(|line| needles.iter().any(|needle| line.contains(needle)))
}

fn is_navigation(line: &str) -> bool {
    let line = line.trim();
    ["configure terminal", "conf t", "end"]
        .iter()
        .any(|navigation| line.eq_ignore_ascii_case(navigation))
}

/// For `banner <type> <delimiter>...`, returns the delimiter and the text
/// after it. A `^C` delimiter is the two characters `^` and `C`.
fn banner_start(line: &str) -> Option<(&str, &str)> {
    let (word, rest) = line.trim_start().split_once(char::is_whitespace)?;
    if !word.eq_ignore_ascii_case("banner") {
        return None;
    }
    let (_kind, rest) = rest.trim_start().split_once(char::is_whitespace)?;
    let text = rest.trim_start();
    let width = if text.starts_with("^C") {
        2
    } else {
        text.chars().next()?.len_utf8()
    };
    Some(text.split_at(width))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scripted::ScriptedConsole;

    #[test]
    fn open_console_declines_setup_and_enters_enable() {
        let console = ScriptedConsole::new(vec![
            (
                "",
                "Would you like to enter the initial configuration dialog? [yes/no]: ",
            ),
            ("no", "\r\nSwitch>"),
            ("enable", "\r\nSwitch#"),
            ("terminal length 0", "\r\nSwitch#"),
        ]);
        let mut session = Session::new(console);
        assert_eq!(session.login_factory().unwrap(), None);
        assert_eq!(session.hostname(), "Switch");
    }

    #[test]
    fn factory_login_keeps_an_accepted_password_when_only_enable_changes() {
        let mut session = session(vec![
            ("", "\r\nUsername: "),
            ("cisco", "\r\nPassword: "),
            ("cisco", "\r\nConfirm password: "),
            ("cisco", "\r\nEnter enable secret: "),
            (
                "cisco",
                "\r\n% Password rejected. Too short.\r\nEnter enable secret: ",
            ),
            ("Passw0rd%", "\r\nConfirm enable secret: "),
            ("Passw0rd%", "\r\nSwitch>"),
            ("enable", "\r\nPassword: "),
            ("Passw0rd%", "\r\nSwitch#"),
            ("terminal length 0", "\r\nSwitch#"),
        ]);
        let secrets = session.login_factory().unwrap().unwrap();
        assert_eq!(secrets.username, "cisco");
        assert_eq!(secrets.password, "cisco");
        assert_eq!(secrets.enable_secret, "Passw0rd%");
    }

    #[test]
    fn short_password_waits_for_the_prompt_then_sends_the_fallback() {
        let console = ScriptedConsole::new(vec![
            ("", "Enter enable secret: "),
            (
                "cisco",
                "\r\n% Password rejected. Too short.\r\nEnter enable secret: ",
            ),
            ("Passw0rd%", "\r\nConfirm enable secret: "),
            ("Passw0rd%", "\r\nSwitch>"),
            ("enable", "\r\nPassword: "),
            ("Passw0rd%", "\r\nSwitch#"),
            ("terminal length 0", "\r\nSwitch#"),
        ]);
        let mut session = Session::new(console);
        let secrets = session.login_factory().unwrap().unwrap();
        assert_eq!(secrets.enable_secret, "Passw0rd%");
    }

    #[test]
    fn configure_returns_the_rejected_lines() {
        let console = ScriptedConsole::new(vec![
            ("configure terminal", "\r\nSwitch(config)#"),
            ("interface GigabitEthernet1/0/1", "\r\nSwitch(config-if)#"),
            (
                " bogus",
                "\r\n% Invalid input detected at '^' marker.\r\nSwitch(config-if)#",
            ),
            ("end", "\r\nSwitch#"),
        ]);
        let mut session = Session::new(console);
        let lines = vec![
            "configure terminal".to_string(),
            "interface GigabitEthernet1/0/1".to_string(),
            " bogus".to_string(),
            "end".to_string(),
        ];
        assert_eq!(
            session.configure(&lines).unwrap(),
            vec![" bogus".to_string()]
        );
    }

    #[test]
    fn reload_answers_no_to_save_and_confirms() {
        let console = ScriptedConsole::new(vec![
            (
                "reload",
                "\r\nSystem configuration has been modified. Save? [yes/no]: ",
            ),
            ("no", "\r\nProceed with reload? [confirm]"),
            ("", "\r\nReload requested\r\n"),
        ]);
        let mut session = Session::new(console);
        session.reload().unwrap();
        assert_eq!(
            session.console().written(),
            &["reload".to_string(), "no".to_string(), "".to_string()]
        );
    }

    #[test]
    fn reload_answers_y_to_the_nxos_reboot_question() {
        let mut session = session(vec![
            (
                "reload",
                "\r\nThis command will reboot the system. (y/n)? [n] ",
            ),
            ("y", "\r\n"),
        ]);
        session.reload().unwrap();
        assert_eq!(written(&session), ["reload", "y"]);
    }

    #[test]
    fn login_with_a_file_user_after_reboot() {
        let console = ScriptedConsole::new(vec![
            ("", "\r\nPress RETURN to get started!\r\n"),
            ("", "\r\nUser Access Verification\r\n\r\nUsername: "),
            ("bench", "Password: "),
            ("BenchPass", "\r\ncloset-2-sw1#"),
            ("terminal length 0", "\r\ncloset-2-sw1#"),
        ]);
        let mut session = Session::new(console);
        let login = NextLogin::User {
            username: "bench".into(),
            password: "BenchPass".into(),
            enable_secret: "cisco".into(),
        };
        session.login_with(&login).unwrap();
        assert_eq!(session.hostname(), "closet-2-sw1");
    }

    fn session(steps: Vec<(&str, &str)>) -> Session<ScriptedConsole> {
        Session::new(ScriptedConsole::new(steps))
    }

    fn written(session: &Session<ScriptedConsole>) -> Vec<&str> {
        session
            .console()
            .written()
            .iter()
            .map(String::as_str)
            .collect()
    }

    fn strings(lines: &[&str]) -> Vec<String> {
        lines.iter().map(|line| line.to_string()).collect()
    }

    fn user(username: &str, password: &str, enable_secret: &str) -> NextLogin {
        NextLogin::User {
            username: username.into(),
            password: password.into(),
            enable_secret: enable_secret.into(),
        }
    }

    #[test]
    fn factory_login_terminates_autoinstall_and_presses_return() {
        let mut session = session(vec![
            (
                "",
                "Would you like to enter the initial configuration dialog? [yes/no]: ",
            ),
            ("no", "\r\nWould you like to terminate autoinstall? [yes]: "),
            ("yes", "\r\n\r\nPress RETURN to get started!\r\n"),
            ("", "\r\nSwitch>"),
            ("enable", "\r\nSwitch#"),
            ("terminal length 0", "\r\nSwitch#"),
        ]);
        assert_eq!(session.login_factory().unwrap(), None);
    }

    #[test]
    fn login_stops_after_forty_prompts() {
        let mut session = session(vec![("", "Press RETURN to get started!\r\n"); 41]);
        assert!(matches!(
            session.login_factory(),
            Err(SessionError::Stopped(_))
        ));
        assert_eq!(session.console().written().len(), 41);
    }

    #[test]
    fn login_with_stops_on_the_initial_dialog() {
        let mut session = session(vec![(
            "",
            "\r\nWould you like to enter the initial configuration dialog? [yes/no]: ",
        )]);
        let login = NextLogin::Factory(FactorySecrets {
            username: "cisco".into(),
            password: "cisco".into(),
            enable_secret: "cisco".into(),
        });
        assert_eq!(
            session.login_with(&login),
            Err(SessionError::Stopped(
                "The device booted without the saved configuration.".into()
            ))
        );
        assert_eq!(written(&session), [""]);
    }

    #[test]
    fn login_with_answers_the_enable_password_with_the_enable_secret() {
        let mut session = session(vec![
            ("", "\r\nUser Access Verification\r\n\r\nPassword: "),
            ("LinePass", "\r\nSwitch>"),
            ("enable", "\r\nPassword: "),
            ("EnPass", "\r\nSwitch#"),
            ("terminal length 0", "\r\nSwitch#"),
        ]);
        session
            .login_with(&user("cisco", "LinePass", "EnPass"))
            .unwrap();
        assert_eq!(session.hostname(), "Switch");
    }

    #[test]
    fn login_with_stops_when_the_device_refuses_the_login() {
        for refusal in [
            "% Login invalid",
            "% Bad passwords",
            "% Authentication failed",
            "% ACCESS DENIED",
        ] {
            let output = format!("\r\n{refusal}\r\n\r\nUsername: ");
            let mut session = session(vec![
                ("", "\r\nUsername: "),
                ("bench", "Password: "),
                ("BenchPass", output.as_str()),
            ]);
            match session.login_with(&user("bench", "BenchPass", "cisco")) {
                Err(SessionError::Stopped(reason)) => assert!(reason.contains(refusal)),
                other => panic!("unexpected {other:?}"),
            }
        }
    }

    #[test]
    fn login_with_stops_at_once_on_bad_secrets() {
        for refusal in ["% Bad secrets", "% BAD SECRETS"] {
            let output = format!("\r\n{refusal}\r\n\r\nSwitch>");
            let mut session = session(vec![
                ("", "\r\nUser Access Verification\r\n\r\nPassword: "),
                ("LinePass", "\r\nSwitch>"),
                ("enable", "\r\nPassword: "),
                ("Wrong", output.as_str()),
            ]);
            match session.login_with(&user("cisco", "LinePass", "Wrong")) {
                Err(SessionError::Stopped(reason)) => assert!(reason.contains(refusal)),
                other => panic!("unexpected {other:?}"),
            }
            assert_eq!(written(&session), ["", "LinePass", "enable", "Wrong"]);
        }
    }

    #[test]
    fn factory_login_stops_on_bad_secrets() {
        let mut session = session(vec![
            ("", "\r\nSwitch>"),
            ("enable", "\r\nPassword: "),
            ("cisco", "\r\n% Bad secrets\r\n\r\nSwitch>"),
        ]);
        match session.login_factory() {
            Err(SessionError::Stopped(reason)) => assert!(reason.contains("% Bad secrets")),
            other => panic!("unexpected {other:?}"),
        }
        assert_eq!(written(&session), ["", "enable", "cisco"]);
    }

    #[test]
    fn login_with_a_stop_writes_nothing() {
        let mut session = session(vec![]);
        let login = NextLogin::Stop {
            lines: vec!["enable secret 9 $9$abc".into()],
        };
        match session.login_with(&login) {
            Err(SessionError::Stopped(reason)) => {
                assert!(reason.contains("enable secret 9 $9$abc"))
            }
            other => panic!("unexpected {other:?}"),
        }
        assert!(session.console().written().is_empty());
    }

    #[test]
    fn exec_drops_the_echo_and_the_prompt_and_the_transcript_keeps_both() {
        let mut session = session(vec![(
            "show clock",
            "show clock\r\n*10:00:00.000 UTC Wed Oct 28 2026\r\nSwitch#",
        )]);
        assert_eq!(
            session.exec("show clock").unwrap(),
            "*10:00:00.000 UTC Wed Oct 28 2026\r\n"
        );
        assert_eq!(
            session.transcript(),
            "show clock\rshow clock\r\n*10:00:00.000 UTC Wed Oct 28 2026\r\nSwitch#"
        );
        assert_eq!(session.hostname(), "Switch");
    }

    #[test]
    fn a_missing_prompt_is_a_timeout_naming_what_was_awaited() {
        let mut session = session(vec![("show version", "\r\nCisco IOS XE Software\r\n")]);
        match session.exec("show version") {
            Err(SessionError::Timeout { waiting_for }) => {
                assert!(waiting_for.contains("show version"))
            }
            other => panic!("unexpected {other:?}"),
        }
    }

    #[test]
    fn session_errors_read_as_plain_text() {
        assert_eq!(
            SessionError::Io("the serial port closed".into()).to_string(),
            "Console error: the serial port closed"
        );
        assert_eq!(
            SessionError::Timeout {
                waiting_for: "the reload confirmation".into()
            }
            .to_string(),
            "The device did not respond while waiting for the reload confirmation."
        );
        assert_eq!(
            SessionError::Stopped("The image copy did not report bytes copied.".into()).to_string(),
            "The image copy did not report bytes copied."
        );
    }

    #[test]
    fn a_write_off_script_is_an_io_error() {
        let mut session = session(vec![("show version", "\r\nSwitch#")]);
        match session.exec("show clock") {
            Err(SessionError::Io(message)) => {
                assert!(message.contains("show version") && message.contains("show clock"))
            }
            other => panic!("unexpected {other:?}"),
        }
    }

    #[test]
    fn command_rejected_is_a_configure_rejection() {
        let line = "switchport mode access";
        let mut session = session(vec![
            ("configure terminal", "\r\nSwitch(config)#"),
            (
                line,
                "\r\n% Command rejected: this command is not supported.\r\nSwitch(config)#",
            ),
            ("end", "\r\nSwitch#"),
        ]);
        assert_eq!(
            session.configure(&strings(&[line])).unwrap(),
            strings(&[line])
        );
    }

    #[test]
    fn a_warning_is_not_a_rejection() {
        let line = "username bench privilege 15 password BenchPass";
        let mut session = session(vec![
            ("configure terminal", "\r\nSwitch(config)#"),
            (
                line,
                "\r\n% Warning: the password is stored in clear text.\r\nSwitch(config)#",
            ),
            ("end", "\r\nSwitch#"),
        ]);
        assert!(session.configure(&strings(&[line])).unwrap().is_empty());
    }

    #[test]
    fn the_host_name_follows_the_latest_privileged_prompt() {
        let mut session = session(vec![
            ("configure terminal", "\r\nSwitch(config)#"),
            ("hostname closet-2-sw1", "\r\ncloset-2-sw1(config)#"),
            ("end", "\r\ncloset-2-sw1#"),
        ]);
        session
            .configure(&strings(&["hostname closet-2-sw1"]))
            .unwrap();
        assert_eq!(session.hostname(), "closet-2-sw1");
    }

    #[test]
    fn a_banner_is_sent_through_its_closing_delimiter_without_waiting() {
        let mut session = session(vec![
            ("configure terminal", "\r\nSwitch(config)#"),
            (
                "banner motd ^C",
                "\r\nEnter TEXT message.  End with the character '^C'.\r\n",
            ),
            ("Bench switch", ""),
            ("end", ""),
            ("^C", "\r\nSwitch(config)#"),
            ("interface GigabitEthernet1/0/1", "\r\nSwitch(config-if)#"),
            ("end", "\r\nSwitch#"),
        ]);
        let lines = strings(&[
            "banner motd ^C",
            "Bench switch",
            "end",
            "^C",
            "interface GigabitEthernet1/0/1",
        ]);
        assert!(session.configure(&lines).unwrap().is_empty());
    }

    #[test]
    fn a_banner_may_close_on_its_own_line_or_use_another_delimiter() {
        let mut session = session(vec![
            ("configure terminal", "\r\nSwitch(config)#"),
            ("banner login #Authorized use only#", "\r\nSwitch(config)#"),
            (
                "banner exec %",
                "\r\nEnter TEXT message.  End with the character '%'.\r\n",
            ),
            (
                "Bench 100%",
                "\r\n% Invalid input detected at '^' marker.\r\nSwitch(config)#",
            ),
            ("end", "\r\nSwitch#"),
        ]);
        let lines = strings(&[
            "banner login #Authorized use only#",
            "banner exec %",
            "Bench 100%",
        ]);
        assert_eq!(
            session.configure(&lines).unwrap(),
            strings(&["banner exec %"])
        );
    }

    #[test]
    fn save_answers_the_destination_and_needs_ok() {
        let mut session = session(vec![
            (
                "copy running-config startup-config",
                "\r\nDestination filename [startup-config]? ",
            ),
            ("", "\r\nBuilding configuration...\r\n[OK]\r\nSwitch#"),
        ]);
        session.save().unwrap();
        assert_eq!(
            written(&session),
            ["copy running-config startup-config", ""]
        );
    }

    #[test]
    fn save_accepts_copy_complete_without_a_destination_prompt() {
        let mut session = session(vec![(
            "copy running-config startup-config",
            "\r\n[####################] 100%\r\nCopy complete.\r\nswitch#",
        )]);
        session.save().unwrap();
        assert_eq!(written(&session), ["copy running-config startup-config"]);
    }

    #[test]
    fn save_stops_on_an_error_or_without_ok() {
        for (output, reason_part) in [
            (
                "\r\n%Error opening nvram:startup-config (No space left on device)\r\nSwitch#",
                "%Error opening nvram:startup-config",
            ),
            ("\r\nBuilding configuration...\r\nSwitch#", "[OK]"),
        ] {
            let mut session = session(vec![
                (
                    "copy running-config startup-config",
                    "\r\nDestination filename [startup-config]? ",
                ),
                ("", output),
            ]);
            match session.save() {
                Err(SessionError::Stopped(reason)) => assert!(reason.contains(reason_part)),
                other => panic!("unexpected {other:?}"),
            }
        }
    }

    #[test]
    fn save_and_copy_stops_include_the_device_output() {
        let mut saving = session(vec![
            (
                "copy running-config startup-config",
                "\r\nDestination filename [startup-config]? ",
            ),
            ("", "\r\nBuilding configuration...\r\nSwitch#"),
        ]);
        match saving.save() {
            Err(SessionError::Stopped(reason)) => {
                assert!(reason.contains("Building configuration..."))
            }
            other => panic!("unexpected {other:?}"),
        }

        let command = "copy tftp://192.0.2.1/image.17.12.05.bin flash:";
        let mut copy = session(vec![
            (command, "\r\nDestination filename [image.17.12.05.bin]? "),
            (
                "",
                "\r\nAccessing tftp://192.0.2.1/image.17.12.05.bin...\r\nSwitch#",
            ),
        ]);
        match copy.copy_image(command) {
            Err(SessionError::Stopped(reason)) => {
                assert!(reason.contains("Accessing tftp://192.0.2.1/image.17.12.05.bin"))
            }
            other => panic!("unexpected {other:?}"),
        }
    }

    #[test]
    fn copy_image_answers_the_destination_and_the_overwrite() {
        let command = "copy tftp://192.0.2.1/cat9k_iosxe.17.12.05.SPA.bin flash:";
        let mut session = session(vec![
            (
                command,
                "\r\nDestination filename [cat9k_iosxe.17.12.05.SPA.bin]? ",
            ),
            (
                "",
                "\r\n%Warning:There is a file already existing with this name\r\nDo you want to over write? [confirm]",
            ),
            (
                "",
                "\r\nAccessing tftp://192.0.2.1/cat9k_iosxe.17.12.05.SPA.bin...\r\n!!!!!!!!\r\n[OK - 1234567 bytes]\r\n\r\n1234567 bytes copied in 812.345 secs (1520 bytes/sec)\r\nSwitch#",
            ),
        ]);
        session.copy_image(command).unwrap();
        assert_eq!(written(&session), [command, "", ""]);
    }

    #[test]
    fn copy_image_accepts_a_nexus_tftp_success() {
        let command = "copy tftp://192.0.2.1/nxos.10.3.6.bin bootflash:nxos.10.3.6.bin";
        let mut session = session(vec![(
            command,
            "\r\nTFTP get operation was successful\r\nCopy complete, now saving to disk (please wait)...\r\nswitch#",
        )]);
        session.copy_image(command).unwrap();
        assert_eq!(written(&session), [command]);
    }

    #[test]
    fn copy_image_answers_y_to_a_nexus_overwrite() {
        let command = "copy tftp://192.0.2.1/nxos.10.3.6.bin bootflash:nxos.10.3.6.bin";
        let mut session = session(vec![
            (
                command,
                "\r\nnxos.10.3.6.bin already exists, overwrite? (y/n) [n] ",
            ),
            (
                "y",
                "\r\nTFTP get operation was successful\r\nCopy complete, now saving to disk (please wait)...\r\nswitch#",
            ),
        ]);
        session.copy_image(command).unwrap();
        assert_eq!(written(&session), [command, "y"]);
    }

    #[test]
    fn copy_image_stops_on_an_error_or_without_bytes_copied() {
        let command = "copy tftp://192.0.2.1/image.17.12.05.bin flash:";
        for (output, reason_part) in [
            (
                "\r\n%Error opening tftp://192.0.2.1/image.17.12.05.bin (Timed out)\r\nSwitch#",
                "%Error opening tftp://192.0.2.1/image.17.12.05.bin (Timed out)",
            ),
            (
                "\r\nAccessing tftp://192.0.2.1/image.17.12.05.bin...\r\nSwitch#",
                "bytes copied",
            ),
        ] {
            let mut session = session(vec![
                (command, "\r\nDestination filename [image.17.12.05.bin]? "),
                ("", output),
            ]);
            match session.copy_image(command) {
                Err(SessionError::Stopped(reason)) => assert!(reason.contains(reason_part)),
                other => panic!("unexpected {other:?}"),
            }
        }
    }

    #[test]
    fn install_answers_y_and_returns_once_the_device_reloads() {
        let command = "install add file flash:cat9k_iosxe.17.12.05.SPA.bin activate commit";
        let mut session = session(vec![
            (
                command,
                "\r\ninstall_add_activate_commit: START Wed Oct 28 09:00:00 UTC 2026\r\nDo you want to save the configuration? [yes/no]: ",
            ),
            (
                "no",
                "\r\nThis operation may require a reload of the system. Do you want to proceed? [y/n]",
            ),
            (
                "y",
                "\r\n--- Starting Commit ---\r\nSUCCESS: install_add_activate_commit Wed Oct 28 09:20:00 UTC 2026\r\n",
            ),
        ]);
        session.install("cat9k_iosxe.17.12.05.SPA.bin").unwrap();
        assert_eq!(written(&session), [command, "no", "y"]);
    }

    #[test]
    fn install_stops_on_a_failure_before_or_after_proceeding() {
        let command = "install add file flash:image.17.12.05.bin activate commit";
        let failed = "FAILED: install_add_activate_commit exit(1) Wed Oct 28 09:01:00 UTC 2026";
        let before = format!("\r\ninstall_add_activate_commit: START\r\n{failed}\r\nSwitch#");
        let after = format!("\r\n--- Starting Commit ---\r\n{failed}\r\nSwitch#");
        let scripts = [
            vec![(command, before.as_str())],
            vec![
                (command, "\r\nDo you want to proceed? [y/n]"),
                ("y", after.as_str()),
            ],
        ];
        for steps in scripts {
            let mut session = session(steps);
            match session.install("image.17.12.05.bin") {
                Err(SessionError::Stopped(reason)) => assert!(reason.contains(failed)),
                other => panic!("unexpected {other:?}"),
            }
        }
    }

    #[test]
    fn run_install_returns_when_the_switch_is_reloaded_without_a_prompt() {
        let command = "install all nxos bootflash:nxos.10.3.6.bin non-interactive";
        let mut session = session(vec![(
            command,
            "\r\nCompatibility check is done.\r\nSwitch will be reloaded for disruptive upgrade.\r\n",
        )]);
        session.run_install(command).unwrap();
        assert_eq!(written(&session), [command]);
    }

    #[test]
    fn access_point_install_finishes_on_device_will_reload_then_reads_the_factory_login() {
        let command = "archive download-sw /overwrite /reload flash:ap3g3-k9w8.17.12.05.tar";
        let mut session = session(vec![
            (
                command,
                "archive download-sw /overwrite /reload flash:ap3g3-k9w8.17.12.05.tar\r\nexamining image...\r\nThis operation may require a reload of the system.\r\nImage download complete.\r\nDevice will reload.\r\n\r\nWould you like to enter the initial configuration dialog? [yes/no]: ",
            ),
            ("", ""),
            ("no", "\r\nAP>"),
            ("enable", "\r\nAP#"),
            ("terminal length 0", "\r\nAP#"),
        ]);
        session.run_install(command).unwrap();
        let during_install = session.transcript();
        assert!(during_install.contains("may require a reload"));
        assert!(during_install.contains("Device will reload"));
        assert!(!during_install
            .to_ascii_lowercase()
            .contains("initial configuration dialog"));
        assert_eq!(written(&session), [command]);
        session.login_factory().unwrap();
        assert!(session
            .transcript()
            .to_ascii_lowercase()
            .contains("initial configuration dialog"));
        assert_eq!(
            written(&session),
            [command, "", "no", "enable", "terminal length 0"]
        );
    }
}
