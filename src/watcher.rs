use inotify::{EventMask, Inotify, WatchMask};
use rustix::fs::{CWD, FileType, Mode, makedev, mknodat};
use std::collections::HashSet;
use std::io;
use std::os::fd::{AsFd, BorrowedFd};
use std::path::{Path, PathBuf};
use tracing::{debug, info, warn};

pub struct Watcher {
    inotify: Inotify,
    input_dir: PathBuf,
    buf: Vec<u8>,
    /// Device names present in /sys/class/input/ at startup.
    /// These belong to the host or other containers — we skip them.
    baseline: HashSet<String>,
    /// If set, only accept devices whose sysfs phys field contains this string.
    phys_filter: Option<String>,
}

#[derive(Debug)]
pub enum WatchEvent {
    DeviceAdded(PathBuf),
    DeviceRemoved(PathBuf),
}

impl Watcher {
    pub fn new(input_dir: &Path, phys_filter: Option<&str>) -> io::Result<Self> {
        let inotify = Inotify::init()?;
        inotify
            .watches()
            .add(input_dir, WatchMask::CREATE | WatchMask::DELETE)?;

        // Snapshot current sysfs devices as baseline (host/other containers)
        let baseline = Self::snapshot_sysfs();
        if let Some(filter) = &phys_filter {
            info!(
                ?input_dir,
                baseline_count = baseline.len(),
                phys_filter = filter,
                "watching for input device changes (phys filter active)"
            );
        } else {
            info!(
                ?input_dir,
                baseline_count = baseline.len(),
                "watching for input device changes"
            );
        }

        Ok(Self {
            inotify,
            input_dir: input_dir.to_path_buf(),
            buf: vec![0u8; 4096],
            baseline,
            phys_filter: phys_filter.map(String::from),
        })
    }

    /// Read the phys field from /sys/class/input/eventN/device/phys.
    fn read_phys(event_name: &str) -> Option<String> {
        let path = format!("/sys/class/input/{}/device/phys", event_name);
        std::fs::read_to_string(path)
            .ok()
            .map(|s| s.trim().to_string())
    }

    /// Check if a device passes the phys filter.
    /// Retries briefly because sysfs phys file may not be populated yet when inotify fires.
    fn passes_phys_filter(&self, event_name: &str) -> bool {
        let filter = match &self.phys_filter {
            Some(f) => f,
            None => return true,
        };
        for attempt in 0..10 {
            if attempt > 0 {
                std::thread::sleep(std::time::Duration::from_millis(50));
            }
            match Self::read_phys(event_name) {
                Some(phys) if phys.contains(filter.as_str()) => return true,
                Some(phys) => {
                    debug!(
                        event_name,
                        phys, filter, "skipping device — phys doesn't match filter"
                    );
                    return false;
                }
                None if attempt < 9 => continue,
                None => {
                    debug!(
                        event_name,
                        filter, "skipping device — no phys field after retries"
                    );
                    return false;
                }
            }
        }
        false
    }

    /// Snapshot all event device names from /sys/class/input/.
    fn snapshot_sysfs() -> HashSet<String> {
        let sys_dir = Path::new("/sys/class/input");
        std::fs::read_dir(sys_dir)
            .into_iter()
            .flatten()
            .filter_map(|entry| entry.ok())
            .filter_map(|entry| {
                let name = entry.file_name().into_string().ok()?;
                if name.starts_with("event") {
                    Some(name)
                } else {
                    None
                }
            })
            .collect()
    }

    pub fn scan_existing(&self) -> Vec<PathBuf> {
        let mut paths: Vec<PathBuf> = std::fs::read_dir(&self.input_dir)
            .into_iter()
            .flatten()
            .filter_map(|entry| entry.ok())
            .filter(|entry| {
                entry.file_name().to_str().is_some_and(|name| {
                    name.starts_with("event") && self.passes_phys_filter(name)
                })
            })
            .map(|entry| entry.path())
            .collect();
        paths.sort();
        for path in &paths {
            debug!(?path, "found existing input device");
        }
        paths
    }

    /// Scan /sys/class/input/ for NEW event devices (not in baseline) and create
    /// missing device nodes via mknod. Returns paths of newly created nodes.
    pub fn ensure_device_nodes(&self) -> Vec<PathBuf> {
        let mut created = Vec::new();
        let sys_dir = Path::new("/sys/class/input");
        let entries = match std::fs::read_dir(sys_dir) {
            Ok(entries) => entries,
            Err(e) => {
                warn!(err = %e, "cannot read /sys/class/input");
                return created;
            }
        };

        for entry in entries.flatten() {
            let name = match entry.file_name().into_string() {
                Ok(n) if n.starts_with("event") => n,
                _ => continue,
            };

            // Skip devices that existed at startup (host/other containers)
            if self.baseline.contains(&name) {
                continue;
            }

            // Skip devices that don't match phys filter (other containers)
            if !self.passes_phys_filter(&name) {
                continue;
            }

            let dev_path = self.input_dir.join(&name);
            if dev_path.exists() {
                continue;
            }

            // Read major:minor from /sys/class/input/eventN/dev
            let dev_file = sys_dir.join(&name).join("dev");
            let dev_str = match std::fs::read_to_string(&dev_file) {
                Ok(s) => s,
                Err(_) => continue,
            };
            let dev_str = dev_str.trim();
            let parts: Vec<&str> = dev_str.split(':').collect();
            if parts.len() != 2 {
                continue;
            }
            let major: u32 = match parts[0].parse() {
                Ok(v) => v,
                Err(_) => continue,
            };
            let minor: u32 = match parts[1].parse() {
                Ok(v) => v,
                Err(_) => continue,
            };

            let mode = Mode::from_raw_mode(0o660);
            let dev = makedev(major, minor);
            match mknodat(CWD, &dev_path, FileType::CharacterDevice, mode, dev) {
                Ok(()) => {
                    info!(path = %dev_path.display(), major, minor, "created device node");
                    created.push(dev_path);
                }
                Err(e) => {
                    warn!(path = %dev_path.display(), err = %e, "mknod failed");
                }
            }
        }

        created
    }

    pub fn read_events(&mut self) -> io::Result<Vec<WatchEvent>> {
        let events = match self.inotify.read_events(&mut self.buf) {
            Ok(events) => events,
            Err(e) if e.kind() == io::ErrorKind::WouldBlock => return Ok(Vec::new()),
            Err(e) => return Err(e),
        };

        let mut result = Vec::new();
        for event in events {
            let name = match event.name.and_then(|n| n.to_str()) {
                Some(name) if name.starts_with("event") => name,
                _ => continue,
            };

            let path = self.input_dir.join(name);

            if event.mask.contains(EventMask::CREATE) {
                info!(?path, "input device added");
                result.push(WatchEvent::DeviceAdded(path));
            } else if event.mask.contains(EventMask::DELETE) {
                info!(?path, "input device removed");
                result.push(WatchEvent::DeviceRemoved(path));
            }
        }

        Ok(result)
    }
}

impl AsFd for Watcher {
    fn as_fd(&self) -> BorrowedFd<'_> {
        self.inotify.as_fd()
    }
}
