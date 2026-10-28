use crate::{validate_burn_in_hours, FirmwareChoice, RunOptions};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StartForm {
    pub serial_device: String,
    pub config_path: String,
    pub firmware_on: bool,
    pub image_path: String,
    pub local_ethernet: String,
    pub burn_in_on: bool,
    pub hours: u32,
    pub port_testing: bool,
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
            firmware_on: false,
            image_path: String::new(),
            local_ethernet: String::new(),
            burn_in_on: false,
            hours: 1,
            port_testing: false,
        }
    }

    pub fn step_hours(&mut self, delta: i32) {
        self.hours = self.hours.saturating_add_signed(delta).max(1);
    }

    pub fn to_options(&self) -> Result<RunOptions, &'static str> {
        if field_empty(&self.serial_device) {
            return Err("Choose the serial device for the console cable.");
        }
        if field_empty(&self.config_path) {
            return Err("Choose the configuration file.");
        }
        let firmware = if self.firmware_on {
            if field_empty(&self.image_path) {
                return Err("Choose the firmware image file.");
            }
            if field_empty(&self.local_ethernet) {
                return Err("Choose the ethernet port that will serve the image.");
            }
            Some(FirmwareChoice {
                image_path: self.image_path.clone(),
                local_ethernet: self.local_ethernet.clone(),
            })
        } else {
            None
        };
        let burn_in_hours = if self.burn_in_on {
            validate_burn_in_hours(self.hours).map_err(|_| "Burn-in needs at least 1 hour.")?;
            Some(self.hours)
        } else {
            None
        };
        Ok(RunOptions {
            serial_device: self.serial_device.clone(),
            config_path: self.config_path.clone(),
            firmware,
            burn_in_hours,
            port_testing: self.port_testing,
        })
    }
}

fn field_empty(value: &str) -> bool {
    value.trim().is_empty()
}

#[cfg(test)]
mod tests {
    use super::*;

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
}
