//! Linux mountinfo parsing, filtering, and native capacity collection.

use std::collections::HashMap;
use std::path::Path;

use crate::collector::drives::{normalize, DriveCandidate};
use crate::collector::error::CollectError;
use crate::collector::linux::source::{ProcSource, RawStatvfs};

#[derive(Debug, Clone, PartialEq, Eq)]
struct MountRecord {
    device: String,
    root: String,
    mount_point: String,
    filesystem_type: String,
    source: String,
}

const EXCLUDED_FILESYSTEMS: &[&str] = &[
    "proc",
    "sysfs",
    "devpts",
    "cgroup",
    "cgroup2",
    "securityfs",
    "debugfs",
    "tracefs",
    "configfs",
    "pstore",
    "efivarfs",
    "mqueue",
    "hugetlbfs",
    "bpf",
    "fusectl",
    "tmpfs",
    "devtmpfs",
    "ramfs",
    "overlay",
    "nfs",
    "nfs4",
    "cifs",
    "smb3",
    "sshfs",
    "fuse.sshfs",
    "9p",
    "ceph",
    "glusterfs",
    "afs",
];

fn decode_mountinfo(value: &str) -> Option<String> {
    let mut result = String::with_capacity(value.len());
    let mut chars = value.chars();
    while let Some(ch) = chars.next() {
        if ch != '\\' {
            result.push(ch);
            continue;
        }
        let code: String = chars.by_ref().take(3).collect();
        let decoded = match code.as_str() {
            "040" => ' ',
            "011" => '\t',
            "012" => '\n',
            "134" => '\\',
            _ if code.len() < 3 => {
                // A truncated escape sequence at end of input must not
                // discard the whole mount entry; keep the raw characters
                // so the drive still appears in metrics.
                tracing::warn!(
                    escape = ?code,
                    "truncated mountinfo octal escape; keeping raw characters"
                );
                result.push('\\');
                result.push_str(&code);
                continue;
            }
            _ => {
                tracing::warn!(
                    escape = ?code,
                    "unrecognized mountinfo octal escape; skipping mount entry"
                );
                return None;
            }
        };
        result.push(decoded);
    }
    Some(result)
}

fn parse_mountinfo_line(line: &str) -> Option<MountRecord> {
    let (left, right) = line.split_once(" - ")?;
    let left: Vec<_> = left.split_whitespace().collect();
    let right: Vec<_> = right.split_whitespace().collect();
    if left.len() < 6 || right.len() < 2 {
        return None;
    }
    let device = left[2].to_string();
    let root = decode_mountinfo(left[3])?;
    let mount_point = decode_mountinfo(left[4])?;
    let filesystem_type = right[0].to_string();
    let source = decode_mountinfo(right[1])?;
    Some(MountRecord {
        device,
        root,
        mount_point,
        filesystem_type,
        source,
    })
}

fn eligible(record: &MountRecord) -> bool {
    !EXCLUDED_FILESYSTEMS.contains(&record.filesystem_type.as_str())
        && !record.mount_point.is_empty()
        && !record.mount_point.starts_with("/proc/")
}

fn preferred(left: &MountRecord, right: &MountRecord) -> bool {
    let left_rank = (left.mount_point != "/", left.root != "/");
    let right_rank = (right.mount_point != "/", right.root != "/");
    left_rank < right_rank
        || (left_rank == right_rank
            && (left.mount_point.len(), left.mount_point.as_str())
                < (right.mount_point.len(), right.mount_point.as_str()))
}

fn identity(record: &MountRecord) -> String {
    if record.device != "0:0" {
        return record.device.clone();
    }
    format!(
        "synthetic:{}:{}:{}:{}",
        record.filesystem_type, record.source, record.root, record.mount_point
    )
}

fn capacity(stats: RawStatvfs) -> Option<(u64, u64, u64)> {
    let unit = (stats.fragment_size != 0)
        .then_some(stats.fragment_size)
        .or_else(|| (stats.block_size != 0).then_some(stats.block_size))?;
    let total = stats.blocks.checked_mul(unit)?;
    let free = stats.free_blocks.checked_mul(unit)?;
    let available = stats.available_blocks.checked_mul(unit)?;
    (total > 0 && free <= total && available <= total).then_some((total, free, available))
}

pub(crate) fn collect(
    source: &ProcSource,
) -> Result<Vec<gregg_protocol::v2::DriveMetrics>, CollectError> {
    let raw = source.read_mountinfo()?;
    let mut selected: HashMap<String, MountRecord> = HashMap::new();
    for line in raw.lines() {
        let Some(record) = parse_mountinfo_line(line) else {
            continue;
        };
        if !eligible(&record) {
            continue;
        }
        let key = identity(&record);
        match selected.get(&key) {
            Some(current) if !preferred(&record, current) => {}
            _ => {
                selected.insert(key, record);
            }
        }
    }

    let mut candidates = Vec::new();
    for (key, record) in selected {
        let path = Path::new(&record.mount_point);
        let Ok(stats) = source.statvfs(path) else {
            continue;
        };
        let Some((total, free, available)) = capacity(stats) else {
            tracing::warn!(
                mount_point = %record.mount_point,
                "statvfs reported zero block size or overflowing capacity; skipping drive"
            );
            continue;
        };
        candidates.push(DriveCandidate {
            identity: key,
            name: record.mount_point,
            total_bytes: total,
            total_free_bytes: free,
            available_bytes: available,
        });
    }
    Ok(normalize(candidates))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::collector::linux::source::{MemorySource, RawStatvfs};
    use std::sync::Arc;

    const MOUNTS: &str = "42 1 8:1 / / rw - ext4 /dev/sda1 rw\n43 1 8:1 / /mnt/root rw - ext4 /dev/sda1 rw\n44 1 8:2 /home /home rw - xfs /dev/sda2 rw\n45 1 0:0 / /tmp/foo\\040bar rw - tmpfs tmpfs rw\n46 1 8:3 / /net rw - nfs server:/x rw\n";

    #[test]
    fn parses_filters_deduplicates_and_sorts() {
        let source = MemorySource::new()
            .with_file("/proc/self/mountinfo", MOUNTS)
            .with_logical_cores(2);
        let mut source = ProcSource::for_source(Arc::new(source));
        source.memory_source_mut().unwrap().add_statvfs(
            "/",
            RawStatvfs {
                blocks: 10,
                free_blocks: 3,
                available_blocks: 3,
                fragment_size: 100,
                block_size: 100,
            },
        );
        source.memory_source_mut().unwrap().add_statvfs(
            "/home",
            RawStatvfs {
                blocks: 20,
                free_blocks: 4,
                available_blocks: 4,
                fragment_size: 100,
                block_size: 100,
            },
        );
        let drives = collect(&source).unwrap();
        assert_eq!(
            drives
                .iter()
                .map(|drive| drive.name.as_str())
                .collect::<Vec<_>>(),
            ["/", "/home"]
        );
        assert_eq!(drives[0].used_bytes, 700);
    }

    #[test]
    fn malformed_lines_are_skipped_and_capacity_is_checked() {
        let source = MemorySource::new()
            .with_file(
                "/proc/self/mountinfo",
                "bad\n1 1 8:1 / / rw - ext4 /dev/sda rw\n",
            )
            .with_logical_cores(1);
        let mut source = ProcSource::for_source(Arc::new(source));
        source.memory_source_mut().unwrap().add_statvfs(
            "/",
            RawStatvfs {
                blocks: u64::MAX,
                free_blocks: 0,
                available_blocks: 0,
                fragment_size: 2,
                block_size: 1,
            },
        );
        assert!(collect(&source).unwrap().is_empty());
    }

    #[test]
    fn truncated_escape_keeps_raw_characters_instead_of_dropping_entry() {
        // A truncated escape at end of input must survive as literal
        // characters so the mount entry is not silently discarded.
        assert_eq!(decode_mountinfo("/mnt\\04"), Some(String::from("/mnt\\04")));
        assert_eq!(decode_mountinfo("/tmp/x\\"), Some(String::from("/tmp/x\\")));
    }

    #[test]
    fn preserves_total_free_and_caller_available_separately() {
        let source = MemorySource::new()
            .with_file("/proc/self/mountinfo", MOUNTS)
            .with_logical_cores(1);
        let mut source = ProcSource::for_source(Arc::new(source));
        source.memory_source_mut().unwrap().add_statvfs(
            "/",
            RawStatvfs {
                blocks: 100,
                free_blocks: 40,
                available_blocks: 25,
                fragment_size: 1,
                block_size: 1,
            },
        );

        let drive = collect(&source).unwrap().into_iter().next().unwrap();
        assert_eq!(drive.used_bytes, 60);
        assert_eq!(drive.total_bytes, 100);
        assert_eq!(drive.available_bytes, Some(25));
    }
}
