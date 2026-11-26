//! The host's settings: its bounds and shares. One stream; each setting has
//! a default, and the `settings` command changes one at a time. The host's
//! named resources (a device every checkout's workers share) are declared
//! here too, each with its slots and a note (factorium#284).
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

/// Each setting's one home: its variant, name, default, unit and what it
/// bounds, in the order the CLI lists them; a time's unit is seconds. The
/// enums, the listing and [`Setting::row`] are made from it.
macro_rules! settings {
    ($($s:ident $name:literal $default:literal $unit:ident $what:literal;)*
     ; time: $($t:ident $tname:literal $tdefault:literal $twhat:literal;)*) => {
        /// A time a setting bounds. Kept in seconds; read in ms.
        #[derive(Clone, Copy, Debug, PartialEq, Eq)]
        pub enum Time { $($t),* }

        /// Every setting there is.
        #[derive(Clone, Copy, Debug, PartialEq, Eq)]
        pub enum Setting { $($s,)* Time(Time) }

        /// Every setting, in the order the CLI lists them.
        const ALL: &[Setting] = &[$(Setting::$s,)* $(Setting::Time(Time::$t)),*];

        impl Setting {
            fn row(self) -> (&'static str, u64, Unit, &'static str) {
                match self {
                    $(Setting::$s => ($name, $default, Unit::$unit, $what),)*
                    $(Setting::Time(Time::$t) => ($tname, $tdefault, Unit::Seconds, $twhat),)*
                }
            }
        }
    };
}

#[rustfmt::skip]
settings! {
    CpuBusyMax "cpu_busy_max" 75 Percent "admission and the queue start nothing while the CPU is busier than this";
    MemoryFreeMin "memory_free_min" 15 Percent "nor while less memory than this is free";
    DiskFloorGib "disk_floor_gib" 20 Gib "nor while less disk than this is free; worktrees' target/ are reclaimed";
    WorkersCpuShare "workers_cpu_share" 80 Percent "workers together may hold this much of the CPU";
    WorkersPerCheckout "workers_per_checkout" 6 Count "a checkout with this many live workers, those in landing among them, starts no other but an urgent issue's or a promotion's mutants issue";
    WorkerCpuShare "worker_cpu_share" 50 Percent "one worker past this share of the CPU is hot";
    WorkerMemoryGib "worker_memory_gib" 16 Gib "one worker past this resident memory is hot";
    WorkerProcsMax "worker_procs_max" 400 Count "one worker past this many processes is hot";
    TickFailuresMax "tick_failures_max" 3 Count "this many ticks failing in a row go to the watch";
    MovedRedsMax "moved_reds_max" 4 Count "a landing that does not merge because proving moved again counts as a first red at most this many times in a row";
    RoundsMax "rounds_max" 3 Count "a watch episode starts at most this many rounds; past them it is unresolved, the owner's";
    ; time:
    HotFor "hot_for" 120 "a share held this long makes a worker hot";
    SilentBound "silent_bound" 1800 "a worker idle this long without done, or hung with its turn open, goes to the watch";
    HoldBound "hold_bound" 1800 "a heavy command holding others up this long is ended";
    StallBound "stall_bound" 600 "a heavy queue that starts nothing, or a landing no gate runs for on a line that has not moved, this long goes to the watch";
    PromotionBound "promotion_bound" 3600 "the oldest unpromoted landing waits at most this long";
    MutantsBound "mutants_bound" 1800 "a promotion waits at most this long, from its filing, for the issue that picks and runs the mutants of what it promotes, where slimfit holds them at the base";
    CutBound "cut_bound" 86400 "the oldest uncut promotion waits at most this long";
    TasksBound "tasks_bound" 86400 "idle work's tasks run this long past `every` without an idle host run anyway, at background priority";
    LookEvery "look_every" 300 "the watch's look runs this often";
    RoundBound "round_bound" 3600 "a watch round lasts at most this long";
    RoundAgain "round_again" 1800 "a watch episode still standing this long after its round ended starts another";
    HeldFor "held_for" 1800 "admission held for one reason this long goes to the watch";
    TickEvery "tick_every" 10 "the daemon ticks this often";
    HeavySettle "heavy_settle" 30 "a heavy command starts beside another only on a reading taken this long after the last start";
    HeavySettleIdle "heavy_settle_idle" 10 "or this long after it, on a reading at most half of `cpu_busy_max`";
    HeavyKeep "heavy_keep" 600 "a worker's or a program's heavy command that left the queue unstarted (refused, or its waiter gone) keeps its place this long: asked again, it takes it";
    QueueWaitBound "queue_wait_bound" 300 "admission starts no worker while a heavy command has waited its turn this long";
    PickBound "pick_bound" 600 "a worker's second agent (`worker pick-mutants`, `worker review-growth`) is ended, red, past this";
    UpgradeEvery "upgrade_every" 3600 "a daemon whose host cuts no version of its own formula upgrades it from the tap this often, while no line move runs";
    StarveBound "starve_bound" 14400 "one waiting this long for a resource goes ahead of all others, oldest first, however long it declared it holds";
    HoldDefault "hold_default" 1800 "a command that declares no hold of its resource is read as holding it this long, in the shortest-first order of its queue";
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

impl Setting {
    /// Every setting, in listing order.
    pub fn all() -> impl Iterator<Item = Setting> {
        ALL.iter().copied()
    }

    /// The setting the CLI calls `name`, if there is one.
    pub fn named(name: &str) -> Option<Setting> {
        Self::all().find(|s| s.name() == name)
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
    /// The named resources, by name.
    #[serde(default)]
    pub resources: BTreeMap<String, Resource>,
}

/// A named resource: how many hold it at once, and what it is.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Resource {
    pub slots: u64,
    #[serde(default)]
    pub note: Option<String>,
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

    /// How each resource's queue is ordered: `starve_bound` and `hold_default`.
    pub fn order(&self) -> crate::neutral::queue::Order {
        crate::neutral::queue::Order { starve: self.ms(T::StarveBound), default: self.ms(T::HoldDefault) }
    }

    /// The slots of each named resource `names` asks for that is declared;
    /// else the first not declared, refused.
    pub fn slots(&self, names: &[String]) -> Result<BTreeMap<String, u64>, String> {
        let mut slots = BTreeMap::new();
        for name in names.iter().filter(|n| n.as_str() != crate::heavy::HOST) {
            let Some(r) = self.resources.get(name) else {
                let declared: Vec<&String> = self.resources.keys().collect();
                return Err(format!(
                    "the host declares no resource {name} (it declares {declared:?}, and `host`): the owner \
                     declares one with `factorium settings --resource {name} --slots 1 [--note <what it is>]`"
                ));
            };
            slots.insert(name.clone(), r.slots);
        }
        Ok(slots)
    }
}

#[derive(Serialize, Deserialize)]
#[serde(tag = "type", content = "data")]
pub enum SettingsEvent {
    SettingSet { name: String, value: u64 },
    ResourceSet { name: String, resource: Resource },
    ResourceRemoved { name: String },
}

pub struct HostSettings;

impl Aggregate for HostSettings {
    const NAME: &'static str = "settings";
    type State = Settings;
    type Event = SettingsEvent;

    fn evolve(s: &mut Settings, event: SettingsEvent) {
        match event {
            SettingsEvent::SettingSet { name, value } => {
                s.set.insert(name, value);
            }
            SettingsEvent::ResourceSet { name, resource } => {
                s.resources.insert(name, resource);
            }
            SettingsEvent::ResourceRemoved { name } => {
                s.resources.remove(&name);
            }
        }
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
    crate::only(s.get(setting) != p.value, SettingsEvent::SettingSet { name: p.name.clone(), value: p.value })
});

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Declare {
    pub name: String,
    /// Its slots and note; none: it is no longer declared.
    pub resource: Option<Resource>,
}

command!(DeclareResource, "settings.resource", HostSettings, Declare, |_p| settings_id(), |s, p, _now| {
    let name = p.name.as_str();
    if name == crate::heavy::HOST {
        return no("host_resource", "`host` is the host's load, by the room rule: it is not declared".to_string());
    }
    if name.is_empty() || !name.chars().all(|c| c.is_ascii_alphanumeric() || "_-.".contains(c)) {
        return no("bad_resource", format!("{name:?}: a resource's name is letters, digits, `_`, `-` and `.`"));
    }
    Ok(match &p.resource {
        Some(r) if r.slots == 0 => return no("bad_resource", "a resource has one slot at least".to_string()),
        Some(r) if s.resources.get(name) != Some(r) => {
            vec![SettingsEvent::ResourceSet { name: name.into(), resource: r.clone() }]
        }
        None if s.resources.contains_key(name) => vec![SettingsEvent::ResourceRemoved { name: name.into() }],
        _ => Vec::new(),
    })
});
