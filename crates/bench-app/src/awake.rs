//! Keeps the computer awake through a run that can last many hours.

use std::io;
use std::marker::PhantomData;

/// Keeps the computer from sleeping until dropped. Windows ties the request
/// to the thread that made it, so this stays on that thread.
pub struct KeepAwake {
    #[cfg(target_os = "macos")]
    caffeinate: std::process::Child,
    _thread: PhantomData<*const ()>,
}

impl KeepAwake {
    /// `-w` also ends `caffeinate` when this program exits without dropping it.
    #[cfg(target_os = "macos")]
    pub fn new() -> io::Result<Self> {
        let caffeinate = std::process::Command::new("caffeinate")
            .args(["-dims", "-w", &std::process::id().to_string()])
            .spawn()?;
        Ok(Self {
            caffeinate,
            _thread: PhantomData,
        })
    }

    #[cfg(windows)]
    pub fn new() -> io::Result<Self> {
        use windows_sys::Win32::System::Power::{
            SetThreadExecutionState, ES_CONTINUOUS, ES_SYSTEM_REQUIRED,
        };
        // SAFETY: the call only sets this thread's execution state flags.
        if unsafe { SetThreadExecutionState(ES_CONTINUOUS | ES_SYSTEM_REQUIRED) } == 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(Self {
            _thread: PhantomData,
        })
    }
}

impl Drop for KeepAwake {
    fn drop(&mut self) {
        #[cfg(target_os = "macos")]
        {
            let _ = self.caffeinate.kill();
            let _ = self.caffeinate.wait();
        }
        #[cfg(windows)]
        {
            use windows_sys::Win32::System::Power::{SetThreadExecutionState, ES_CONTINUOUS};
            // SAFETY: the call only clears this thread's execution state flags.
            unsafe { SetThreadExecutionState(ES_CONTINUOUS) };
        }
    }
}

#[cfg(all(test, target_os = "macos"))]
mod tests {
    use super::*;

    #[test]
    fn dropping_ends_caffeinate() {
        let mut awake = KeepAwake::new().unwrap();
        assert!(awake.caffeinate.try_wait().unwrap().is_none());
        let pid = awake.caffeinate.id().to_string();
        drop(awake);
        let alive = std::process::Command::new("kill")
            .args(["-0", &pid])
            .output()
            .unwrap();
        assert!(!alive.status.success(), "caffeinate {pid} is still running");
    }
}
