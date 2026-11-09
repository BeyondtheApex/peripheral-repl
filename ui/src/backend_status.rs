//! Shared presentation state for the application tray and sidebar.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum BackendStatus {
    Running,
    #[default]
    Disconnected,
    Stopped,
    Starting,
    Stopping,
    Restarting,
    Error,
}
impl BackendStatus {
    pub fn color(self) -> u32 {
        match self {
            Self::Running => 0x36cc8b,
            Self::Stopped | Self::Disconnected => 0x8793a7,
            Self::Starting | Self::Stopping | Self::Restarting => 0xe7b85d,
            Self::Error => 0xf08075,
        }
    }
    pub fn label(self) -> &'static str {
        crate::i18n::text(match self {
            Self::Running => "后台运行中",
            Self::Stopped => "后台已停止",
            Self::Disconnected => "后台未连接",
            Self::Starting => "后台启动中",
            Self::Stopping => "后台停止中",
            Self::Restarting => "后台重启中",
            Self::Error => "后台异常",
        })
    }
    pub fn busy(self) -> bool {
        matches!(self, Self::Starting | Self::Stopping | Self::Restarting)
    }
}
