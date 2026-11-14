//! Preserve the backend's privilege mode across an application update.
//! Elevated recovery still goes through the normal UAC flow from the GUI.
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Resume {
    #[default]
    Stopped,
    Normal,
    Elevated,
}

impl Resume {
    pub fn from_update_argument(value: Option<&str>) -> anyhow::Result<Self> {
        match value {
            None | Some("no-backend") => Ok(Self::Stopped),
            Some("resume-backend") => Ok(Self::Normal),
            Some("resume-backend-elevated") => Ok(Self::Elevated),
            Some(value) => anyhow::bail!("Unknown backend resume mode: {value}"),
        }
    }

    pub fn update_argument(self) -> &'static str {
        match self {
            Self::Stopped => "no-backend",
            Self::Normal => "resume-backend",
            Self::Elevated => "resume-backend-elevated",
        }
    }

    pub fn ui_argument(self) -> Option<&'static str> {
        match self {
            Self::Stopped => None,
            Self::Normal => Some("--resume-backend"),
            Self::Elevated => Some("--resume-backend-elevated"),
        }
    }

    pub fn action(self) -> Option<&'static str> {
        match self {
            Self::Stopped => None,
            Self::Normal => Some("start_normal"),
            Self::Elevated => Some("elevate"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn updates_preserve_privileges_and_do_not_start_stopped_backends() {
        for (mode, argument, action) in [
            (Resume::Stopped, None, None),
            (
                Resume::Normal,
                Some("--resume-backend"),
                Some("start_normal"),
            ),
            (
                Resume::Elevated,
                Some("--resume-backend-elevated"),
                Some("elevate"),
            ),
        ] {
            let restored = Resume::from_update_argument(Some(mode.update_argument())).unwrap();
            let wire = serde_json::to_value(restored).unwrap();
            let restored: Resume = serde_json::from_value(wire).unwrap();
            assert_eq!(restored, mode);
            assert_eq!(restored.ui_argument(), argument);
            assert_eq!(restored.action(), action);
        }
        assert_eq!(Resume::from_update_argument(None).unwrap(), Resume::Stopped);
        assert!(Resume::from_update_argument(Some("elevated?")).is_err());
    }
}
