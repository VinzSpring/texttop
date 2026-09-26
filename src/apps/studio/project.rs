//! Studio project model, genre templates, and JSON save/load.

use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

pub const SCENES: usize = 8;
pub const DRUM_VOICES: usize = 8;
pub const STEPS: usize = 16;
pub const DRUM_NAMES: [&str; DRUM_VOICES] = ["Kick", "Snare", "CH", "OH", "Clap", "Perc", "Tom", "Rim"];
/// Mixer and project share this cap so extra sampler tracks still sound.
pub const MAX_TRACKS: usize = 16;
pub const MAX_CLIPS: usize = 256;
pub const MAX_ARR_CLIPS: usize = 512;
pub const MAX_NOTES: usize = 1024;
pub const MAX_BARS: u32 = 256;
/// Longest MIDI clip: four bars of sixteenths.
pub const MAX_CLIP_STEPS: u16 = 64;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TrackKind {
    Drums,
    Bass,
    Lead,
    Pad,
    Sampler,
}

impl TrackKind {
    pub fn color(self) -> (u8, u8, u8) {
        match self {
            TrackKind::Drums => (255, 140, 70),
            TrackKind::Bass => (180, 120, 255),
            TrackKind::Lead => (70, 210, 255),
            TrackKind::Pad => (255, 120, 180),
            TrackKind::Sampler => (110, 220, 140),
        }
    }
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
pub struct FxChips {
    #[serde(default)]
    pub filter: bool,
    #[serde(default)]
    pub delay: bool,
    #[serde(default)]
    pub reverb: bool,
    #[serde(default)]
    pub pump: bool,
    #[serde(default = "default_filter_amt")]
    pub filter_amt: f32,
}

fn default_filter_amt() -> f32 {
    0.55
}

impl Default for FxChips {
    fn default() -> Self {
        FxChips {
            filter: false,
            delay: false,
            reverb: false,
            pump: false,
            filter_amt: 0.55,
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Track {
    pub name: String,
    pub kind: TrackKind,
    #[serde(default = "default_vol")]
    pub vol: f32,
    #[serde(default)]
    pub pan: f32,
    #[serde(default)]
    pub mute: bool,
    #[serde(default)]
    pub solo: bool,
    #[serde(default)]
    pub fx: FxChips,
}

fn default_vol() -> f32 {
    0.85
}

impl Track {
    pub fn new(name: &str, kind: TrackKind) -> Self {
        Track {
            name: name.into(),
            kind,
            vol: 0.85,
            pan: 0.0,
            mute: false,
            solo: false,
            fx: FxChips::default(),
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct MidiNote {
    pub pitch: u8,
    pub start: u16,
    pub length: u16,
    #[serde(default = "default_vel")]
    pub vel: u8,
}

fn default_vel() -> u8 {
    100
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum ClipKind {
    Drum {
        steps: [[bool; STEPS]; DRUM_VOICES],
    },
    Midi {
        notes: Vec<MidiNote>,
    },
    Audio {
        path: String,
        #[serde(default)]
        looped: bool,
    },
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Clip {
    pub id: String,
    pub name: String,
    pub kind: ClipKind,
}

impl Clip {
    pub fn length_steps(&self) -> u16 {
        match &self.kind {
            ClipKind::Drum { .. } => STEPS as u16,
            ClipKind::Midi { notes } => {
                let end = notes
                    .iter()
                    .map(|n| n.start.saturating_add(n.length))
                    .max()
                    .unwrap_or(0)
                    .clamp(STEPS as u16, MAX_CLIP_STEPS);
                end.div_ceil(STEPS as u16) * STEPS as u16
            }
            ClipKind::Audio { .. } => STEPS as u16,
        }
    }

    pub fn bars(&self) -> u32 {
        (self.length_steps() as u32 / STEPS as u32).max(1)
    }

    /// Whether this clip can play on a track of `kind`.
    pub fn fits(&self, kind: TrackKind) -> bool {
        matches!(
            (&self.kind, kind),
            (ClipKind::Drum { .. }, TrackKind::Drums)
                | (
                    ClipKind::Midi { .. },
                    TrackKind::Bass | TrackKind::Lead | TrackKind::Pad
                )
                | (ClipKind::Audio { .. }, TrackKind::Sampler)
        )
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ArrClip {
    pub clip_id: String,
    pub track: usize,
    pub start_bar: u32,
    pub length_bars: u32,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Project {
    #[serde(default = "default_bpm")]
    pub bpm: f32,
    pub tracks: Vec<Track>,
    pub clips: Vec<Clip>,
    /// `[scene][track] = clip id`
    pub session: Vec<Vec<Option<String>>>,
    #[serde(default)]
    pub arrangement: Vec<ArrClip>,
}

fn default_bpm() -> f32 {
    120.0
}

impl Project {
    pub fn blank() -> Self {
        let tracks = vec![
            Track::new("Drums", TrackKind::Drums),
            Track::new("Bass", TrackKind::Bass),
            Track::new("Lead", TrackKind::Lead),
            Track::new("Pad", TrackKind::Pad),
        ];
        let n = tracks.len();
        Project {
            bpm: 120.0,
            tracks,
            clips: Vec::new(),
            session: vec![vec![None; n]; SCENES],
            arrangement: Vec::new(),
        }
    }

    pub fn ensure_grid(&mut self) {
        let n = self.tracks.len();
        self.session.resize(SCENES, vec![None; n]);
        for row in &mut self.session {
            row.resize(n, None);
        }
    }

    pub fn clip(&self, id: &str) -> Option<&Clip> {
        self.clips.iter().find(|c| c.id == id)
    }

    pub fn clip_mut(&mut self, id: &str) -> Option<&mut Clip> {
        self.clips.iter_mut().find(|c| c.id == id)
    }

    pub fn next_id(&self) -> String {
        let max = self
            .clips
            .iter()
            .filter_map(|c| c.id.strip_prefix('c')?.parse::<u32>().ok())
            .max()
            .unwrap_or(0);
        format!("c{}", max + 1)
    }

    pub fn add_clip(&mut self, name: &str, kind: ClipKind) -> String {
        let id = self.next_id();
        self.clips.push(Clip {
            id: id.clone(),
            name: name.into(),
            kind,
        });
        id
    }

    pub fn to_json(&self) -> Result<String, String> {
        serde_json::to_string_pretty(self).map_err(|e| e.to_string())
    }

    pub fn from_json(s: &str) -> Result<Self, String> {
        let mut p: Project = serde_json::from_str(s).map_err(|e| e.to_string())?;
        p.bpm = if p.bpm.is_finite() {
            p.bpm.clamp(40.0, 240.0)
        } else {
            120.0
        };
        if p.tracks.is_empty() {
            return Err("project has no tracks".into());
        }
        if p.tracks.len() > MAX_TRACKS {
            return Err(format!("too many tracks ({}, max {MAX_TRACKS})", p.tracks.len()));
        }
        if p.clips.len() > MAX_CLIPS {
            return Err(format!("too many clips ({}, max {MAX_CLIPS})", p.clips.len()));
        }
        if p.arrangement.len() > MAX_ARR_CLIPS {
            return Err(format!(
                "too many arrangement clips ({}, max {MAX_ARR_CLIPS})",
                p.arrangement.len()
            ));
        }
        for c in &mut p.clips {
            if let ClipKind::Midi { notes } = &mut c.kind {
                if notes.len() > MAX_NOTES {
                    return Err(format!("clip {} has too many notes", c.id));
                }
                // Hand-edited files: drop notes outside the clip range and
                // keep the rest playable.
                notes.retain(|n| n.pitch <= 127 && n.start < MAX_CLIP_STEPS);
                for n in notes.iter_mut() {
                    n.length = n.length.clamp(1, MAX_CLIP_STEPS - n.start);
                    n.vel = n.vel.min(127);
                }
            }
        }
        for t in &mut p.tracks {
            t.vol = if t.vol.is_finite() { t.vol.clamp(0.0, 1.5) } else { 0.85 };
            t.pan = if t.pan.is_finite() { t.pan.clamp(-1.0, 1.0) } else { 0.0 };
        }
        p.arrangement.retain(|a| a.length_bars > 0 && a.track < p.tracks.len());
        if p.arrangement_bars() > MAX_BARS {
            return Err(format!(
                "arrangement too long ({} bars, max {MAX_BARS})",
                p.arrangement_bars()
            ));
        }
        p.ensure_grid();
        Ok(p)
    }

    pub fn load(path: &Path) -> Result<Self, String> {
        let s = fs::read_to_string(path).map_err(|e| format!("cannot read: {e}"))?;
        Self::from_json(&s)
    }

    pub fn save(&self, path: &Path) -> Result<(), String> {
        if let Some(dir) = path.parent() {
            fs::create_dir_all(dir).map_err(|e| format!("cannot create dir: {e}"))?;
        }
        let mut p = self.clone();
        if let Some(root) = path.parent() {
            relativize_samples(&mut p, root);
        }
        fs::write(path, p.to_json()?).map_err(|e| format!("cannot write: {e}"))
    }

    pub fn arrangement_bars(&self) -> u32 {
        self.arrangement
            .iter()
            .map(|a| a.start_bar + a.length_bars)
            .max()
            .unwrap_or(0)
    }

    /// Remove `[start, end)` from `track`'s lane, trimming or splitting any
    /// clip that overlaps it.
    pub fn clear_range(&mut self, track: usize, start: u32, end: u32) {
        let mut out = Vec::with_capacity(self.arrangement.len() + 1);
        for a in self.arrangement.drain(..) {
            let a_end = a.start_bar + a.length_bars;
            if a.track != track || a_end <= start || a.start_bar >= end {
                out.push(a);
                continue;
            }
            if a.start_bar < start {
                out.push(ArrClip {
                    length_bars: start - a.start_bar,
                    ..a.clone()
                });
            }
            if a_end > end {
                out.push(ArrClip {
                    start_bar: end,
                    length_bars: a_end - end,
                    ..a
                });
            }
        }
        self.arrangement = out;
    }

    /// Fold bars captured by session recording into the arrangement. A
    /// recorded bar replaces whatever the lane held there; consecutive bars
    /// of the same clip grow one region.
    pub fn merge_recorded(&mut self, extra: &[ArrClip]) {
        for a in extra {
            if a.start_bar + a.length_bars > MAX_BARS {
                continue;
            }
            self.clear_range(a.track, a.start_bar, a.start_bar + a.length_bars);
            if let Some(last) = self
                .arrangement
                .iter_mut()
                .rev()
                .find(|x| x.track == a.track && x.clip_id == a.clip_id)
            {
                if last.start_bar + last.length_bars == a.start_bar {
                    last.length_bars += a.length_bars;
                    continue;
                }
            }
            if self.arrangement.len() < MAX_ARR_CLIPS {
                self.arrangement.push(a.clone());
            }
        }
    }

    /// Drop clips nothing refers to (removed cells, unused browser picks).
    pub fn prune_unused_clips(&mut self) {
        let used: std::collections::HashSet<&str> = self
            .session
            .iter()
            .flatten()
            .flatten()
            .map(String::as_str)
            .chain(self.arrangement.iter().map(|a| a.clip_id.as_str()))
            .collect();
        let keep: Vec<bool> = self.clips.iter().map(|c| used.contains(c.id.as_str())).collect();
        let mut i = 0;
        self.clips.retain(|_| {
            i += 1;
            keep[i - 1]
        });
    }

    /// Delete a track along with its session column and arrangement lane.
    pub fn remove_track(&mut self, t: usize) {
        if t >= self.tracks.len() || self.tracks.len() <= 1 {
            return;
        }
        self.tracks.remove(t);
        for row in &mut self.session {
            if t < row.len() {
                row.remove(t);
            }
        }
        self.arrangement.retain(|a| a.track != t);
        for a in &mut self.arrangement {
            if a.track > t {
                a.track -= 1;
            }
        }
        self.ensure_grid();
        self.prune_unused_clips();
    }

    /// Copy clip `id` under a fresh id (for variations).
    pub fn duplicate_clip(&mut self, id: &str) -> Option<String> {
        if self.clips.len() >= MAX_CLIPS {
            return None;
        }
        let src = self.clip(id)?.clone();
        let name = if src.name.ends_with('+') {
            src.name.clone()
        } else {
            format!("{}+", src.name)
        };
        Some(self.add_clip(&name, src.kind))
    }

    /// Absolute sample paths, resolved against the directory the project was
    /// loaded from, so a later save elsewhere still finds them.
    pub fn absolutize_samples(&mut self, project_dir: Option<&Path>) {
        for clip in &mut self.clips {
            if let ClipKind::Audio { path, .. } = &mut clip.kind {
                if !path.is_empty() {
                    *path = resolve_sample_path(path, project_dir)
                        .to_string_lossy()
                        .to_string();
                }
            }
        }
    }
}

/// First `untitled.ttstudio`, `untitled-2.ttstudio`, … that doesn't exist yet.
pub fn fresh_project_path() -> PathBuf {
    let dir = studio_dir();
    let first = dir.join("untitled.ttstudio");
    if !first.exists() {
        return first;
    }
    (2..10_000)
        .map(|i| dir.join(format!("untitled-{i}.ttstudio")))
        .find(|p| !p.exists())
        .unwrap_or(first)
}

fn relativize_samples(p: &mut Project, root: &Path) {
    for clip in &mut p.clips {
        if let ClipKind::Audio { path, .. } = &mut clip.kind {
            let abs = PathBuf::from(&*path);
            if let Ok(rel) = abs.strip_prefix(root) {
                *path = rel.to_string_lossy().to_string();
            }
        }
    }
}

/// Resolve a clip audio path against the project directory (for relative saves).
pub fn resolve_sample_path(stored: &str, project_dir: Option<&Path>) -> PathBuf {
    let p = PathBuf::from(stored);
    if p.is_absolute() {
        p
    } else if let Some(dir) = project_dir {
        dir.join(p)
    } else {
        p
    }
}

pub fn studio_dir() -> PathBuf {
    crate::apps::state_home().join(".texttop_studio")
}

fn empty_drum() -> [[bool; STEPS]; DRUM_VOICES] {
    [[false; STEPS]; DRUM_VOICES]
}

fn hits(pairs: &[(usize, &[usize])]) -> [[bool; STEPS]; DRUM_VOICES] {
    let mut s = empty_drum();
    for (voice, steps) in pairs {
        for &st in *steps {
            if *voice < DRUM_VOICES && st < STEPS {
                s[*voice][st] = true;
            }
        }
    }
    s
}

fn note(pitch: u8, start: u16, length: u16) -> MidiNote {
    MidiNote {
        pitch,
        start,
        length,
        vel: 100,
    }
}

fn four_tracks(bpm: f32) -> Project {
    let mut p = Project::blank();
    p.bpm = bpm;
    p
}

fn place(p: &mut Project, scene: usize, track: usize, id: String) {
    p.ensure_grid();
    if scene < p.session.len() && track < p.session[scene].len() {
        p.session[scene][track] = Some(id);
    }
}

pub fn template_house() -> Project {
    let mut p = four_tracks(124.0);
    p.tracks[2].fx.delay = true;
    p.tracks[3].fx.reverb = true;
    p.tracks[1].fx.pump = true;
    let kick = p.add_clip(
        "Four",
        ClipKind::Drum {
            steps: hits(&[(0, &[0, 4, 8, 12]), (4, &[4, 12]), (2, &[0, 2, 4, 6, 8, 10, 12, 14]), (3, &[2, 10])]),
        },
    );
    let fill = p.add_clip(
        "Fill",
        ClipKind::Drum {
            steps: hits(&[(0, &[0, 4, 8, 10, 12, 14]), (1, &[14]), (4, &[4, 12]), (2, &[0, 4, 8, 12])]),
        },
    );
    let bass = p.add_clip(
        "Offbeat",
        ClipKind::Midi {
            notes: vec![
                note(36, 2, 2),
                note(36, 6, 2),
                note(39, 10, 2),
                note(36, 14, 2),
            ],
        },
    );
    let lead = p.add_clip(
        "Stab",
        ClipKind::Midi {
            notes: vec![note(60, 0, 2), note(63, 0, 2), note(67, 0, 2), note(60, 8, 2)],
        },
    );
    let pad = p.add_clip(
        "Chord",
        ClipKind::Midi {
            notes: vec![note(48, 0, 16), note(51, 0, 16), note(55, 0, 16)],
        },
    );
    place(&mut p, 0, 0, kick.clone());
    place(&mut p, 0, 1, bass.clone());
    place(&mut p, 0, 2, lead.clone());
    place(&mut p, 0, 3, pad.clone());
    place(&mut p, 1, 0, fill);
    place(&mut p, 1, 1, bass);
    place(&mut p, 1, 3, pad);
    p
}

pub fn template_techno() -> Project {
    let mut p = four_tracks(132.0);
    p.tracks[1].fx.filter = true;
    p.tracks[1].fx.filter_amt = 0.35;
    p.tracks[1].fx.pump = true;
    p.tracks[2].fx.delay = true;
    let kick = p.add_clip(
        "Kick",
        ClipKind::Drum {
            steps: hits(&[(0, &[0, 4, 8, 12]), (2, &[0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15]), (5, &[6, 14]), (7, &[4, 12])]),
        },
    );
    let bass = p.add_clip(
        "Acid",
        ClipKind::Midi {
            notes: vec![
                note(36, 0, 1),
                note(36, 3, 1),
                note(39, 4, 1),
                note(36, 6, 1),
                note(41, 8, 1),
                note(36, 11, 1),
                note(39, 12, 1),
                note(43, 14, 1),
            ],
        },
    );
    let lead = p.add_clip(
        "Blip",
        ClipKind::Midi {
            notes: vec![note(72, 4, 1), note(75, 12, 1)],
        },
    );
    let pad = p.add_clip(
        "Drone",
        ClipKind::Midi {
            notes: vec![note(43, 0, 16), note(50, 0, 16)],
        },
    );
    place(&mut p, 0, 0, kick);
    place(&mut p, 0, 1, bass);
    place(&mut p, 0, 2, lead);
    place(&mut p, 0, 3, pad);
    p
}

pub fn template_ambient() -> Project {
    let mut p = four_tracks(90.0);
    p.tracks[2].fx.reverb = true;
    p.tracks[2].fx.delay = true;
    p.tracks[3].fx.reverb = true;
    p.tracks[0].vol = 0.55;
    let perc = p.add_clip(
        "Soft",
        ClipKind::Drum {
            steps: hits(&[(5, &[4, 12]), (2, &[8]), (7, &[0])]),
        },
    );
    let bass = p.add_clip(
        "Sub",
        ClipKind::Midi {
            notes: vec![note(29, 0, 16)],
        },
    );
    let lead = p.add_clip(
        "Bell",
        ClipKind::Midi {
            notes: vec![note(67, 2, 4), note(70, 10, 6)],
        },
    );
    let pad = p.add_clip(
        "Wash",
        ClipKind::Midi {
            notes: vec![note(48, 0, 16), note(53, 0, 16), note(58, 0, 16), note(63, 8, 8)],
        },
    );
    place(&mut p, 0, 0, perc);
    place(&mut p, 0, 1, bass);
    place(&mut p, 0, 2, lead);
    place(&mut p, 0, 3, pad);
    p
}

pub fn template_trap() -> Project {
    let mut p = four_tracks(140.0);
    p.tracks[1].fx.pump = true;
    p.tracks[2].fx.delay = true;
    let drums = p.add_clip(
        "Trap",
        ClipKind::Drum {
            steps: hits(&[
                (0, &[0, 7, 10]),
                (1, &[4, 12]),
                (2, &[0, 2, 3, 4, 6, 8, 10, 11, 12, 14]),
                (4, &[4, 12]),
                (5, &[6]),
            ]),
        },
    );
    let bass = p.add_clip(
        "808",
        ClipKind::Midi {
            notes: vec![note(28, 0, 6), note(28, 8, 3), note(31, 12, 4)],
        },
    );
    let lead = p.add_clip(
        "Pluck",
        ClipKind::Midi {
            notes: vec![note(64, 2, 1), note(67, 6, 1), note(71, 10, 2)],
        },
    );
    let pad = p.add_clip(
        "Air",
        ClipKind::Midi {
            notes: vec![note(52, 0, 16), note(55, 0, 16)],
        },
    );
    place(&mut p, 0, 0, drums);
    place(&mut p, 0, 1, bass);
    place(&mut p, 0, 2, lead);
    place(&mut p, 0, 3, pad);
    p
}

pub fn template_dnb() -> Project {
    let mut p = four_tracks(174.0);
    p.tracks[1].fx.pump = true;
    p.tracks[2].fx.delay = true;
    let drums = p.add_clip(
        "Break",
        ClipKind::Drum {
            steps: hits(&[
                (0, &[0, 10]),
                (1, &[4, 12]),
                (2, &[0, 2, 4, 6, 8, 10, 12, 14]),
                (3, &[6, 14]),
                (7, &[8]),
            ]),
        },
    );
    let bass = p.add_clip(
        "Reese",
        ClipKind::Midi {
            notes: vec![note(33, 0, 4), note(33, 8, 4), note(36, 12, 4)],
        },
    );
    let lead = p.add_clip(
        "Stab",
        ClipKind::Midi {
            notes: vec![note(60, 4, 1), note(63, 4, 1), note(60, 12, 1)],
        },
    );
    let pad = p.add_clip(
        "Pad",
        ClipKind::Midi {
            notes: vec![note(45, 0, 16), note(48, 0, 16)],
        },
    );
    place(&mut p, 0, 0, drums);
    place(&mut p, 0, 1, bass);
    place(&mut p, 0, 2, lead);
    place(&mut p, 0, 3, pad);
    p
}

pub struct TemplateInfo {
    pub name: &'static str,
    pub bpm: u16,
    pub blurb: &'static str,
    pub build: fn() -> Project,
}

pub const TEMPLATES: &[TemplateInfo] = &[
    TemplateInfo {
        name: "House",
        bpm: 124,
        blurb: "four-on-the-floor",
        build: template_house,
    },
    TemplateInfo {
        name: "Techno",
        bpm: 132,
        blurb: "industrial drive",
        build: template_techno,
    },
    TemplateInfo {
        name: "Ambient",
        bpm: 90,
        blurb: "pads & space",
        build: template_ambient,
    },
    TemplateInfo {
        name: "Trap",
        bpm: 140,
        blurb: "808s & hats",
        build: template_trap,
    },
    TemplateInfo {
        name: "DnB",
        bpm: 174,
        blurb: "breaks & sub",
        build: template_dnb,
    },
    TemplateInfo {
        name: "Blank",
        bpm: 120,
        blurb: "empty grid",
        build: Project::blank,
    },
];

pub fn empty_clip_for(kind: TrackKind) -> ClipKind {
    match kind {
        TrackKind::Drums => ClipKind::Drum { steps: empty_drum() },
        TrackKind::Bass | TrackKind::Lead | TrackKind::Pad => ClipKind::Midi { notes: Vec::new() },
        TrackKind::Sampler => ClipKind::Audio {
            path: String::new(),
            looped: true,
        },
    }
}

pub fn preset_clips(kind: TrackKind) -> Vec<(&'static str, ClipKind)> {
    match kind {
        TrackKind::Drums => vec![
            (
                "Four",
                ClipKind::Drum {
                    steps: hits(&[(0, &[0, 4, 8, 12]), (4, &[4, 12]), (2, &[0, 2, 4, 6, 8, 10, 12, 14])]),
                },
            ),
            (
                "Break",
                ClipKind::Drum {
                    steps: hits(&[(0, &[0, 10]), (1, &[4, 12]), (2, &[0, 2, 4, 6, 8, 10, 12, 14])]),
                },
            ),
            (
                "Hats",
                ClipKind::Drum {
                    steps: hits(&[(2, &[0, 2, 4, 6, 8, 10, 12, 14]), (3, &[2, 6, 10, 14])]),
                },
            ),
            ("Empty", empty_clip_for(kind)),
        ],
        TrackKind::Bass => vec![
            (
                "Offbeat",
                ClipKind::Midi {
                    notes: vec![note(36, 2, 2), note(36, 6, 2), note(36, 10, 2), note(36, 14, 2)],
                },
            ),
            (
                "Root",
                ClipKind::Midi {
                    notes: vec![note(36, 0, 4), note(36, 8, 4)],
                },
            ),
            ("Empty", empty_clip_for(kind)),
        ],
        TrackKind::Lead => vec![
            (
                "Stab",
                ClipKind::Midi {
                    notes: vec![note(60, 0, 2), note(67, 8, 2)],
                },
            ),
            (
                "Arp",
                ClipKind::Midi {
                    notes: vec![note(60, 0, 1), note(64, 2, 1), note(67, 4, 1), note(72, 6, 1)],
                },
            ),
            ("Empty", empty_clip_for(kind)),
        ],
        TrackKind::Pad => vec![
            (
                "Minor",
                ClipKind::Midi {
                    notes: vec![note(48, 0, 16), note(51, 0, 16), note(55, 0, 16)],
                },
            ),
            ("Empty", empty_clip_for(kind)),
        ],
        TrackKind::Sampler => vec![("Empty", empty_clip_for(kind))],
    }
}

pub fn is_audio_path(p: &Path) -> bool {
    matches!(
        p.extension()
            .and_then(|e| e.to_str())
            .unwrap_or("")
            .to_ascii_lowercase()
            .as_str(),
        "mp3" | "wav" | "flac" | "ogg"
    )
}

pub fn is_studio_path(p: &Path) -> bool {
    p.extension()
        .and_then(|e| e.to_str())
        .map(|e| e.eq_ignore_ascii_case("ttstudio"))
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn json_roundtrip_house() {
        let p = template_house();
        let j = p.to_json().unwrap();
        let q = Project::from_json(&j).unwrap();
        assert!((p.bpm - q.bpm).abs() < 0.01);
        assert_eq!(p.tracks.len(), q.tracks.len());
        assert_eq!(p.clips.len(), q.clips.len());
        assert_eq!(p.session[0][0], q.session[0][0]);
    }

    #[test]
    fn next_id_increments() {
        let mut p = Project::blank();
        let a = p.add_clip("a", empty_clip_for(TrackKind::Drums));
        let b = p.add_clip("b", empty_clip_for(TrackKind::Drums));
        assert_eq!(a, "c1");
        assert_eq!(b, "c2");
    }

    #[test]
    fn merge_recorded_extends() {
        let mut p = Project::blank();
        p.merge_recorded(&[ArrClip {
            clip_id: "c1".into(),
            track: 0,
            start_bar: 0,
            length_bars: 1,
        }]);
        p.merge_recorded(&[ArrClip {
            clip_id: "c1".into(),
            track: 0,
            start_bar: 1,
            length_bars: 1,
        }]);
        assert_eq!(p.arrangement.len(), 1);
        assert_eq!(p.arrangement[0].length_bars, 2);
    }

    #[test]
    fn recording_overwrites_the_lane() {
        let mut p = Project::blank();
        p.arrangement.push(ArrClip {
            clip_id: "old".into(),
            track: 0,
            start_bar: 0,
            length_bars: 4,
        });
        p.merge_recorded(&[ArrClip {
            clip_id: "new".into(),
            track: 0,
            start_bar: 1,
            length_bars: 1,
        }]);
        let mut lane: Vec<(String, u32, u32)> = p
            .arrangement
            .iter()
            .map(|a| (a.clip_id.clone(), a.start_bar, a.length_bars))
            .collect();
        lane.sort_by_key(|x| x.1);
        assert_eq!(
            lane,
            vec![
                ("old".into(), 0, 1),
                ("new".into(), 1, 1),
                ("old".into(), 2, 2)
            ]
        );
    }

    #[test]
    fn remove_track_shifts_lanes_and_columns() {
        let mut p = template_house();
        let bass_clip = p.session[0][1].clone();
        p.arrangement.push(ArrClip {
            clip_id: bass_clip.clone().unwrap(),
            track: 1,
            start_bar: 0,
            length_bars: 1,
        });
        p.arrangement.push(ArrClip {
            clip_id: p.session[0][2].clone().unwrap(),
            track: 2,
            start_bar: 0,
            length_bars: 1,
        });
        p.remove_track(1);
        assert_eq!(p.tracks.len(), 3);
        assert!(p.session.iter().all(|r| r.len() == 3));
        assert_eq!(p.arrangement.len(), 1);
        assert_eq!(p.arrangement[0].track, 1);
        // the bass clip is referenced nowhere now
        assert!(p.clip(&bass_clip.unwrap()).is_none());
    }

    #[test]
    fn midi_length_is_bounded() {
        let j = r#"{"bpm":120,"tracks":[{"name":"B","kind":"bass"}],
            "clips":[{"id":"c1","name":"x","kind":{"type":"midi","notes":[
              {"pitch":36,"start":65000,"length":9},
              {"pitch":36,"start":60,"length":60000}]}}],
            "session":[]}"#;
        let p = Project::from_json(j).unwrap();
        let c = &p.clips[0];
        assert_eq!(c.length_steps(), MAX_CLIP_STEPS);
        match &c.kind {
            ClipKind::Midi { notes } => assert_eq!(notes.len(), 1),
            _ => unreachable!(),
        }
    }

    #[test]
    fn huge_arrangement_is_rejected() {
        let mut p = Project::blank();
        p.arrangement.push(ArrClip {
            clip_id: "c1".into(),
            track: 0,
            start_bar: 0,
            length_bars: 10_000,
        });
        let j = p.to_json().unwrap();
        let err = Project::from_json(&j).unwrap_err();
        assert!(err.contains("too long"), "{err}");
    }
}
