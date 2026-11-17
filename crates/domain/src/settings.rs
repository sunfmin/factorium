//! The host's settings: its bounds and shares. One stream; each setting has
//! a default, and `factorium settings` changes one at a time.

use std::collections::BTreeMap;

use factum_core::Aggregate;
use serde::{Deserialize, Serialize};

use crate::{command, id, no};

pub fn settings_id() -> factum_core::Uuid {
    id("settings")
}

/// Every setting, its default and what it bounds. Times are seconds,
/// shares percents, sizes GiB.
pub const SETTINGS: &[(&str, u64, &str)] = &[
    ("cpu_busy_max", 75, "admission and the queue start nothing while the CPU is busier than this"),
    ("memory_free_min", 15, "nor while less memory than this is free"),
    ("disk_floor_gib", 20, "nor while less disk than this is free; worktrees' target/ are reclaimed"),
    ("workers_cpu_share", 80, "workers together may hold this much of the CPU"),
    ("worker_cpu_share", 50, "one worker past this share of the CPU is hot"),
    ("worker_memory_gib", 16, "one worker past this resident memory is hot"),
    ("worker_procs_max", 400, "one worker past this many processes is hot"),
    ("hot_for", 120, "a share held this long makes a worker hot"),
    ("silent_bound", 1800, "a worker idle this long without done goes to the watch"),
    ("hold_bound", 1800, "a heavy command holding others up this long is ended"),
    ("promotion_bound", 3600, "the oldest unpromoted landing waits at most this long"),
    ("cut_bound", 86400, "the oldest uncut promotion waits at most this long"),
    ("look_every", 300, "the watch's look runs this often"),
    ("round_bound", 3600, "a watch round lasts at most this long"),
    ("held_for", 1800, "admission held for one reason this long goes to the watch"),
    ("tick_every", 10, "the daemon ticks this often"),
];

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Settings {
    pub set: BTreeMap<String, u64>,
}

impl Settings {
    /// The setting `name`: what was set, or its default.
    pub fn get(&self, name: &str) -> u64 {
        self.set
            .get(name)
            .copied()
            .unwrap_or_else(|| SETTINGS.iter().find(|(n, ..)| *n == name).map(|(_, d, _)| *d).expect("a known setting"))
    }

    /// Seconds of a setting, in ms.
    pub fn ms(&self, name: &str) -> i64 {
        self.get(name) as i64 * 1000
    }
}

#[derive(Serialize, Deserialize)]
#[serde(tag = "type", content = "data")]
pub enum SettingsEvent {
    SettingSet { name: String, value: u64 },
}

pub struct HostSettings;

impl Aggregate for HostSettings {
    const NAME: &'static str = "settings";
    type State = Settings;
    type Event = SettingsEvent;

    fn evolve(s: &mut Settings, SettingsEvent::SettingSet { name, value }: SettingsEvent) {
        s.set.insert(name, value);
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Set {
    pub name: String,
    pub value: u64,
}

command!(SetSetting, "settings.set", HostSettings, Set, |_p| settings_id(), |s, p, now| {
    if !SETTINGS.iter().any(|(n, ..)| *n == p.name) {
        return no("no_setting", format!("there is no setting {}", p.name));
    }
    Ok(if s.get(&p.name) == p.value {
        Vec::new()
    } else {
        vec![SettingsEvent::SettingSet { name: p.name.clone(), value: p.value }]
    })
});
