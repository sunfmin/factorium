//! The host's settings: its bounds and shares. One stream; each setting has
//! a default, and the `settings` command changes one at a time.
//!
//! Callers ask by variant: a [`Setting`] for a share, size or count in its
//! own unit, a [`Time`] for a bound in ms. Only the CLI names one by text.

use std::collections::BTreeMap;

use factum_core::Aggregate;
use serde::{Deserialize, Serialize};

use crate::{command, id, no};

pub fn settings_id() -> factum_core::Uuid {
    id("settings")
}

/// A time a setting bounds. Kept in seconds; read in ms.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Time {
    HotFor,
    SilentBound,
    HoldBound,
    StallBound,
    PromotionBound,
    CutBound,
    TasksBound,
    LookEvery,
    RoundBound,
    HeldFor,
    TickEvery,
    HeavySettle,
    PickBound,
}

/// Every setting there is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Setting {
    CpuBusyMax,
    MemoryFreeMin,
    DiskFloorGib,
    WorkersCpuShare,
    WorkerCpuShare,
    WorkerMemoryGib,
    WorkerProcsMax,
    TickFailuresMax,
    Time(Time),
}

/// What a setting's value counts.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Unit {
    Percent,
    Gib,
    Count,
    Seconds,
}

impl Unit {
    pub fn name(self) -> &'static str {
        match self {
            Unit::Percent => "percent",
            Unit::Gib => "GiB",
            Unit::Count => "count",
            Unit::Seconds => "seconds",
        }
    }
}

use Setting as S;
use Time as T;

/// Every setting, in the order the CLI lists them.
const ALL: [Setting; 21] = [
    S::CpuBusyMax,
    S::MemoryFreeMin,
    S::DiskFloorGib,
    S::WorkersCpuShare,
    S::WorkerCpuShare,
    S::WorkerMemoryGib,
    S::WorkerProcsMax,
    S::TickFailuresMax,
    S::Time(T::HotFor),
    S::Time(T::SilentBound),
    S::Time(T::HoldBound),
    S::Time(T::StallBound),
    S::Time(T::PromotionBound),
    S::Time(T::CutBound),
    S::Time(T::TasksBound),
    S::Time(T::LookEvery),
    S::Time(T::RoundBound),
    S::Time(T::HeldFor),
    S::Time(T::TickEvery),
    S::Time(T::HeavySettle),
    S::Time(T::PickBound),
];

impl Setting {
    /// Every setting, in listing order.
    pub fn all() -> impl Iterator<Item = Setting> {
        ALL.into_iter()
    }

    /// The setting the CLI calls `name`, if there is one.
    pub fn named(name: &str) -> Option<Setting> {
        Self::all().find(|s| s.name() == name)
    }

    /// Each setting's one home: its name, default, unit and what it bounds.
    #[rustfmt::skip]
    fn row(self) -> (&'static str, u64, Unit, &'static str) {
        use Unit::*;
        match self {
            S::CpuBusyMax => ("cpu_busy_max", 75, Percent, "admission and the queue start nothing while the CPU is busier than this"),
            S::MemoryFreeMin => ("memory_free_min", 15, Percent, "nor while less memory than this is free"),
            S::DiskFloorGib => ("disk_floor_gib", 20, Gib, "nor while less disk than this is free; worktrees' target/ are reclaimed"),
            S::WorkersCpuShare => ("workers_cpu_share", 80, Percent, "workers together may hold this much of the CPU"),
            S::WorkerCpuShare => ("worker_cpu_share", 50, Percent, "one worker past this share of the CPU is hot"),
            S::WorkerMemoryGib => ("worker_memory_gib", 16, Gib, "one worker past this resident memory is hot"),
            S::WorkerProcsMax => ("worker_procs_max", 400, Count, "one worker past this many processes is hot"),
            S::TickFailuresMax => ("tick_failures_max", 3, Count, "this many ticks failing in a row go to the watch"),
            S::Time(T::HotFor) => ("hot_for", 120, Seconds, "a share held this long makes a worker hot"),
            S::Time(T::SilentBound) => ("silent_bound", 1800, Seconds, "a worker idle this long without done, or hung with its turn open, goes to the watch"),
            S::Time(T::HoldBound) => ("hold_bound", 1800, Seconds, "a heavy command holding others up this long is ended"),
            S::Time(T::StallBound) => ("stall_bound", 600, Seconds, "a heavy queue that starts nothing, or a landing no gate runs for, this long goes to the watch"),
            S::Time(T::PromotionBound) => ("promotion_bound", 3600, Seconds, "the oldest unpromoted landing waits at most this long"),
            S::Time(T::CutBound) => ("cut_bound", 86400, Seconds, "the oldest uncut promotion waits at most this long"),
            S::Time(T::TasksBound) => ("tasks_bound", 86400, Seconds, "idle work's tasks run this long past `every` without an idle host run anyway, at background priority"),
            S::Time(T::LookEvery) => ("look_every", 300, Seconds, "the watch's look runs this often"),
            S::Time(T::RoundBound) => ("round_bound", 3600, Seconds, "a watch round lasts at most this long"),
            S::Time(T::HeldFor) => ("held_for", 1800, Seconds, "admission held for one reason this long goes to the watch"),
            S::Time(T::TickEvery) => ("tick_every", 10, Seconds, "the daemon ticks this often"),
            S::Time(T::HeavySettle) => ("heavy_settle", 30, Seconds, "a heavy command starts beside another only on a reading taken this long after the last start"),
            S::Time(T::PickBound) => ("pick_bound", 600, Seconds, "a worker's mutant picker (`worker pick-mutants`) is ended, red, past this"),
        }
    }

    pub fn name(self) -> &'static str {
        self.row().0
    }

    pub fn default(self) -> u64 {
        self.row().1
    }

    pub fn unit(self) -> Unit {
        self.row().2
    }

    /// What the setting bounds.
    pub fn what(self) -> &'static str {
        self.row().3
    }
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Settings {
    pub set: BTreeMap<String, u64>,
}

impl Settings {
    /// The setting, in its unit: what was set, or its default.
    pub fn get(&self, setting: Setting) -> u64 {
        self.set.get(setting.name()).copied().unwrap_or_else(|| setting.default())
    }

    /// A time setting, in ms.
    pub fn ms(&self, time: Time) -> i64 {
        self.get(S::Time(time)) as i64 * 1000
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
    let Some(setting) = Setting::named(&p.name) else {
        return no("no_setting", format!("there is no setting {}", p.name));
    };
    Ok(if s.get(setting) == p.value {
        Vec::new()
    } else {
        vec![SettingsEvent::SettingSet { name: p.name.clone(), value: p.value }]
    })
});
