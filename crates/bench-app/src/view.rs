use std::time::Duration;

use bench_core::{
    port_walk_available, CheckStatus, FactorySecrets, Phase, RunOptions, RunRecord, StartForm,
};

#[derive(Debug, Clone, PartialEq)]
pub struct ViewState {
    pub form: StartForm,
    pub serial_devices: Vec<String>,
    pub ethernet_ports: Vec<String>,
    pub phase: Option<Phase>,
    pub record: RunRecord,
    pub reminder: Option<FactorySecrets>,
    pub tftp_port: Option<String>,
    pub ports: Vec<(String, CheckStatus)>,
    pub current_port: Option<String>,
    pub burn_in_remaining: Option<Duration>,
    pub transcript: String,
    pub export_text: Option<String>,
    pub stop_reason: Option<String>,
    pub hostname: Option<String>,
}

impl Default for ViewState {
    fn default() -> Self {
        Self::new()
    }
}

impl ViewState {
    pub fn new() -> Self {
        Self {
            form: StartForm::new(),
            serial_devices: Vec::new(),
            ethernet_ports: Vec::new(),
            phase: None,
            record: RunRecord {
                configuration: CheckStatus::NotChosen,
                firmware: CheckStatus::NotChosen,
                burn_in: CheckStatus::NotChosen,
                ports: CheckStatus::NotChosen,
                capwap: CheckStatus::NotChosen,
            },
            reminder: None,
            tftp_port: None,
            ports: Vec::new(),
            current_port: None,
            burn_in_remaining: None,
            transcript: String::new(),
            export_text: None,
            stop_reason: None,
            hostname: None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UiCommand {
    Start(RunOptions),
    ConfirmCable,
    CheckPort(String),
    Export,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Screen {
    StartForm,
    CablePrompt,
    PortWalk,
    Record,
}

pub fn screen_for(state: &ViewState) -> Screen {
    match state.phase {
        None => Screen::StartForm,
        Some(Phase::NameTftpPort) => Screen::CablePrompt,
        Some(phase)
            if state.form.port_testing
                && port_walk_available(phase, state.form.family)
                && state.current_port.is_some() =>
        {
            Screen::PortWalk
        }
        Some(_) => Screen::Record,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn walking(phase: Phase) -> ViewState {
        let mut state = ViewState::new();
        state.form.port_testing = true;
        state.phase = Some(phase);
        state.current_port = Some("Gi1/0/3".into());
        state
    }

    #[test]
    fn no_phase_shows_the_start_form() {
        assert_eq!(screen_for(&ViewState::new()), Screen::StartForm);
    }

    #[test]
    fn naming_the_tftp_port_shows_the_cable_prompt() {
        let mut state = ViewState::new();
        state.phase = Some(Phase::NameTftpPort);
        assert_eq!(screen_for(&state), Screen::CablePrompt);
    }

    #[test]
    fn chosen_available_port_walk_with_a_current_port_shows_the_port_walk() {
        assert_eq!(screen_for(&walking(Phase::WaitBurnIn)), Screen::PortWalk);
        assert_eq!(screen_for(&walking(Phase::Done)), Screen::PortWalk);
    }

    #[test]
    fn port_walk_falls_back_to_the_record_when_any_condition_is_missing() {
        let mut not_chosen = walking(Phase::WaitBurnIn);
        not_chosen.form.port_testing = false;
        assert_eq!(screen_for(&not_chosen), Screen::Record);

        assert_eq!(screen_for(&walking(Phase::ApplyConfig)), Screen::Record);

        let mut no_current = walking(Phase::WaitBurnIn);
        no_current.current_port = None;
        assert_eq!(screen_for(&no_current), Screen::Record);
    }

    #[test]
    fn running_and_stopped_phases_show_the_record() {
        let mut state = ViewState::new();
        state.phase = Some(Phase::Login);
        assert_eq!(screen_for(&state), Screen::Record);
        state.phase = Some(Phase::Stopped);
        assert_eq!(screen_for(&state), Screen::Record);
    }

    #[test]
    fn an_access_point_never_shows_the_port_walk() {
        use bench_core::DeviceFamily;

        for phase in [
            Phase::CleanReboot,
            Phase::WaitBurnIn,
            Phase::ReadUptime,
            Phase::Done,
            Phase::ApplyCapwap,
            Phase::ValidateCapwap,
        ] {
            let mut state = walking(phase);
            state.form.family = DeviceFamily::AccessPoint;
            state.form.port_testing = true;
            assert_eq!(screen_for(&state), Screen::Record, "{phase:?}");
        }
    }
}
