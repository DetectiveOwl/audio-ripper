//! Registry of the child processes that are currently running.
//!
//! Windows has Job Objects (see `jobobject`), which terminate every child when
//! the app exits. macOS and Linux have no equivalent, so on those systems each
//! child is started as the leader of its own process group, and the app kills
//! the registered groups when it exits or when a job is cancelled.
//!
//! This covers a normal exit (window closed, quit, Cmd+Q). If the app itself is
//! killed with SIGKILL nothing can run, but yt-dlp and spotDL stop on their own
//! at the next write to the closed pipe.

use std::sync::Mutex;

static CHILDREN: Mutex<Vec<u32>> = Mutex::new(Vec::new());

/// Remembers a running child (its pid is also its process group id).
pub fn register(pid: u32) {
    if let Ok(mut list) = CHILDREN.lock() {
        list.push(pid);
    }
}

/// Forgets a child that has finished.
pub fn unregister(pid: u32) {
    if let Ok(mut list) = CHILDREN.lock() {
        list.retain(|p| *p != pid);
    }
}

/// Kills the process group led by `pid`: the child and everything it started.
#[cfg(unix)]
pub fn kill_group(pid: u32) {
    // SAFETY: plain syscall; a stale or invalid pid just makes it fail with ESRCH.
    unsafe {
        libc::killpg(pid as libc::pid_t, libc::SIGKILL);
    }
}

#[cfg(not(unix))]
pub fn kill_group(_pid: u32) {}

/// Kills every registered child. Called when the app exits.
pub fn kill_all() {
    let pids: Vec<u32> = match CHILDREN.lock() {
        Ok(mut list) => std::mem::take(&mut *list),
        Err(_) => return,
    };
    for pid in pids {
        kill_group(pid);
    }
}

/// Registers a pid and unregisters it when dropped.
pub(crate) struct Registered(pub u32);

impl Registered {
    pub fn new(pid: u32) -> Self {
        register(pid);
        Registered(pid)
    }
}

impl Drop for Registered {
    fn drop(&mut self) {
        unregister(self.0);
    }
}
