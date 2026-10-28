#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SerialSettings {
    pub baud: u32,
    pub data_bits: u8,
    pub parity: &'static str,
    pub stop_bits: u8,
}

pub fn cisco_console() -> SerialSettings {
    SerialSettings {
        baud: 9600,
        data_bits: 8,
        parity: "none",
        stop_bits: 1,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn console_settings_are_9600_8n1() {
        let settings = cisco_console();
        assert_eq!(
            (
                settings.baud,
                settings.data_bits,
                settings.parity,
                settings.stop_bits
            ),
            (9600, 8, "none", 1)
        );
    }
}
