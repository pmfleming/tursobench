//! Startup memory snapshot and database admission policy. No engine allocation cap.
use crate::storage::Preferences;

pub const MIB: u64 = 1024 * 1024;
pub const DATABASE_RESERVATION_MIB: u64 = 512;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct MemorySnapshot {
    pub total_mib: Option<u64>,
    pub available_mib: Option<u64>,
    pub process_limited: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Limits {
    pub budget_mib: u64,
    pub memory_limit: usize,
    pub database_limit: usize,
}

impl MemorySnapshot {
    pub fn detect() -> Self {
        let mut snapshot = std::fs::read_to_string("/proc/meminfo")
            .ok()
            .map(|text| Self::parse(&text))
            .unwrap_or_default();
        // Walk the process cgroup and its ancestors: a parent's limit also
        // applies when the leaf says "max". No polling or I/O in rendering.
        let root = std::path::Path::new("/sys/fs/cgroup");
        let group = std::fs::read_to_string("/proc/self/cgroup")
            .ok()
            .and_then(|text| {
                text.lines()
                    .find_map(|line| line.strip_prefix("0::").map(str::to_owned))
            });
        let mut directory = root.to_path_buf();
        if let Some(group) = group {
            let relative = std::path::Path::new(group.trim_start_matches('/'));
            if relative
                .components()
                .all(|c| matches!(c, std::path::Component::Normal(_)))
            {
                directory.push(relative);
            }
        }
        loop {
            if let (Ok(max), Ok(current)) = (
                std::fs::read_to_string(directory.join("memory.max")),
                std::fs::read_to_string(directory.join("memory.current")),
            ) && let (Ok(max), Ok(current)) =
                (max.trim().parse::<u64>(), current.trim().parse::<u64>())
            {
                snapshot.constrain(max, current);
            }
            if directory == root || !directory.pop() {
                break;
            }
        }
        snapshot
    }

    fn parse(text: &str) -> Self {
        let kib = |key| {
            text.lines().find_map(|line| {
                let rest = line.strip_prefix(key)?;
                let mut words = rest.split_whitespace();
                let value = words.next()?.parse::<u64>().ok()?;
                (words.next()? == "kB").then_some(value / 1024)
            })
        };
        let total_mib = kib("MemTotal:");
        let available_mib = kib("MemAvailable:")
            .map(|available| total_mib.map_or(available, |total| available.min(total)));
        Self {
            total_mib,
            available_mib,
            process_limited: false,
        }
    }

    fn constrain(&mut self, maximum: u64, current: u64) {
        let headroom = maximum.saturating_sub(current) / MIB;
        let capacity = maximum / MIB;
        if self.available_mib.is_none_or(|value| headroom < value)
            || self.total_mib.is_none_or(|value| capacity < value)
        {
            self.process_limited = true;
        }
        self.available_mib = Some(
            self.available_mib
                .map_or(headroom, |value| value.min(headroom)),
        );
        self.total_mib = Some(self.total_mib.map_or(capacity, |value| value.min(capacity)));
    }

    pub fn automatic_budget_mib(self) -> u64 {
        // If detection fails, retain a small, editable fallback budget.
        self.available_mib.map_or_else(
            || self.total_mib.map_or(1024, |total| (total / 4).min(1024)),
            |available| available / 4,
        )
    }

    pub fn limits(self, preferences: &Preferences) -> Limits {
        let budget_mib = preferences
            .memory_budget_mib
            .unwrap_or_else(|| self.automatic_budget_mib());
        let memory_limit =
            usize::try_from(budget_mib / DATABASE_RESERVATION_MIB).unwrap_or(usize::MAX);
        let database_limit = preferences
            .database_limit
            .unwrap_or(memory_limit)
            .min(memory_limit);
        Limits {
            budget_mib,
            memory_limit,
            database_limit,
        }
    }
}

pub fn format_mib(mib: u64) -> String {
    if mib >= 1024 {
        format!("{:.1} GiB", mib as f64 / 1024.0)
    } else {
        format!("{mib} MiB")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn available_memory_sets_budget_and_includes_parent_process_limits() {
        let mut memory = MemorySnapshot::parse(
            "MemTotal: 33554432 kB\nMemFree: 1048576 kB\nMemAvailable: 16777216 kB\n",
        );
        assert_eq!(memory.automatic_budget_mib(), 4096);
        memory.constrain(8192 * MIB, 2048 * MIB);
        memory.constrain(4096 * MIB, 1024 * MIB);
        assert_eq!(memory.available_mib, Some(3072));
        assert_eq!(memory.total_mib, Some(4096));
        assert_eq!(memory.automatic_budget_mib(), 768);
        assert!(memory.process_limited);
        memory.constrain(1024 * MIB, 2048 * MIB);
        assert_eq!(memory.available_mib, Some(0));
    }

    #[test]
    fn overrides_are_limited_by_budget_and_handle_low_or_unknown_memory() {
        let mut preferences = Preferences::default();
        let memory = MemorySnapshot {
            available_mib: Some(16 * 1024),
            ..Default::default()
        };
        assert_eq!(memory.limits(&preferences).database_limit, 8);
        preferences.database_limit = Some(20);
        preferences.memory_budget_mib = Some(2048);
        assert_eq!(memory.limits(&preferences).database_limit, 4);
        preferences.database_limit = Some(2);
        assert_eq!(memory.limits(&preferences).database_limit, 2);
        preferences.memory_budget_mib = Some(511);
        assert_eq!(memory.limits(&preferences).database_limit, 0);
        assert_eq!(MemorySnapshot::default().automatic_budget_mib(), 1024);
        assert_eq!(
            MemorySnapshot::parse("MemAvailable: broken kB").available_mib,
            None
        );
    }
}
