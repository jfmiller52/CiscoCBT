use crate::{ready_to_release, CheckStatus, FactorySecrets, RunRecord};

pub fn release_heading(record: &RunRecord) -> &'static str {
    if ready_to_release(record) {
        "Ready to release"
    } else {
        "Not ready to release"
    }
}

pub fn reminder_line(secrets: &FactorySecrets) -> String {
    format!(
        "Username {} · password {} · enable secret {}",
        secrets.username, secrets.password, secrets.enable_secret
    )
}

pub fn status_label(status: &CheckStatus) -> &'static str {
    match status {
        CheckStatus::Passed => "Passed",
        CheckStatus::Failed { .. } => "Failed",
        CheckStatus::NotFinished => "Not finished",
        CheckStatus::NotChosen => "Not chosen",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{CheckStatus, FactorySecrets, RunRecord};

    #[test]
    fn heading_and_reminder_use_the_accepted_secrets() {
        let record = RunRecord {
            configuration: CheckStatus::Passed,
            firmware: CheckStatus::NotChosen,
            burn_in: CheckStatus::NotChosen,
            ports: CheckStatus::NotChosen,
        };
        assert_eq!(release_heading(&record), "Ready to release");
        let secrets = FactorySecrets {
            username: "cisco".into(),
            password: "Passw0rd%".into(),
            enable_secret: "Passw0rd%".into(),
        };
        let line = reminder_line(&secrets);
        assert!(line.contains("cisco"));
        assert!(line.contains("Passw0rd%"));
        assert_eq!(status_label(&CheckStatus::NotFinished), "Not finished");
    }

    #[test]
    fn unfinished_record_is_not_ready_and_every_status_has_a_label() {
        let record = RunRecord {
            configuration: CheckStatus::Passed,
            firmware: CheckStatus::NotChosen,
            burn_in: CheckStatus::NotFinished,
            ports: CheckStatus::NotChosen,
        };
        assert_eq!(release_heading(&record), "Not ready to release");
        let secrets = FactorySecrets {
            username: "cisco".into(),
            password: "cisco".into(),
            enable_secret: "Passw0rd%".into(),
        };
        assert_eq!(
            reminder_line(&secrets),
            "Username cisco · password cisco · enable secret Passw0rd%"
        );
        assert_eq!(status_label(&CheckStatus::Passed), "Passed");
        assert_eq!(
            status_label(&CheckStatus::Failed { detail: "x".into() }),
            "Failed"
        );
        assert_eq!(status_label(&CheckStatus::NotChosen), "Not chosen");
    }
}
