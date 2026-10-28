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
}
