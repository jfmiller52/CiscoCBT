#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum DeviceFamily {
    #[default]
    IosXe,
    Nxos,
    AccessPoint,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CapwapChoice {
    pub ip: String,
    pub mask: String,
    pub gateway: String,
}
