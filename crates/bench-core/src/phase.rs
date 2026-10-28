use crate::{DeviceFamily, RunOptions};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Phase {
    Login,
    NameTftpPort,
    CopyImage,
    InstallAndReboot,
    ConfirmVersion,
    ApplyCapwap,
    ValidateCapwap,
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
    CapwapApplied,
    CapwapValidated,
    CapwapFailed,
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

fn after_login(options: &RunOptions) -> Phase {
    use Phase::*;
    if options.firmware.is_some() {
        NameTftpPort
    } else if options.family == DeviceFamily::AccessPoint {
        if options.capwap.is_some() {
            ApplyCapwap
        } else {
            Stopped
        }
    } else {
        ApplyConfig
    }
}

fn after_version_matched(options: &RunOptions) -> Phase {
    use Phase::*;
    if options.family == DeviceFamily::AccessPoint {
        if options.capwap.is_some() {
            ApplyCapwap
        } else {
            Done
        }
    } else {
        ApplyConfig
    }
}

pub fn next_phase(current: Phase, options: &RunOptions, event: PhaseEvent) -> Phase {
    use Phase::*;
    use PhaseEvent::*;
    match (current, event) {
        (Done, _) | (Stopped, _) => current,
        (Login, LoggedIn) => after_login(options),
        (NameTftpPort, CableConfirmed) => CopyImage,
        (CopyImage, ImageOnFlash) => InstallAndReboot,
        (InstallAndReboot, Rebooted) => ConfirmVersion,
        (ConfirmVersion, VersionMatched) => after_version_matched(options),
        (ConfirmVersion, VersionRejected) => Stopped,
        (ApplyCapwap, CapwapApplied) => ValidateCapwap,
        (ValidateCapwap, CapwapValidated) => Done,
        (ValidateCapwap, CapwapFailed) => Stopped,
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

pub fn port_walk_available(current: Phase, family: DeviceFamily) -> bool {
    if family == DeviceFamily::AccessPoint {
        return false;
    }
    matches!(
        current,
        Phase::CleanReboot | Phase::WaitBurnIn | Phase::ReadUptime | Phase::Done
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{CapwapChoice, DeviceFamily, FirmwareChoice, RunOptions};

    fn sample_with_firmware_and_burn_in() -> RunOptions {
        RunOptions {
            serial_device: "tty".into(),
            config_path: "cfg".into(),
            family: DeviceFamily::IosXe,
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
            family: DeviceFamily::IosXe,
            firmware: None,
            burn_in_hours: None,
            port_testing: false,
            capwap: None,
        }
    }

    fn sample_capwap() -> CapwapChoice {
        CapwapChoice {
            ip: "10.0.0.1".into(),
            mask: "255.255.255.0".into(),
            gateway: "10.0.0.254".into(),
        }
    }

    fn access_point_options(firmware: Option<FirmwareChoice>, capwap: Option<CapwapChoice>) -> RunOptions {
        RunOptions {
            serial_device: "tty".into(),
            config_path: "cfg".into(),
            family: DeviceFamily::AccessPoint,
            firmware,
            burn_in_hours: None,
            port_testing: false,
            capwap,
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
        assert!(port_walk_available(Phase::WaitBurnIn, options.family));
        assert!(!port_walk_available(Phase::ApplyConfig, options.family));
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

    #[test]
    fn access_point_capwap_only_skips_firmware_and_config() {
        let options = access_point_options(None, Some(sample_capwap()));
        let mut phase = start_phase(&options);
        phase = next_phase(phase, &options, PhaseEvent::LoggedIn);
        assert_eq!(phase, Phase::ApplyCapwap);
        phase = next_phase(phase, &options, PhaseEvent::CapwapApplied);
        assert_eq!(phase, Phase::ValidateCapwap);
        phase = next_phase(phase, &options, PhaseEvent::CapwapValidated);
        assert_eq!(phase, Phase::Done);
    }

    #[test]
    fn access_point_firmware_then_capwap_after_version_match() {
        let options = access_point_options(
            Some(FirmwareChoice {
                image_path: "ap.bin".into(),
                local_ethernet: "eth0".into(),
            }),
            Some(sample_capwap()),
        );
        let mut phase = start_phase(&options);
        phase = next_phase(phase, &options, PhaseEvent::LoggedIn);
        assert_eq!(phase, Phase::NameTftpPort);
        for event in [
            PhaseEvent::CableConfirmed,
            PhaseEvent::ImageOnFlash,
            PhaseEvent::Rebooted,
            PhaseEvent::LoggedIn,
            PhaseEvent::VersionMatched,
        ] {
            phase = next_phase(phase, &options, event);
        }
        assert_eq!(phase, Phase::ApplyCapwap);
        phase = next_phase(phase, &options, PhaseEvent::CapwapApplied);
        assert_eq!(phase, Phase::ValidateCapwap);
        phase = next_phase(phase, &options, PhaseEvent::CapwapValidated);
        assert_eq!(phase, Phase::Done);
    }

    #[test]
    fn access_point_firmware_without_capwap_done_after_version() {
        let options = access_point_options(
            Some(FirmwareChoice {
                image_path: "ap.bin".into(),
                local_ethernet: "eth0".into(),
            }),
            None,
        );
        let mut phase = next_phase(start_phase(&options), &options, PhaseEvent::LoggedIn);
        assert_eq!(phase, Phase::NameTftpPort);
        for event in [
            PhaseEvent::CableConfirmed,
            PhaseEvent::ImageOnFlash,
            PhaseEvent::Rebooted,
            PhaseEvent::LoggedIn,
            PhaseEvent::VersionMatched,
        ] {
            phase = next_phase(phase, &options, event);
        }
        assert_eq!(phase, Phase::Done);
    }

    #[test]
    fn access_point_without_firmware_or_capwap_stops_at_login() {
        let options = access_point_options(None, None);
        assert_eq!(
            next_phase(Phase::Login, &options, PhaseEvent::LoggedIn),
            Phase::Stopped
        );
    }

    #[test]
    fn capwap_validation_failure_stops() {
        let options = access_point_options(None, Some(sample_capwap()));
        assert_eq!(
            next_phase(
                Phase::ValidateCapwap,
                &options,
                PhaseEvent::CapwapFailed
            ),
            Phase::Stopped
        );
    }

    #[test]
    fn port_walk_unavailable_for_access_point_even_at_done() {
        let options = access_point_options(None, Some(sample_capwap()));
        assert!(!port_walk_available(Phase::Done, options.family));
        assert!(!port_walk_available(Phase::CleanReboot, options.family));
    }
}
