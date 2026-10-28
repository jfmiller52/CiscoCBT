use crate::RunOptions;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Phase {
    Login,
    NameTftpPort,
    CopyImage,
    InstallAndReboot,
    ConfirmVersion,
    ApplyConfig,
    SaveAndReboot,
    ValidateConfig,
    CleanReboot,
    WaitBurnIn,
    ReadUptime,
    Done,
    Stopped,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PhaseEvent {
    LoggedIn,
    CableConfirmed,
    ImageOnFlash,
    Rebooted,
    VersionMatched,
    VersionRejected,
    ConfigApplied,
    Saved,
    ComparisonPassed,
    ComparisonFailed,
    WaitFinished,
    UptimePassed,
    UptimeFailed,
}

pub fn start_phase(_options: &RunOptions) -> Phase {
    Phase::Login
}

pub fn next_phase(current: Phase, options: &RunOptions, event: PhaseEvent) -> Phase {
    use Phase::*;
    use PhaseEvent::*;
    match (current, event) {
        (Done, _) | (Stopped, _) => current,
        (Login, LoggedIn) => {
            if options.firmware.is_some() {
                NameTftpPort
            } else {
                ApplyConfig
            }
        }
        (NameTftpPort, CableConfirmed) => CopyImage,
        (CopyImage, ImageOnFlash) => InstallAndReboot,
        (InstallAndReboot, Rebooted) => ConfirmVersion,
        (ConfirmVersion, VersionMatched) => ApplyConfig,
        (ConfirmVersion, VersionRejected) => Stopped,
        (ApplyConfig, ConfigApplied) => SaveAndReboot,
        (SaveAndReboot, Rebooted) => ValidateConfig,
        (ValidateConfig, ComparisonPassed) => CleanReboot,
        (ValidateConfig, ComparisonFailed) => Stopped,
        (CleanReboot, Rebooted) => {
            if options.burn_in_hours.is_some() {
                WaitBurnIn
            } else {
                Done
            }
        }
        (WaitBurnIn, WaitFinished) => ReadUptime,
        (ReadUptime, UptimePassed) => Done,
        (ReadUptime, UptimeFailed) => Stopped,
        (_, LoggedIn) => current,
        (SaveAndReboot, Saved) => current,
        _ => current,
    }
}

pub fn port_walk_available(current: Phase) -> bool {
    matches!(
        current,
        Phase::CleanReboot | Phase::WaitBurnIn | Phase::ReadUptime | Phase::Done
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{FirmwareChoice, RunOptions};

    fn sample_with_firmware_and_burn_in() -> RunOptions {
        RunOptions {
            serial_device: "tty".into(),
            config_path: "cfg".into(),
            family: crate::DeviceFamily::IosXe,
            firmware: Some(FirmwareChoice {
                image_path: "img".into(),
                local_ethernet: "eth0".into(),
            }),
            burn_in_hours: Some(24),
            port_testing: false,
            capwap: None,
        }
    }

    fn sample_config_only() -> RunOptions {
        RunOptions {
            serial_device: "tty".into(),
            config_path: "cfg".into(),
            family: crate::DeviceFamily::IosXe,
            firmware: None,
            burn_in_hours: None,
            port_testing: false,
            capwap: None,
        }
    }

    #[test]
    fn firmware_run_reboots_three_times_before_the_wait() {
        let options = sample_with_firmware_and_burn_in();
        let mut phase = start_phase(&options);
        assert_eq!(phase, Phase::Login);
        for event in [
            PhaseEvent::LoggedIn,
            PhaseEvent::CableConfirmed,
            PhaseEvent::ImageOnFlash,
            PhaseEvent::Rebooted,
            PhaseEvent::LoggedIn,
            PhaseEvent::VersionMatched,
            PhaseEvent::ConfigApplied,
            PhaseEvent::Saved,
            PhaseEvent::Rebooted,
            PhaseEvent::LoggedIn,
            PhaseEvent::ComparisonPassed,
            PhaseEvent::Rebooted,
            PhaseEvent::WaitFinished,
            PhaseEvent::UptimePassed,
        ] {
            phase = next_phase(phase, &options, event);
        }
        assert_eq!(phase, Phase::Done);
        assert!(port_walk_available(Phase::WaitBurnIn));
        assert!(!port_walk_available(Phase::ApplyConfig));
    }

    #[test]
    fn without_firmware_login_leads_to_apply_and_ends_after_the_clean_reboot() {
        let options = sample_config_only();
        let mut phase = start_phase(&options);
        assert_eq!(phase, Phase::Login);
        phase = next_phase(phase, &options, PhaseEvent::LoggedIn);
        assert_eq!(phase, Phase::ApplyConfig);
        for event in [
            PhaseEvent::ConfigApplied,
            PhaseEvent::Saved,
            PhaseEvent::Rebooted,
            PhaseEvent::LoggedIn,
            PhaseEvent::ComparisonPassed,
            PhaseEvent::Rebooted,
        ] {
            phase = next_phase(phase, &options, event);
        }
        assert_eq!(phase, Phase::Done);
    }

    #[test]
    fn a_failed_comparison_stops_before_the_clean_reboot() {
        let options = sample_config_only();
        assert_eq!(
            next_phase(
                Phase::ValidateConfig,
                &options,
                PhaseEvent::ComparisonFailed
            ),
            Phase::Stopped
        );
    }
}
