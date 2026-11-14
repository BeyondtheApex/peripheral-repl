use std::{io, process::ExitStatus};

pub(super) enum Child {
    #[cfg(any(not(windows), test))]
    Standard(std::process::Child),
    #[cfg(windows)]
    Native(super::windows_worker::Process),
}
impl Child {
    pub fn kill(&mut self) -> io::Result<()> {
        match self {
            #[cfg(any(not(windows), test))]
            Self::Standard(c) => c.kill(),
            #[cfg(windows)]
            Self::Native(c) => c.kill(),
        }
    }
    pub fn wait(&mut self) -> io::Result<ExitStatus> {
        match self {
            #[cfg(any(not(windows), test))]
            Self::Standard(c) => c.wait(),
            #[cfg(windows)]
            Self::Native(c) => c.wait(),
        }
    }
    #[cfg(test)]
    pub fn try_wait(&mut self) -> io::Result<Option<ExitStatus>> {
        match self {
            Self::Standard(c) => c.try_wait(),
            #[cfg(windows)]
            Self::Native(c) => c.try_wait(),
        }
    }
    #[cfg(test)]
    pub fn id(&self) -> u32 {
        match self {
            Self::Standard(c) => c.id(),
            #[cfg(windows)]
            Self::Native(c) => c.id(),
        }
    }
}
