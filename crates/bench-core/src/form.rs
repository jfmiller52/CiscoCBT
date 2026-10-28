use crate::{
    validate_burn_in_hours, CapwapChoice, DeviceFamily, FirmwareChoice, RunOptions,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StartForm {
    pub serial_device: String,
    pub config_path: String,
    pub family: DeviceFamily,
    pub firmware_on: bool,
    pub image_path: String,
    pub local_ethernet: String,
    pub burn_in_on: bool,
    pub hours: u32,
    pub port_testing: bool,
    pub capwap_on: bool,
    pub capwap_ip: String,
    pub capwap_mask: String,
    pub capwap_gateway: String,
}

impl Default for StartForm {
    fn default() -> Self {
        Self::new()
    }
}

impl StartForm {
    pub fn new() -> Self {
        Self {
            serial_device: String::new(),
            config_path: String::new(),
            family: DeviceFamily::default(),
            firmware_on: false,
            image_path: String::new(),
            local_ethernet: String::new(),
            burn_in_on: false,
            hours: 1,
            port_testing: false,
            capwap_on: false,
            capwap_ip: String::new(),
            capwap_mask: String::new(),
            capwap_gateway: String::new(),
        }
    }

    pub fn step_hours(&mut self, delta: i32) {
        self.hours = self.hours.saturating_add_signed(delta).max(1);
    }

    pub fn on_family_changed(&mut self) {
        match self.family {
            DeviceFamily::AccessPoint => {
                self.burn_in_on = false;
                self.port_testing = false;
                self.config_path.clear();
            }
            _ => {
                self.capwap_on = false;
                self.capwap_ip.clear();
                self.capwap_mask.clear();
                self.capwap_gateway.clear();
            }
        }
    }

    pub fn to_options(&self) -> Result<RunOptions, &'static str> {
        if field_empty(&self.serial_device) {
            return Err("Choose the serial device for the console cable.");
        }

        match self.family {
            DeviceFamily::IosXe | DeviceFamily::Nxos => self.to_switch_options(),
            DeviceFamily::AccessPoint => self.to_access_point_options(),
        }
    }

    fn to_switch_options(&self) -> Result<RunOptions, &'static str> {
        if field_empty(&self.config_path) {
            return Err("Choose the configuration file.");
        }
        let firmware = self.firmware_choice()?;
        let burn_in_hours = self.burn_in_hours_choice()?;
        Ok(RunOptions {
            serial_device: self.serial_device.clone(),
            config_path: self.config_path.clone(),
            family: self.family,
            firmware,
            burn_in_hours,
            port_testing: self.port_testing,
            capwap: None,
        })
    }

    fn to_access_point_options(&self) -> Result<RunOptions, &'static str> {
        if !self.firmware_on && !self.capwap_on {
            return Err("Choose firmware upgrade and/or CAPWAP addressing.");
        }
        let firmware = self.firmware_choice()?;
        let capwap = if self.capwap_on {
            if field_empty(&self.capwap_ip)
                || field_empty(&self.capwap_mask)
                || field_empty(&self.capwap_gateway)
            {
                return Err("Enter the CAPWAP IP, subnet mask, and default gateway.");
            }
            Some(CapwapChoice {
                ip: self.capwap_ip.clone(),
                mask: self.capwap_mask.clone(),
                gateway: self.capwap_gateway.clone(),
            })
        } else {
            None
        };
        Ok(RunOptions {
            serial_device: self.serial_device.clone(),
            config_path: self.config_path.clone(),
            family: self.family,
            firmware,
            burn_in_hours: None,
            port_testing: false,
            capwap,
        })
    }

    fn firmware_choice(&self) -> Result<Option<FirmwareChoice>, &'static str> {
        if !self.firmware_on {
            return Ok(None);
        }
        if field_empty(&self.image_path) {
            return Err("Choose the firmware image file.");
        }
        if field_empty(&self.local_ethernet) {
            return Err("Choose the ethernet port that will serve the image.");
        }
        Ok(Some(FirmwareChoice {
            image_path: self.image_path.clone(),
            local_ethernet: self.local_ethernet.clone(),
        }))
    }

    fn burn_in_hours_choice(&self) -> Result<Option<u32>, &'static str> {
        if self.burn_in_on {
            validate_burn_in_hours(self.hours).map_err(|_| "Burn-in needs at least 1 hour.")?;
            Ok(Some(self.hours))
        } else {
            Ok(None)
        }
    }
}

fn field_empty(value: &str) -> bool {
    value.trim().is_empty()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ap_form(serial: &str) -> StartForm {
        let mut form = StartForm::new();
        form.family = DeviceFamily::AccessPoint;
        form.serial_device = serial.into();
        form
    }

    #[test]
    fn hours_step_by_one_and_never_below_one() {
        let mut form = StartForm::new();
        form.burn_in_on = true;
        form.step_hours(-1);
        assert_eq!(form.hours, 1);
        form.step_hours(1);
        assert_eq!(form.hours, 2);
    }

    #[test]
    fn firmware_off_omits_the_image() {
        let mut form = StartForm::new();
        form.serial_device = "/dev/tty.usbserial".into();
        form.config_path = "closet-access.cfg".into();
        form.firmware_on = false;
        let options = form.to_options().unwrap();
        assert!(options.firmware.is_none());
        assert!(!options.port_testing);
    }

    #[test]
    fn default_matches_new() {
        assert_eq!(StartForm::default(), StartForm::new());
    }

    #[test]
    fn whitespace_only_fields_are_empty() {
        let mut form = StartForm::new();
        form.serial_device = "  \t  ".into();
        assert_eq!(
            form.to_options(),
            Err("Choose the serial device for the console cable.")
        );
    }

    #[test]
    fn burn_in_zero_hours_is_rejected() {
        let mut form = StartForm::new();
        form.serial_device = "/dev/tty".into();
        form.config_path = "cfg".into();
        form.burn_in_on = true;
        form.hours = 0;
        assert_eq!(form.to_options(), Err("Burn-in needs at least 1 hour."));
    }

    #[test]
    fn access_point_without_firmware_or_capwap_is_rejected() {
        let form = ap_form("/dev/tty");
        assert_eq!(
            form.to_options(),
            Err("Choose firmware upgrade and/or CAPWAP addressing.")
        );
    }

    #[test]
    fn access_point_capwap_only_allows_empty_config() {
        let mut form = ap_form("/dev/tty");
        form.capwap_on = true;
        form.capwap_ip = "10.0.0.1".into();
        form.capwap_mask = "255.255.255.0".into();
        form.capwap_gateway = "10.0.0.254".into();
        let options = form.to_options().unwrap();
        assert!(options.config_path.is_empty());
        assert_eq!(
            options.capwap,
            Some(CapwapChoice {
                ip: "10.0.0.1".into(),
                mask: "255.255.255.0".into(),
                gateway: "10.0.0.254".into(),
            })
        );
        assert!(options.firmware.is_none());
        assert!(options.burn_in_hours.is_none());
        assert!(!options.port_testing);
    }

    #[test]
    fn access_point_firmware_only_succeeds() {
        let mut form = ap_form("/dev/tty");
        form.firmware_on = true;
        form.image_path = "ap.bin".into();
        form.local_ethernet = "en0".into();
        let options = form.to_options().unwrap();
        assert!(options.firmware.is_some());
        assert!(options.capwap.is_none());
    }

    #[test]
    fn ios_xe_still_requires_config() {
        let mut form = StartForm::new();
        form.family = DeviceFamily::IosXe;
        form.serial_device = "/dev/tty".into();
        assert_eq!(form.to_options(), Err("Choose the configuration file."));
    }

    #[test]
    fn on_family_changed_to_access_point_clears_switch_fields() {
        let mut form = StartForm::new();
        form.config_path = "switch.cfg".into();
        form.burn_in_on = true;
        form.port_testing = true;
        form.capwap_on = true;
        form.capwap_ip = "10.0.0.1".into();
        form.family = DeviceFamily::AccessPoint;
        form.on_family_changed();
        assert!(!form.burn_in_on);
        assert!(!form.port_testing);
        assert!(form.config_path.is_empty());
        assert!(form.capwap_on);
        assert_eq!(form.capwap_ip, "10.0.0.1");
    }

    #[test]
    fn on_family_changed_away_from_access_point_clears_capwap() {
        let mut form = ap_form("/dev/tty");
        form.capwap_on = true;
        form.capwap_ip = "10.0.0.1".into();
        form.capwap_mask = "255.255.255.0".into();
        form.capwap_gateway = "10.0.0.254".into();
        form.family = DeviceFamily::IosXe;
        form.on_family_changed();
        assert!(!form.capwap_on);
        assert!(form.capwap_ip.is_empty());
        assert!(form.capwap_mask.is_empty());
        assert!(form.capwap_gateway.is_empty());
    }
}
