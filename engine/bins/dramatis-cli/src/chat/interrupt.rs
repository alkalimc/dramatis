//! Ctrl-C, heard from startup to exit.
//!
//! `tokio::signal::ctrl_c()` installs its handler on the first poll, so waiting on it only
//! inside a command leaves every other moment to the default action (the process dies and
//! a `--fresh` home stays behind) or, once installed, to a silent drop. One task started
//! at launch owns the signal instead. The first Ctrl-C cancels the turn in flight and wakes
//! whoever waits for it; one that nothing took (work that cannot be interrupted) makes the
//! next one leave the process, after the temporary home is removed. At the line editor the
//! terminal is in raw mode and Ctrl-C arrives as a key, not as this signal.

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, OnceLock};

use runtime::AgentBridge;
use tokio::sync::Notify;

/// What a signal means right now.
#[derive(Debug, PartialEq, Eq)]
pub enum Hit {
    /// Cancel the turn in flight.
    Cancel,
    /// The previous Ctrl-C was not taken: leave.
    Exit,
}

#[derive(Default)]
pub struct Interrupt {
    pending: AtomicBool,
    wake: Notify,
    bridge: OnceLock<Arc<AgentBridge>>,
}

impl Interrupt {
    /// Take over Ctrl-C for the life of the process. The handler is installed before this
    /// returns. `home` is a temporary home to remove on the way out.
    #[cfg(unix)]
    pub fn install(home: Option<PathBuf>) -> std::io::Result<Arc<Self>> {
        use tokio::signal::unix::{SignalKind, signal};
        let mut sigint = signal(SignalKind::interrupt())?;
        let this = Arc::new(Self::default());
        let task = this.clone();
        tokio::spawn(async move {
            while sigint.recv().await.is_some() {
                task.on_signal(home.as_deref());
            }
        });
        Ok(this)
    }

    /// As on Unix, with the console's Ctrl-C event as the signal.
    #[cfg(windows)]
    pub fn install(home: Option<PathBuf>) -> std::io::Result<Arc<Self>> {
        let mut sigint = tokio::signal::windows::ctrl_c()?;
        let this = Arc::new(Self::default());
        let task = this.clone();
        tokio::spawn(async move {
            while sigint.recv().await.is_some() {
                task.on_signal(home.as_deref());
            }
        });
        Ok(this)
    }

    /// One Ctrl-C: cancel the turn in flight, or leave if the last one was not taken.
    #[cfg(any(unix, windows))]
    fn on_signal(&self, home: Option<&std::path::Path>) {
        if self.hit() == Hit::Exit {
            if let Some(dir) = home {
                let _ = std::fs::remove_dir_all(dir);
            }
            println!();
            std::process::exit(130);
        }
        if let Some(bridge) = self.bridge.get() {
            bridge.cancel();
        }
    }

    /// Without a signal to own, the default action stays.
    #[cfg(not(any(unix, windows)))]
    pub fn install(_: Option<PathBuf>) -> std::io::Result<Arc<Self>> {
        Ok(Arc::new(Self::default()))
    }

    /// Let a Ctrl-C cancel this bridge's work.
    pub fn attach(&self, bridge: Arc<AgentBridge>) {
        let _ = self.bridge.set(bridge);
    }

    /// Record a Ctrl-C and wake the waiter.
    pub fn hit(&self) -> Hit {
        if self.pending.swap(true, Ordering::SeqCst) {
            return Hit::Exit;
        }
        self.wake.notify_one();
        Hit::Cancel
    }

    /// Forget a Ctrl-C nobody took: called where a new wait begins.
    pub fn clear(&self) {
        self.pending.store(false, Ordering::SeqCst);
    }

    /// Resolves at the next Ctrl-C, and takes it.
    pub async fn wait(&self) {
        loop {
            let woken = self.wake.notified();
            if self.pending.swap(false, Ordering::SeqCst) {
                return;
            }
            woken.await;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn a_taken_ctrl_c_is_forgotten_and_an_untaken_one_makes_the_next_exit() {
        let i = Interrupt::default();
        assert_eq!(i.hit(), Hit::Cancel);
        i.wait().await;
        assert_eq!(i.hit(), Hit::Cancel);
        assert_eq!(i.hit(), Hit::Exit);
        i.clear();
        assert_eq!(i.hit(), Hit::Cancel);
    }

    #[tokio::test]
    async fn a_wait_started_after_the_signal_still_hears_it() {
        let i = Interrupt::default();
        i.hit();
        tokio::time::timeout(std::time::Duration::from_secs(1), i.wait())
            .await
            .expect("the earlier Ctrl-C is delivered");
    }

    #[tokio::test]
    async fn a_waiter_wakes_on_a_later_signal() {
        let i = Arc::new(Interrupt::default());
        let waiter = tokio::spawn({
            let i = i.clone();
            async move { i.wait().await }
        });
        tokio::task::yield_now().await;
        i.hit();
        tokio::time::timeout(std::time::Duration::from_secs(1), waiter)
            .await
            .expect("woken")
            .unwrap();
    }
}
