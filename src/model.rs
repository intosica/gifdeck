//! Persistent data model: widgets, presets, schedule rules.
//!
//! Terminology
//! * **Widget**  – one GIF/PNG shown in its own window.
//! * **Preset**  – a named, ordered set of widgets. Exactly one preset is active.
//! * **Rule**    – "use preset P on these weekdays, between these times".
//!
//! Ordering convention: index 0 of `Preset::widgets` is the *front-most* widget
//! (highest layer priority). The list in the UI is shown in the same order.

use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// Which stacking layer a widget lives in.
#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Hash, Debug, Default)]
pub enum Level {
    /// Pinned to the desktop, behind normal windows.
    Desktop,
    /// An ordinary window layer.
    #[default]
    Normal,
    /// Always on top of other windows.
    Top,
}

impl Level {
    pub const ALL: [Level; 3] = [Level::Normal, Level::Top, Level::Desktop];

    pub fn label(self) -> &'static str {
        match self {
            Level::Normal => "Normal",
            Level::Top => "Always on top",
            Level::Desktop => "Desktop (behind windows)",
        }
    }

    pub fn from_index(i: u32) -> Level {
        Level::ALL.get(i as usize).copied().unwrap_or_default()
    }

    pub fn index(self) -> u32 {
        Level::ALL.iter().position(|l| *l == self).unwrap_or(0) as u32
    }
}

/// How a folder widget picks the file it shows.
#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum RotMode {
    /// Next file (natural name order) every `interval` seconds.
    #[default]
    Sequential,
    /// Random file after a random delay between `min` and `max` seconds.
    Random,
    /// File chosen by time of day (`Rotation::schedule`).
    Scheduled,
}

impl RotMode {
    pub const ALL: [RotMode; 3] = [RotMode::Sequential, RotMode::Random, RotMode::Scheduled];

    pub fn label(self) -> &'static str {
        match self {
            RotMode::Sequential => "Sequential (in file order)",
            RotMode::Random => "Random",
            RotMode::Scheduled => "Scheduled (by time of day)",
        }
    }

    pub fn from_index(i: u32) -> RotMode {
        RotMode::ALL.get(i as usize).copied().unwrap_or_default()
    }

    pub fn index(self) -> u32 {
        RotMode::ALL.iter().position(|m| *m == self).unwrap_or(0) as u32
    }
}

/// "From this time of day on, show this file."
#[derive(Serialize, Deserialize, Clone, PartialEq, Debug, Default)]
#[serde(default)]
pub struct SchedEntry {
    /// Minutes since midnight.
    pub start: u16,
    /// File name inside the widget's folder.
    pub file: String,
}

#[derive(Serialize, Deserialize, Clone, PartialEq, Debug)]
#[serde(default)]
pub struct Rotation {
    pub mode: RotMode,
    /// Seconds (sequential).
    pub interval: u32,
    /// Seconds (random bounds).
    pub min: u32,
    pub max: u32,
    pub schedule: Vec<SchedEntry>,
}

impl Default for Rotation {
    fn default() -> Self {
        Rotation { mode: RotMode::Sequential, interval: 60, min: 30, max: 300, schedule: Vec::new() }
    }
}

impl Rotation {
    /// Index into `files` the schedule selects at `minute` (last entry that started, wrapping over midnight).
    pub fn scheduled_file<'a>(&self, files: &'a [PathBuf], minute: u16) -> Option<&'a PathBuf> {
        let name_of = |p: &PathBuf| p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        let mut entries: Vec<&SchedEntry> = self.schedule.iter().collect();
        entries.sort_by_key(|e| e.start);
        let entry = entries.iter().rev().find(|e| e.start <= minute).or(entries.last())?;
        files.iter().find(|f| name_of(f) == entry.file)
    }
}

/// How a widget reacts to an application.
#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum AppMode {
    #[default]
    Always,
    /// Shown only while the application has a window.
    OnlyWhenOpen,
    /// Moves to the desktop layer while the application's window is showing.
    MinimizeWhenOpen,
}

impl AppMode {
    pub const ALL: [AppMode; 3] = [AppMode::Always, AppMode::OnlyWhenOpen, AppMode::MinimizeWhenOpen];

    pub fn label(self) -> &'static str {
        match self {
            AppMode::Always => "Always",
            AppMode::OnlyWhenOpen => "Only when the application is open",
            AppMode::MinimizeWhenOpen => "Minimize when the application is open",
        }
    }

    pub fn from_index(i: u32) -> AppMode {
        AppMode::ALL.get(i as usize).copied().unwrap_or_default()
    }

    pub fn index(self) -> u32 {
        AppMode::ALL.iter().position(|m| *m == self).unwrap_or(0) as u32
    }
}

/// An installed application, identified well enough to recognise its windows.
#[derive(Serialize, Deserialize, Clone, PartialEq, Debug, Default)]
#[serde(default)]
pub struct AppRef {
    /// Desktop file id.
    pub id: String,
    pub name: String,
    pub icon: String,
    /// Lower-case names a window may announce itself with (WM_CLASS, application id, executable).
    pub keys: Vec<String>,
}

/// Where imported files live.
#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum Storage {
    /// Use the file where it is.
    Source,
    /// Copy into `~/GifDeck/…`.
    #[default]
    Home,
    /// Copy into `~/Documents/GifDeck/…`.
    Documents,
}

impl Storage {
    pub const ALL: [Storage; 3] = [Storage::Source, Storage::Home, Storage::Documents];

    pub fn label(self) -> &'static str {
        match self {
            Storage::Source => "Original location",
            Storage::Home => "Copy to Home",
            Storage::Documents => "Copy to Documents",
        }
    }

    pub fn from_index(i: u32) -> Storage {
        Storage::ALL.get(i as usize).copied().unwrap_or_default()
    }

    pub fn index(self) -> u32 {
        Storage::ALL.iter().position(|m| *m == self).unwrap_or(0) as u32
    }

    /// `<base>/GifDeck/<kind>`; `None` when files are used in place.
    pub fn dir(self, kind: &str) -> Option<PathBuf> {
        let base = match self {
            Storage::Source => return None,
            Storage::Home => gtk::glib::home_dir(),
            Storage::Documents => gtk::glib::user_special_dir(gtk::glib::UserDirectory::Documents).unwrap_or_else(|| gtk::glib::home_dir().join("Documents")),
        };
        Some(base.join("GifDeck").join(kind))
    }
}

/// A saved PNG used as clipping mask (its alpha channel is the shape).
#[derive(Serialize, Deserialize, Clone, PartialEq, Debug, Default)]
#[serde(default)]
pub struct Mask {
    pub id: String,
    pub name: String,
    pub path: PathBuf,
}

#[derive(Serialize, Deserialize, Clone, PartialEq, Debug)]
#[serde(default)]
pub struct WidgetConfig {
    pub id: String,
    /// Path of the (app-owned copy of the) image.
    pub path: PathBuf,
    pub x: i32,
    pub y: i32,
    /// 1.0 = original size.
    pub scale: f64,
    /// 0.0 – 1.0
    pub opacity: f64,
    /// Show the standard window decorations (title bar / border).
    pub decorated: bool,
    pub level: Level,
    /// Let mouse clicks fall through to whatever is behind the widget.
    pub click_through: bool,
    /// Whether the widget is currently shown.
    pub visible: bool,
    /// Folder widgets (`path` is a directory): how the shown file changes.
    pub rotation: Rotation,
    /// Alpha mask (PNG path) clipping the widget.
    pub mask: Option<PathBuf>,
    /// While dragging, stick to the screen edges when close.
    pub snap_edges: bool,
    /// While dragging, never leave the screen.
    pub keep_inside: bool,
    pub app: Option<AppRef>,
    pub app_mode: AppMode,
}

impl Default for WidgetConfig {
    fn default() -> Self {
        WidgetConfig {
            id: new_id(),
            path: PathBuf::new(),
            x: 80,
            y: 80,
            scale: 1.0,
            opacity: 1.0,
            decorated: false,
            level: Level::Normal,
            click_through: false,
            visible: true,
            rotation: Rotation::default(),
            mask: None,
            snap_edges: false,
            keep_inside: false,
            app: None,
            app_mode: AppMode::Always,
        }
    }
}

impl WidgetConfig {
    pub fn is_folder(&self) -> bool {
        self.path.is_dir()
    }

    /// `slot` staggers new widgets so they do not all open on the same spot.
    pub fn new(path: PathBuf, slot: usize) -> Self {
        let off = (slot % 8) as i32 * 40;
        WidgetConfig { path, x: 80 + off, y: 80 + off, ..Default::default() }
    }

    pub fn title(&self) -> String {
        // Media copies are stored as "<id>-<original name>"; show the original.
        if self.is_folder() {
            return self.path.file_name().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
        }
        let name = self.path.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
        match name.split_once('-') {
            Some((prefix, rest)) if prefix.len() >= 8 && !rest.is_empty() => rest.to_string(),
            _ => name,
        }
    }
}

#[derive(Serialize, Deserialize, Clone, PartialEq, Debug)]
#[serde(default)]
pub struct Preset {
    pub id: String,
    pub name: String,
    /// Front-most first.
    pub widgets: Vec<WidgetConfig>,
}

impl Default for Preset {
    fn default() -> Self {
        Preset { id: new_id(), name: "Preset".into(), widgets: Vec::new() }
    }
}

pub const DAY_NAMES: [&str; 7] = ["Mon", "Tue", "Wed", "Thu", "Fri", "Sat", "Sun"];

/// "Use `preset` on these days, in this time window."
#[derive(Serialize, Deserialize, Clone, PartialEq, Debug)]
#[serde(default)]
pub struct Rule {
    pub id: String,
    pub preset: String,
    /// Monday .. Sunday
    pub days: [bool; 7],
    pub all_day: bool,
    /// Minutes since midnight, inclusive.
    pub start: u16,
    /// Minutes since midnight, exclusive. `end < start` means "past midnight".
    pub end: u16,
}

impl Default for Rule {
    fn default() -> Self {
        Rule {
            id: new_id(),
            preset: String::new(),
            days: [true; 7],
            all_day: true,
            start: 9 * 60,
            end: 17 * 60,
        }
    }
}

impl Rule {
    /// `day`: 0 = Monday … 6 = Sunday. `minute`: minutes since midnight.
    pub fn matches(&self, day: usize, minute: u16) -> bool {
        let day = day % 7;
        if self.all_day || self.start == self.end {
            return self.days[day];
        }
        if self.start < self.end {
            self.days[day] && minute >= self.start && minute < self.end
        } else {
            // Overnight window, e.g. 22:00 – 06:00. The part after midnight
            // belongs to the rule's *previous* day.
            (self.days[day] && minute >= self.start) || (self.days[(day + 6) % 7] && minute < self.end)
        }
    }
}

/// "16:05" or "4:05 PM".
pub fn fmt_time(m: u16, h24: bool) -> String {
    let (h, min) = ((m / 60) % 24, m % 60);
    if h24 {
        format!("{h:02}:{min:02}")
    } else {
        format!("{}:{min:02} {}", if h % 12 == 0 { 12 } else { h % 12 }, if h < 12 { "AM" } else { "PM" })
    }
}

/// First matching rule wins (rules are ordered by the user).
pub fn evaluate(rules: &[Rule], day: usize, minute: u16) -> Option<&Rule> {
    rules.iter().find(|r| r.matches(day, minute))
}

/// Current (weekday 0=Mon, minute-of-day) in local time.
pub fn now_slot() -> (usize, u16) {
    match gtk::glib::DateTime::now_local() {
        Ok(dt) => {
            let day = (dt.day_of_week().clamp(1, 7) - 1) as usize;
            (day, (dt.hour() * 60 + dt.minute()) as u16)
        }
        Err(_) => (0, 0),
    }
}

#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(default)]
pub struct AppState {
    pub version: u32,
    pub presets: Vec<Preset>,
    /// Id of the active preset.
    pub active: String,
    pub rules: Vec<Rule>,
    pub schedule_enabled: bool,
    /// What the user asked for; the real state lives in the desktop/portal.
    pub autostart: bool,
    /// Master switch (header button / quick settings toggle).
    pub widgets_enabled: bool,
    /// Widgets are hidden while any of these applications is showing a window.
    pub blacklist: Vec<AppRef>,
    /// Show times as 24-hour ("military") instead of AM/PM.
    pub time_24h: bool,
    pub storage: Storage,
    pub masks: Vec<Mask>,
}

impl Default for AppState {
    fn default() -> Self {
        let preset = Preset { name: "Default".into(), ..Default::default() };
        AppState {
            version: 1,
            active: preset.id.clone(),
            presets: vec![preset],
            rules: Vec::new(),
            schedule_enabled: false,
            autostart: false,
            widgets_enabled: true,
            blacklist: Vec::new(),
            time_24h: true,
            storage: Storage::default(),
            masks: Vec::new(),
        }
    }
}

impl AppState {
    pub fn config_dir() -> PathBuf {
        gtk::glib::user_config_dir().join("gifdeck")
    }

    pub fn data_dir() -> PathBuf {
        gtk::glib::user_data_dir().join("gifdeck")
    }

    /// Where imported images are copied to.
    pub fn media_dir() -> PathBuf {
        Self::data_dir().join("media")
    }

    fn file() -> PathBuf {
        Self::config_dir().join("state.json")
    }

    pub fn load() -> Self {
        let mut st: AppState = fs::read_to_string(Self::file())
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default();
        st.repair();
        st
    }

    pub fn save(&self) {
        let dir = Self::config_dir();
        if fs::create_dir_all(&dir).is_err() {
            return;
        }
        let tmp = dir.join("state.json.tmp");
        if let Ok(json) = serde_json::to_string_pretty(self) {
            // Write + rename so a crash never leaves a half-written file.
            if fs::write(&tmp, json).is_ok() {
                let _ = fs::rename(&tmp, Self::file());
            }
        }
    }

    /// Make sure invariants hold: at least one preset, a valid active id,
    /// rules that point at existing presets.
    pub fn repair(&mut self) {
        if self.presets.is_empty() {
            *self = AppState::default();
            return;
        }
        if !self.presets.iter().any(|p| p.id == self.active) {
            self.active = self.presets[0].id.clone();
        }
        let ids: Vec<String> = self.presets.iter().map(|p| p.id.clone()).collect();
        self.rules.retain(|r| ids.contains(&r.preset));
    }

    pub fn preset(&self, id: &str) -> Option<&Preset> {
        self.presets.iter().find(|p| p.id == id)
    }

    pub fn active_preset(&self) -> &Preset {
        self.preset(&self.active).unwrap_or(&self.presets[0])
    }

    pub fn active_preset_mut(&mut self) -> &mut Preset {
        let idx = self.presets.iter().position(|p| p.id == self.active).unwrap_or(0);
        &mut self.presets[idx]
    }

    /// Every media file still referenced by some preset.
    pub fn referenced_media(&self) -> Vec<&Path> {
        let widgets = self.presets.iter().flat_map(|p| p.widgets.iter().map(|w| w.path.as_path()));
        widgets.chain(self.masks.iter().map(|m| m.path.as_path())).collect()
    }
}

/// A top-level window somebody else owns (X11 only).
#[derive(Clone, PartialEq, Debug, Default)]
pub struct Win {
    /// Lower-case WM_CLASS parts / application id.
    pub keys: Vec<String>,
    pub minimized: bool,
}

/// Does `app` have a window? `showing_only`: ignore minimized ones.
pub fn app_open(app: &AppRef, wins: &[Win], showing_only: bool) -> bool {
    wins.iter().any(|w| (!showing_only || !w.minimized) && w.keys.iter().any(|k| app.keys.contains(k)))
}

impl AppState {
    /// What should really be on screen: the active preset's widgets after the master switch,
    /// the blacklist and each widget's application rule.
    pub fn effective_widgets(&self, wins: &[Win]) -> Vec<WidgetConfig> {
        let hide_all = !self.widgets_enabled || self.blacklist.iter().any(|a| app_open(a, wins, true));
        self.active_preset()
            .widgets
            .iter()
            .map(|w| {
                let mut w = w.clone();
                if hide_all {
                    w.visible = false;
                }
                if let Some(app) = &w.app {
                    match w.app_mode {
                        AppMode::Always => {}
                        AppMode::OnlyWhenOpen => w.visible &= app_open(app, wins, false),
                        AppMode::MinimizeWhenOpen if app_open(app, wins, true) => w.level = Level::Desktop,
                        AppMode::MinimizeWhenOpen => {}
                    }
                }
                w
            })
            .collect()
    }
}

pub fn new_id() -> String {
    gtk::glib::uuid_string_random().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rule(days: &[usize], start: u16, end: u16) -> Rule {
        let mut d = [false; 7];
        for i in days {
            d[*i] = true;
        }
        Rule { days: d, all_day: false, start, end, ..Default::default() }
    }

    #[test]
    fn daytime_window() {
        let r = rule(&[0, 1, 2, 3, 4], 9 * 60, 17 * 60);
        assert!(r.matches(0, 9 * 60));
        assert!(r.matches(4, 16 * 60 + 59));
        assert!(!r.matches(4, 17 * 60)); // end is exclusive
        assert!(!r.matches(5, 12 * 60)); // Saturday
        assert!(!r.matches(0, 8 * 60 + 59));
    }

    #[test]
    fn overnight_window_spills_into_next_day() {
        // Friday 22:00 -> Saturday 06:00
        let r = rule(&[4], 22 * 60, 6 * 60);
        assert!(r.matches(4, 23 * 60));
        assert!(r.matches(5, 5 * 60 + 59)); // Saturday early morning belongs to Friday's rule
        assert!(!r.matches(5, 6 * 60));
        assert!(!r.matches(5, 23 * 60)); // Saturday night is NOT selected
        assert!(!r.matches(4, 5 * 60)); // Friday early morning belongs to Thursday's rule
        // Sunday -> Monday wrap
        let r = rule(&[6], 22 * 60, 2 * 60);
        assert!(r.matches(0, 60));
    }

    #[test]
    fn all_day_and_degenerate() {
        let mut r = rule(&[2], 0, 0);
        assert!(r.matches(2, 12 * 60) && !r.matches(3, 12 * 60)); // start == end -> all day
        r.all_day = true;
        r.start = 100;
        r.end = 200;
        assert!(r.matches(2, 999));
    }

    #[test]
    fn first_matching_rule_wins() {
        let mut a = rule(&[0], 0, 0);
        a.preset = "a".into();
        let mut b = Rule { preset: "b".into(), ..Default::default() }; // every day
        b.all_day = true;
        let rules = vec![a, b];
        assert_eq!(evaluate(&rules, 0, 600).map(|r| r.preset.as_str()), Some("a"));
        assert_eq!(evaluate(&rules, 1, 600).map(|r| r.preset.as_str()), Some("b"));
        assert!(evaluate(&[], 0, 0).is_none());
    }

    #[test]
    fn state_roundtrip_and_repair() {
        let mut st = AppState::default();
        st.active_preset_mut().widgets.push(WidgetConfig::new("/tmp/x.gif".into(), 0));
        st.rules.push(Rule { preset: "does-not-exist".into(), ..Default::default() });
        st.active = "bogus".into();
        let json = serde_json::to_string(&st).unwrap();
        let mut back: AppState = serde_json::from_str(&json).unwrap();
        back.repair();
        assert_eq!(back.active, back.presets[0].id);
        assert!(back.rules.is_empty());
        assert_eq!(back.presets[0].widgets.len(), 1);
    }

    #[test]
    fn old_files_with_missing_fields_still_load() {
        let json = r#"{"presets":[{"id":"p","name":"N","widgets":[{"path":"/a.gif"}]}],"active":"p"}"#;
        let mut st: AppState = serde_json::from_str(json).unwrap();
        st.repair();
        let w = &st.presets[0].widgets[0];
        assert_eq!(w.scale, 1.0);
        assert!(w.visible && !w.decorated);
    }

    #[test]
    fn title_strips_import_prefix() {
        let w = WidgetConfig::new("/x/0f8a2c1e-cat dance.gif".into(), 0);
        assert_eq!(w.title(), "cat dance");
        let w = WidgetConfig::new("/x/plain.gif".into(), 0);
        assert_eq!(w.title(), "plain");
    }

    #[test]
    fn app_rules() {
        let term = AppRef { keys: vec!["term".into()], ..Default::default() };
        let game = AppRef { keys: vec!["game".into()], ..Default::default() };
        let mut st = AppState::default();
        let mut w = WidgetConfig::new("/a.gif".into(), 0);
        w.app = Some(term.clone());
        w.app_mode = AppMode::OnlyWhenOpen;
        let mut w2 = WidgetConfig::new("/b.gif".into(), 1);
        w2.app = Some(term.clone());
        w2.app_mode = AppMode::MinimizeWhenOpen;
        st.active_preset_mut().widgets = vec![w, w2];
        let win = |k: &str, minimized| Win { keys: vec![k.into()], minimized };

        let e = st.effective_widgets(&[]);
        assert!(!e[0].visible && e[1].visible && e[1].level == Level::Normal);
        let e = st.effective_widgets(&[win("term", false)]);
        assert!(e[0].visible && e[1].level == Level::Desktop);
        let e = st.effective_widgets(&[win("term", true)]); // minimized: still "open", no longer covering
        assert!(e[0].visible && e[1].level == Level::Normal);

        st.blacklist.push(game);
        assert!(st.effective_widgets(&[win("game", false)]).iter().all(|w| !w.visible));
        assert!(st.effective_widgets(&[win("game", true)]).iter().any(|w| w.visible));
        st.widgets_enabled = false;
        assert!(st.effective_widgets(&[]).iter().all(|w| !w.visible));
    }

    #[test]
    fn rotation_schedule_and_time_format() {
        let files: Vec<PathBuf> = ["a.gif", "b.gif"].iter().map(PathBuf::from).collect();
        let r = Rotation {
            schedule: vec![SchedEntry { start: 18 * 60, file: "b.gif".into() }, SchedEntry { start: 6 * 60, file: "a.gif".into() }],
            ..Default::default()
        };
        assert_eq!(r.scheduled_file(&files, 12 * 60), Some(&files[0]));
        assert_eq!(r.scheduled_file(&files, 19 * 60), Some(&files[1]));
        assert_eq!(r.scheduled_file(&files, 60), Some(&files[1])); // before the first entry: yesterday's last
        assert_eq!(fmt_time(16 * 60 + 5, true), "16:05");
        assert_eq!(fmt_time(16 * 60 + 5, false), "4:05 PM");
        assert_eq!(fmt_time(0, false), "12:00 AM");
    }
}
