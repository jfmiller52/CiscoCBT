//! Console login state machines. The console session classifies device
//! output into events and writes whatever the machine tells it to send.

const FACTORY_SECRET: &str = "cisco";
const FALLBACK_SECRET: &str = "Passw0rd%";

/// Credentials the operator must remember after a factory login created them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FactorySecrets {
    pub username: String,
    pub password: String,
    pub enable_secret: String,
}

/// What the console session saw on a factory-fresh device.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LoginEvent {
    InitialDialog,
    UsernamePrompt,
    PasswordPrompt,
    ConfirmPassword,
    EnableSecretPrompt,
    UserExec,
    Rejected { too_short: bool, raw: String },
    Privileged,
}

/// What the console session should do next.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LoginAction {
    Send(&'static str),
    Wait,
    Stop { reason: String },
    InPrivileged { reminder: Option<FactorySecrets> },
}

/// Logs into a factory-fresh device, creating the account and enable secret
/// when the device asks for them.
#[derive(Debug)]
pub struct FactoryLogin {
    secret: &'static str,
    saw_username_prompt: bool,
    saw_account_prompt: bool,
    stopped: Option<LoginAction>,
}

impl FactoryLogin {
    pub fn new() -> Self {
        Self {
            secret: FACTORY_SECRET,
            saw_username_prompt: false,
            saw_account_prompt: false,
            stopped: None,
        }
    }

    pub fn on_event(&mut self, event: LoginEvent) -> LoginAction {
        if let Some(stop) = &self.stopped {
            return stop.clone();
        }
        match event {
            LoginEvent::InitialDialog => LoginAction::Send("no"),
            LoginEvent::UsernamePrompt => {
                self.saw_username_prompt = true;
                self.saw_account_prompt = true;
                LoginAction::Send(FACTORY_SECRET)
            }
            LoginEvent::PasswordPrompt
            | LoginEvent::ConfirmPassword
            | LoginEvent::EnableSecretPrompt => {
                self.saw_account_prompt = true;
                LoginAction::Send(self.secret)
            }
            LoginEvent::UserExec => LoginAction::Send("enable"),
            LoginEvent::Rejected { too_short, raw } => {
                if too_short && self.secret == FACTORY_SECRET {
                    self.secret = FALLBACK_SECRET;
                    LoginAction::Wait
                } else {
                    self.stop(format!("device rejected the password: {raw}"))
                }
            }
            LoginEvent::Privileged => LoginAction::InPrivileged {
                reminder: self.saw_account_prompt.then(|| FactorySecrets {
                    username: if self.saw_username_prompt {
                        FACTORY_SECRET.to_string()
                    } else {
                        String::new()
                    },
                    password: self.secret.to_string(),
                    enable_secret: self.secret.to_string(),
                }),
            },
        }
    }

    fn stop(&mut self, reason: String) -> LoginAction {
        let action = LoginAction::Stop { reason };
        self.stopped = Some(action.clone());
        action
    }
}

impl Default for FactoryLogin {
    fn default() -> Self {
        Self::new()
    }
}

/// How to log in on the console once the technician's file has been applied.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NextLogin {
    /// The file leaves the console alone; use the factory login.
    Factory(FactorySecrets),
    /// The file changes the console login to one the program can type.
    User {
        username: String,
        password: String,
        enable_secret: String,
    },
    /// The login is a hash or is ambiguous; stop before the reboot.
    Stop { lines: Vec<String> },
}

const TYPE7_KEY: &[u8] = b"dsfd;kfoA,.iyewrkldJKDHSUBsgvca69834ncx";
/// IOS only produces type 7 seeds 0 through 15.
const TYPE7_MAX_INDEX: usize = 15;

/// Decodes a Cisco type 7 string. Returns `None` when it is malformed.
pub fn decode_type7(encoded: &str) -> Option<String> {
    if !encoded.is_ascii() || encoded.len() < 4 || encoded.len() % 2 != 0 {
        return None;
    }
    let (index, body) = encoded.split_at(2);
    if !index.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let index: usize = index.parse().ok()?;
    if index > TYPE7_MAX_INDEX {
        return None;
    }
    let mut decoded = Vec::with_capacity(body.len() / 2);
    for (i, pair) in body.as_bytes().chunks(2).enumerate() {
        let byte = u8::from_str_radix(std::str::from_utf8(pair).ok()?, 16).ok()?;
        let plain = byte ^ TYPE7_KEY[(index + i) % TYPE7_KEY.len()];
        if !(0x20..=0x7e).contains(&plain) {
            return None;
        }
        decoded.push(plain);
    }
    String::from_utf8(decoded).ok()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    AaaLogin,
    User { privilege_15: bool },
    EnableSecret,
    EnablePassword,
    ConsolePassword,
}

/// A line of the file that bears on the console login. `value` is what the
/// program could type, or `None` when it is a hash or cannot be decoded.
struct Entry<'a> {
    line: &'a str,
    kind: Kind,
    value: Option<String>,
}

/// Splits off the first whitespace-delimited token.
fn split_token(s: &str) -> Option<(&str, &str)> {
    let s = s.trim_start();
    if s.is_empty() {
        return None;
    }
    let end = s.find(char::is_whitespace).unwrap_or(s.len());
    Some(s.split_at(end))
}

/// Reads `[TYPE] VALUE` and returns the typable password, if any.
fn typable_value(rest: &str) -> Option<String> {
    let (kind, value) = match split_token(rest) {
        Some((token, after)) if matches!(token, "0" | "5" | "7" | "8" | "9") => {
            (token, after.trim())
        }
        _ => ("0", rest.trim()),
    };
    if value.is_empty()
        || ["$1$", "$5$", "$8$", "$9$"]
            .iter()
            .any(|p| value.starts_with(p))
    {
        return None;
    }
    match kind {
        "0" => Some(value.to_string()),
        "7" => decode_type7(value),
        _ => None,
    }
}

fn is_word(token: &str, word: &str) -> bool {
    token.eq_ignore_ascii_case(word)
}

fn parse_global(line: &str) -> Option<Entry<'_>> {
    let (command, rest) = split_token(line)?;
    if is_word(command, "username") {
        let (_, mut rest) = split_token(rest)?;
        let mut privilege_15 = false;
        let mut value = None;
        while let Some((token, after)) = split_token(rest) {
            rest = after;
            if is_word(token, "privilege") {
                let (level, after) = split_token(rest).unwrap_or(("", ""));
                privilege_15 = level == "15";
                rest = after;
            } else if is_word(token, "password") || is_word(token, "secret") {
                value = typable_value(rest);
                break;
            }
        }
        return Some(Entry {
            line,
            kind: Kind::User { privilege_15 },
            value,
        });
    }
    if is_word(command, "enable") {
        let (word, mut rest) = split_token(rest)?;
        let kind = if is_word(word, "secret") {
            Kind::EnableSecret
        } else if is_word(word, "password") {
            Kind::EnablePassword
        } else if is_word(word, "algorithm-type") {
            let (_, after) = split_token(rest)?;
            let (secret, after) = split_token(after)?;
            if !is_word(secret, "secret") {
                return None;
            }
            rest = after;
            Kind::EnableSecret
        } else {
            return None;
        };
        if let Some((level, after)) = split_token(rest) {
            if is_word(level, "level") {
                let (number, after) = split_token(after)?;
                if number != "15" {
                    return None;
                }
                rest = after;
            }
        }
        return Some(Entry {
            line,
            kind,
            value: typable_value(rest),
        });
    }
    if is_word(command, "aaa") {
        let (second, rest) = split_token(rest)?;
        let (third, _) = split_token(rest)?;
        if is_word(second, "authentication") && is_word(third, "login") {
            return Some(Entry {
                line,
                kind: Kind::AaaLogin,
                value: None,
            });
        }
    }
    None
}

fn is_console_header(line: &str) -> bool {
    let mut tokens = line.split_whitespace();
    matches!(
        (tokens.next(), tokens.next(), tokens.next(), tokens.next()),
        (Some(line), Some(con), Some("0"), None)
            if is_word(line, "line") && (is_word(con, "con") || is_word(con, "console"))
    )
}

fn scan(uploaded: &str) -> Vec<Entry<'_>> {
    let mut entries = Vec::new();
    let mut in_console = false;
    for line in uploaded.lines() {
        if line.trim().is_empty() {
            continue;
        }
        let indented = line.starts_with(char::is_whitespace);
        if !indented {
            in_console = is_console_header(line);
        }
        if in_console && indented {
            if let Some((word, rest)) = split_token(line) {
                if is_word(word, "password") {
                    entries.push(Entry {
                        line,
                        kind: Kind::ConsolePassword,
                        value: typable_value(rest),
                    });
                }
            }
        } else if let Some(entry) = parse_global(line) {
            entries.push(entry);
        }
    }
    entries
}

fn lines_of(entries: &[&Entry<'_>]) -> Vec<String> {
    entries.iter().map(|e| e.line.to_string()).collect()
}

/// Decides how to log in after the technician's file has been applied and
/// saved, or stops before the reboot when the console would be unreachable.
pub fn login_after_apply(uploaded: &str, factory: &FactorySecrets) -> NextLogin {
    let entries = scan(uploaded);
    let of_kind = |pick: fn(&Kind) -> bool| -> Vec<&Entry<'_>> {
        entries.iter().filter(|e| pick(&e.kind)).collect()
    };

    let aaa = of_kind(|k| matches!(k, Kind::AaaLogin));
    if !aaa.is_empty() {
        return NextLogin::Stop {
            lines: lines_of(&aaa),
        };
    }

    let users = of_kind(|k| matches!(k, Kind::User { .. }));
    let admins: Vec<&Entry<'_>> = users
        .iter()
        .copied()
        .filter(|e| matches!(e.kind, Kind::User { privilege_15: true }) && e.value.is_some())
        .collect();
    let enable_secrets = of_kind(|k| matches!(k, Kind::EnableSecret));
    let enable_passwords = of_kind(|k| matches!(k, Kind::EnablePassword));
    let last_value = |list: &[&Entry<'_>]| list.iter().rev().find_map(|e| e.value.clone());
    // The enable secret outranks the enable password on the device.
    let enable_value = if enable_secrets.is_empty() {
        last_value(&enable_passwords)
    } else {
        last_value(&enable_secrets)
    };

    match admins.as_slice() {
        [] => {}
        [admin] => {
            let username = admin
                .line
                .split_whitespace()
                .nth(1)
                .unwrap_or_default()
                .to_string();
            return NextLogin::User {
                username,
                password: admin.value.clone().unwrap_or_default(),
                enable_secret: enable_value.unwrap_or_else(|| factory.enable_secret.clone()),
            };
        }
        many => {
            return NextLogin::Stop {
                lines: lines_of(many),
            }
        }
    }
    if !users.is_empty() {
        return NextLogin::Stop {
            lines: lines_of(&users),
        };
    }

    let console = of_kind(|k| matches!(k, Kind::ConsolePassword));
    let hashed: Vec<&Entry<'_>> = entries.iter().filter(|e| e.value.is_none()).collect();
    if !hashed.is_empty() {
        return NextLogin::Stop {
            lines: lines_of(&hashed),
        };
    }
    if console.is_empty() && enable_secrets.is_empty() && enable_passwords.is_empty() {
        return NextLogin::Factory(factory.clone());
    }
    NextLogin::User {
        username: factory.username.clone(),
        password: last_value(&console).unwrap_or_else(|| factory.password.clone()),
        enable_secret: enable_value.unwrap_or_else(|| factory.enable_secret.clone()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn open_console_sends_no_then_enable() {
        let mut login = FactoryLogin::new();
        assert_eq!(
            login.on_event(LoginEvent::InitialDialog),
            LoginAction::Send("no")
        );
        assert_eq!(
            login.on_event(LoginEvent::UserExec),
            LoginAction::Send("enable")
        );
        assert!(matches!(
            login.on_event(LoginEvent::Privileged),
            LoginAction::InPrivileged { reminder: None }
        ));
    }

    #[test]
    fn short_password_retries_with_passw0rd_percent() {
        let mut login = FactoryLogin::new();
        login.on_event(LoginEvent::UsernamePrompt);
        login.on_event(LoginEvent::PasswordPrompt);
        let retry = login.on_event(LoginEvent::Rejected {
            too_short: true,
            raw: "% Password rejected. Too short.".into(),
        });
        assert_eq!(retry, LoginAction::Wait);
        assert_eq!(
            login.on_event(LoginEvent::PasswordPrompt),
            LoginAction::Send("Passw0rd%")
        );
        assert_eq!(
            login.on_event(LoginEvent::ConfirmPassword),
            LoginAction::Send("Passw0rd%")
        );
        let done = login.on_event(LoginEvent::Privileged);
        match done {
            LoginAction::InPrivileged {
                reminder: Some(secrets),
            } => {
                assert_eq!(secrets.username, "cisco");
                assert_eq!(secrets.password, "Passw0rd%");
                assert_eq!(secrets.enable_secret, "Passw0rd%");
            }
            other => panic!("unexpected {other:?}"),
        }
    }

    #[test]
    fn a_rejection_that_is_not_too_short_stops() {
        let mut login = FactoryLogin::new();
        login.on_event(LoginEvent::PasswordPrompt);
        let action = login.on_event(LoginEvent::Rejected {
            too_short: false,
            raw: "% Password rejected.".into(),
        });
        assert!(matches!(action, LoginAction::Stop { .. }));
    }

    #[test]
    fn username_prompt_sends_cisco() {
        let mut login = FactoryLogin::new();
        assert_eq!(
            login.on_event(LoginEvent::UsernamePrompt),
            LoginAction::Send("cisco")
        );
        assert_eq!(
            login.on_event(LoginEvent::PasswordPrompt),
            LoginAction::Send("cisco")
        );
        assert_eq!(
            login.on_event(LoginEvent::ConfirmPassword),
            LoginAction::Send("cisco")
        );
        assert_eq!(
            login.on_event(LoginEvent::EnableSecretPrompt),
            LoginAction::Send("cisco")
        );
    }

    #[test]
    fn reminder_without_username_prompt_has_empty_username() {
        let mut login = FactoryLogin::new();
        login.on_event(LoginEvent::EnableSecretPrompt);
        match login.on_event(LoginEvent::Privileged) {
            LoginAction::InPrivileged {
                reminder: Some(secrets),
            } => {
                assert_eq!(secrets.username, "");
                assert_eq!(secrets.password, "cisco");
                assert_eq!(secrets.enable_secret, "cisco");
            }
            other => panic!("unexpected {other:?}"),
        }
    }

    #[test]
    fn privileged_may_repeat_with_the_same_reminder() {
        let mut login = FactoryLogin::new();
        login.on_event(LoginEvent::UsernamePrompt);
        let first = login.on_event(LoginEvent::Privileged);
        let second = login.on_event(LoginEvent::Privileged);
        assert!(matches!(
            first,
            LoginAction::InPrivileged { reminder: Some(_) }
        ));
        assert_eq!(first, second);
    }

    #[test]
    fn rejection_after_fallback_stops_even_when_too_short() {
        let mut login = FactoryLogin::new();
        login.on_event(LoginEvent::PasswordPrompt);
        let raw = "% Password rejected. Too short.";
        assert_eq!(
            login.on_event(LoginEvent::Rejected {
                too_short: true,
                raw: raw.into()
            }),
            LoginAction::Wait
        );
        match login.on_event(LoginEvent::Rejected {
            too_short: true,
            raw: raw.into(),
        }) {
            LoginAction::Stop { reason } => assert!(reason.contains(raw)),
            other => panic!("unexpected {other:?}"),
        }
    }

    #[test]
    fn stop_is_sticky() {
        let mut login = FactoryLogin::new();
        let stop = login.on_event(LoginEvent::Rejected {
            too_short: false,
            raw: "% nope".into(),
        });
        assert!(matches!(stop, LoginAction::Stop { .. }));
        assert_eq!(login.on_event(LoginEvent::InitialDialog), stop);
        assert_eq!(login.on_event(LoginEvent::PasswordPrompt), stop);
        assert_eq!(login.on_event(LoginEvent::Privileged), stop);
    }

    fn factory() -> FactorySecrets {
        FactorySecrets {
            username: "cisco".into(),
            password: "cisco".into(),
            enable_secret: "cisco".into(),
        }
    }

    #[test]
    fn hashed_enable_secret_stops_before_reboot() {
        let file = "enable secret 9 $9$abc\n";
        assert!(matches!(
            login_after_apply(file, &factory()),
            NextLogin::Stop { .. }
        ));
    }

    #[test]
    fn one_privilege_15_user_is_used() {
        let file = "username bench privilege 15 password BenchPass\n";
        match login_after_apply(file, &factory()) {
            NextLogin::User {
                username,
                password,
                enable_secret,
            } => {
                assert_eq!(username, "bench");
                assert_eq!(password, "BenchPass");
                assert_eq!(enable_secret, "cisco");
            }
            other => panic!("unexpected {other:?}"),
        }
    }

    #[test]
    fn two_typable_privilege_15_users_stop() {
        let file = "username a privilege 15 secret 0 One\nusername b privilege 15 secret 0 Two\n";
        match login_after_apply(file, &factory()) {
            NextLogin::Stop { lines } => assert_eq!(lines.len(), 2),
            other => panic!("unexpected {other:?}"),
        }
    }

    #[test]
    fn type7_cisco_decodes() {
        assert_eq!(decode_type7("02050D480809").as_deref(), Some("cisco"));
    }

    #[test]
    fn unchanged_console_keeps_the_factory_login() {
        let factory = FactorySecrets {
            username: "cisco".into(),
            password: "Passw0rd%".into(),
            enable_secret: "Passw0rd%".into(),
        };
        match login_after_apply("hostname closet-2-sw1\n", &factory) {
            NextLogin::Factory(secrets) => assert_eq!(secrets.password, "Passw0rd%"),
            other => panic!("unexpected {other:?}"),
        }
    }

    #[test]
    fn malformed_type7_decodes_to_none() {
        assert_eq!(decode_type7(""), None);
        assert_eq!(decode_type7("02"), None);
        assert_eq!(decode_type7("02050D48080"), None);
        assert_eq!(decode_type7("02050D48080Z"), None);
        assert_eq!(decode_type7("99050D480809"), None);
        assert_eq!(decode_type7("0é050D4808"), None);
    }

    #[test]
    fn aaa_authentication_login_stops() {
        let file = "aaa new-model\naaa authentication login default group tacacs+ local\n";
        match login_after_apply(file, &factory()) {
            NextLogin::Stop { lines } => assert_eq!(
                lines,
                vec!["aaa authentication login default group tacacs+ local".to_string()]
            ),
            other => panic!("unexpected {other:?}"),
        }
    }

    #[test]
    fn hashed_privilege_15_user_stops_with_the_username_lines() {
        let file = "username a privilege 15 algorithm-type scrypt secret 9 $9$abc\n";
        match login_after_apply(file, &factory()) {
            NextLogin::Stop { lines } => assert_eq!(lines, vec![file.trim_end().to_string()]),
            other => panic!("unexpected {other:?}"),
        }
    }

    #[test]
    fn user_without_privilege_15_is_not_a_login() {
        let file = "username a privilege 5 secret 0 One\nusername b password 0 Two\n";
        match login_after_apply(file, &factory()) {
            NextLogin::Stop { lines } => assert_eq!(lines.len(), 2),
            other => panic!("unexpected {other:?}"),
        }
    }

    #[test]
    fn one_typable_user_beside_others_is_used_with_options_and_type7() {
        let file = "username ro privilege 1 secret 9 $9$zzz\n\
                    username op privilege 15 algorithm-type scrypt password 7 02050D480809\n";
        match login_after_apply(file, &factory()) {
            NextLogin::User {
                username, password, ..
            } => {
                assert_eq!(username, "op");
                assert_eq!(password, "cisco");
            }
            other => panic!("unexpected {other:?}"),
        }
    }

    #[test]
    fn value_is_the_rest_of_the_line() {
        let file = "username bench privilege 15 secret my pass word\n";
        match login_after_apply(file, &factory()) {
            NextLogin::User { password, .. } => assert_eq!(password, "my pass word"),
            other => panic!("unexpected {other:?}"),
        }
    }

    #[test]
    fn privilege_15_user_ignores_hashed_enable_and_uses_typable_enable() {
        let file = "enable secret 9 $9$abc\nusername a privilege 15 secret 0 One\n";
        assert!(matches!(
            login_after_apply(file, &factory()),
            NextLogin::User { enable_secret, .. } if enable_secret == "cisco"
        ));
        let file = "enable secret 0 Open\nusername a privilege 15 secret 0 One\n";
        assert!(matches!(
            login_after_apply(file, &factory()),
            NextLogin::User { enable_secret, .. } if enable_secret == "Open"
        ));
        let file = "enable password Legacy\nusername a privilege 15 secret 0 One\n";
        assert!(matches!(
            login_after_apply(file, &factory()),
            NextLogin::User { enable_secret, .. } if enable_secret == "Legacy"
        ));
    }

    #[test]
    fn undecodable_type7_is_not_typable() {
        let file = "username a privilege 15 password 7 02ZZ\n";
        assert!(matches!(
            login_after_apply(file, &factory()),
            NextLogin::Stop { .. }
        ));
    }

    #[test]
    fn console_password_and_enable_are_used_when_typable() {
        let file = "enable secret 0 EnPass\nline con 0\n password 7 02050D480809\n login\nline vty 0 4\n password 9 $9$ignored\n";
        match login_after_apply(file, &factory()) {
            NextLogin::User {
                username,
                password,
                enable_secret,
            } => {
                assert_eq!(username, "cisco");
                assert_eq!(password, "cisco");
                assert_eq!(enable_secret, "EnPass");
            }
            other => panic!("unexpected {other:?}"),
        }
    }

    #[test]
    fn hashed_console_password_stops_with_that_line() {
        let file = "line console 0\n password 5 $1$abc$def\n";
        match login_after_apply(file, &factory()) {
            NextLogin::Stop { lines } => {
                assert_eq!(lines, vec![" password 5 $1$abc$def".to_string()])
            }
            other => panic!("unexpected {other:?}"),
        }
    }

    #[test]
    fn other_line_sections_are_ignored() {
        let file = "line vty 0 4\n password 5 $1$abc$def\nline aux 0\n password 7 zz\n";
        assert!(matches!(
            login_after_apply(file, &factory()),
            NextLogin::Factory(_)
        ));
    }

    #[test]
    fn console_password_after_another_section_is_not_the_console() {
        let file = "line con 0\n login\n!\ninterface Vlan1\n password 5 $1$x\n";
        assert!(matches!(
            login_after_apply(file, &factory()),
            NextLogin::Factory(_)
        ));
    }

    #[test]
    fn enable_algorithm_type_counts_as_enable_secret() {
        let file = "enable algorithm-type scrypt secret Plain1\n";
        assert!(matches!(
            login_after_apply(file, &factory()),
            NextLogin::User { enable_secret, .. } if enable_secret == "Plain1"
        ));
    }

    #[test]
    fn enable_secret_at_another_level_is_ignored() {
        let file = "enable secret level 5 9 $9$abc\nenable secret level 15 0 Top\n";
        assert!(matches!(
            login_after_apply(file, &factory()),
            NextLogin::User { enable_secret, .. } if enable_secret == "Top"
        ));
        let file = "enable secret level 5 9 $9$abc\n";
        assert!(matches!(
            login_after_apply(file, &factory()),
            NextLogin::Factory(_)
        ));
    }

    #[test]
    fn hashed_value_without_a_type_is_not_typable() {
        let file = "enable secret $9$abc\n";
        assert!(matches!(
            login_after_apply(file, &factory()),
            NextLogin::Stop { .. }
        ));
    }

    #[test]
    fn crlf_lines_are_returned_without_the_line_ending() {
        let file = "enable secret 5 $1$abc\r\n";
        match login_after_apply(file, &factory()) {
            NextLogin::Stop { lines } => {
                assert_eq!(lines, vec!["enable secret 5 $1$abc".to_string()])
            }
            other => panic!("unexpected {other:?}"),
        }
    }
}
