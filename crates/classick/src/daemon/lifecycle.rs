use tokio::sync::mpsc;

const PARENT_POLL_INTERVAL: std::time::Duration = std::time::Duration::from_millis(250);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ShutdownReason {
    Client,
    Signal,
    ParentDeath,
}

pub fn spawn_shutdown_monitor(parent_pid: Option<u32>) -> mpsc::UnboundedReceiver<ShutdownReason> {
    let (shutdown_tx, shutdown_rx) = mpsc::unbounded_channel();
    tokio::spawn(async move {
        let reason = tokio::select! {
            _ = wait_for_shutdown_signal() => ShutdownReason::Signal,
            _ = wait_for_parent_death(parent_pid) => ShutdownReason::ParentDeath,
            _ = shutdown_tx.closed() => return,
        };
        let _ = shutdown_tx.send(reason);
    });
    shutdown_rx
}

async fn wait_for_parent_death(parent_pid: Option<u32>) {
    let Some(parent_pid) = parent_pid else {
        std::future::pending::<()>().await;
        return;
    };

    loop {
        if !parent_still_matches(parent_pid) {
            return;
        }
        tokio::time::sleep(PARENT_POLL_INTERVAL).await;
    }
}

#[cfg(unix)]
async fn wait_for_shutdown_signal() {
    tokio::select! {
        _ = wait_for_ctrl_c() => {},
        _ = wait_for_sigterm() => {},
    }
}

#[cfg(windows)]
async fn wait_for_shutdown_signal() {
    wait_for_ctrl_c().await;
}

async fn wait_for_ctrl_c() {
    match tokio::signal::ctrl_c().await {
        Ok(()) => {}
        Err(error) => {
            tracing::error!("daemon lifecycle: failed to listen for Ctrl-C: {error}");
            std::future::pending::<()>().await;
        }
    }
}

#[cfg(unix)]
async fn wait_for_sigterm() {
    use tokio::signal::unix::{signal, SignalKind};

    match signal(SignalKind::terminate()) {
        Ok(mut stream) => {
            if stream.recv().await.is_none() {
                tracing::error!("daemon lifecycle: SIGTERM listener closed unexpectedly");
                std::future::pending::<()>().await;
            }
        }
        Err(error) => {
            tracing::error!("daemon lifecycle: failed to listen for SIGTERM: {error}");
            std::future::pending::<()>().await;
        }
    }
}

#[cfg(unix)]
fn current_parent_pid() -> Option<u32> {
    let parent_pid = unsafe { libc::getppid() };
    u32::try_from(parent_pid).ok()
}

#[cfg(unix)]
fn parent_still_matches(expected_parent_pid: u32) -> bool {
    current_parent_pid() == Some(expected_parent_pid)
}

#[cfg(windows)]
fn current_parent_pid() -> Option<u32> {
    windows_process_snapshot().and_then(|(parent_pid, _)| parent_pid)
}

#[cfg(windows)]
fn parent_still_matches(expected_parent_pid: u32) -> bool {
    windows_process_snapshot().is_some_and(|(current_parent_pid, parent_is_alive)| {
        current_parent_pid == Some(expected_parent_pid) && parent_is_alive(expected_parent_pid)
    })
}

#[cfg(windows)]
fn windows_process_snapshot() -> Option<(Option<u32>, impl Fn(u32) -> bool)> {
    use std::collections::HashSet;
    use std::mem::size_of;
    use windows_sys::Win32::Foundation::{CloseHandle, INVALID_HANDLE_VALUE};
    use windows_sys::Win32::System::Diagnostics::ToolHelp::{
        CreateToolhelp32Snapshot, Process32FirstW, Process32NextW, PROCESSENTRY32W,
        TH32CS_SNAPPROCESS,
    };

    let snapshot = unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) };
    if snapshot == INVALID_HANDLE_VALUE {
        tracing::warn!(
            "daemon lifecycle: failed to inspect parent process: {}",
            std::io::Error::last_os_error()
        );
        return None;
    }

    let mut entry: PROCESSENTRY32W = unsafe { std::mem::zeroed() };
    entry.dwSize = size_of::<PROCESSENTRY32W>() as u32;
    let current_pid = std::process::id();
    let mut current_parent_pid = None;
    let mut process_ids = HashSet::new();
    let mut have_entry = unsafe { Process32FirstW(snapshot, &mut entry) } != 0;
    while have_entry {
        process_ids.insert(entry.th32ProcessID);
        if entry.th32ProcessID == current_pid {
            current_parent_pid = Some(entry.th32ParentProcessID);
        }
        have_entry = unsafe { Process32NextW(snapshot, &mut entry) } != 0;
    }
    unsafe { CloseHandle(snapshot) };
    Some((current_parent_pid, move |pid| process_ids.contains(&pid)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn current_parent_keeps_the_lease_alive() {
        let parent_pid = current_parent_pid().expect("supported platform has a parent process");
        let mut shutdown_rx = spawn_shutdown_monitor(Some(parent_pid));

        assert!(
            tokio::time::timeout(std::time::Duration::from_millis(100), shutdown_rx.recv())
                .await
                .is_err(),
            "a live current parent must not end the daemon lease"
        );
    }

    #[tokio::test]
    async fn dead_parent_ends_the_lease() {
        let parent_pid = current_parent_pid().expect("supported platform has a parent process");
        let dead_parent_pid = parent_pid ^ 1;
        let mut shutdown_rx = spawn_shutdown_monitor(Some(dead_parent_pid));

        assert_eq!(
            tokio::time::timeout(std::time::Duration::from_secs(2), shutdown_rx.recv())
                .await
                .expect("parent mismatch is detected")
                .expect("shutdown monitor remains connected"),
            ShutdownReason::ParentDeath
        );
    }

    #[tokio::test]
    async fn missing_parent_pid_disables_the_parent_lease() {
        let mut shutdown_rx = spawn_shutdown_monitor(None);

        assert!(
            tokio::time::timeout(std::time::Duration::from_millis(100), shutdown_rx.recv())
                .await
                .is_err(),
            "manual daemons must not inherit a parent lease"
        );
    }
}
