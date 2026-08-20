#![forbid(unsafe_code)]

use std::fs::{File, OpenOptions, TryLockError};
use std::path::Path;
use std::time::{Duration, Instant};

const RETRY_INTERVAL: Duration = Duration::from_millis(25);
const LOCK_FILE_NAME: &str = "tea-daemon-startup.lock";

pub struct StartupMutex {
    _file: File,
}

impl StartupMutex {
    pub fn acquire(data_dir: &Path, timeout: Duration) -> Result<Self, String> {
        std::fs::create_dir_all(data_dir).map_err(|error| {
            format!(
                "failed to create Tea daemon startup lock directory {}: {error}",
                data_dir.display()
            )
        })?;
        let lock_path = data_dir.join(LOCK_FILE_NAME);
        // Keep the file in place: recreating it could let launchers lock
        // different file objects for the same path.
        let file = OpenOptions::new()
            .create(true)
            .read(true)
            .write(true)
            .truncate(false)
            .open(&lock_path)
            .map_err(|error| {
                format!(
                    "failed to open Tea daemon startup lock {}: {error}",
                    lock_path.display()
                )
            })?;
        let started = Instant::now();
        loop {
            match file.try_lock() {
                Ok(()) => return Ok(Self { _file: file }),
                Err(TryLockError::WouldBlock) if started.elapsed() < timeout => {
                    std::thread::sleep(RETRY_INTERVAL);
                }
                Err(TryLockError::WouldBlock) => {
                    return Err(format!(
                        "timed out waiting for another Tea launcher after {} seconds",
                        timeout.as_secs()
                    ));
                }
                Err(TryLockError::Error(error)) => {
                    return Err(format!(
                        "failed to lock Tea daemon startup lock {}: {error}",
                        lock_path.display()
                    ));
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn startup_mutex_serializes_competing_guards() {
        let data_dir = std::env::temp_dir().join(format!(
            "tea-startup-lock-test-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let guard = StartupMutex::acquire(&data_dir, Duration::from_secs(1)).unwrap();
        assert!(data_dir.join(LOCK_FILE_NAME).is_file());

        let contender_path = data_dir.clone();
        let contender = std::thread::spawn(move || {
            StartupMutex::acquire(&contender_path, Duration::from_millis(100)).is_err()
        });
        assert!(contender.join().unwrap());

        drop(guard);
        assert!(StartupMutex::acquire(&data_dir, Duration::from_secs(1)).is_ok());
        let _ = std::fs::remove_dir_all(data_dir);
    }
}
