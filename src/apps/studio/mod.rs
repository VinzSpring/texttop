//! Studio — CapCut-simple electronic DAW (session grid + arrange + piano roll).

mod engine;
mod project;

use std::cell::Cell;
use std::path::{Path, PathBuf};
use std::sync::atomic::Ordering;
use std::sync::mpsc::{self, Receiver};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use ratatui::crossterm::event::{
    KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind,
};
use ratatui::layout::Rect;
use ratatui::style::Color;
use ratatui::widgets::Clear;
use ratatui::Frame;
use unicode_width::UnicodeWidthStr;

use super::{Action, AppWindow, Launch};
use engine::{Audition, Cmd, Engine};
use project::{
    empty_clip_for, fresh_project_path, is_audio_path, is_studio_path, preset_clips, studio_dir,
    ArrClip, ClipKind, MidiNote, Project, TrackKind, DRUM_NAMES, DRUM_VOICES, MAX_ARR_CLIPS,
    MAX_BARS, MAX_CLIPS, MAX_CLIP_STEPS, MAX_TRACKS, SCENES, STEPS, TEMPLATES,
};

const BG: Color = Color::Rgb(12, 14, 22);
const FG: Color = Color::Rgb(220, 224, 236);
const DIM: Color = Color::Rgb(130, 138, 160);
const ACCENT: Color = Color::Rgb(0, 190, 255);
const SEL: Color = Color::Rgb(255, 220, 90);
const PLAY: Color = Color::Rgb(110, 230, 140);
const REC: Color = Color::Rgb(255, 80, 90);
const PANEL: Color = Color::Rgb(22, 26, 40);
const BAR_BG: Color = Color::Rgb(16, 18, 28);
const MIX_BG: Color = Color::Rgb(14, 16, 26);
const TRANS_BG: Color = Color::Rgb(18, 20, 32);
const POP_BG: Color = Color::Rgb(28, 24, 48);

/// Piano-roll rows (two octaves).
const PIANO_ROWS: u16 = 24;
/// Arrange lane width per bar, in cells.
const BAR_W: u16 = 3;
/// Status messages stay up this many ticks before the hint returns.
const MSG_TICKS: u16 = 120;
/// A destructive key must be pressed again within this many ticks.
const CONFIRM_TICKS: u16 = 60;

const TRACK_KINDS: [(TrackKind, &str); 5] = [
    (TrackKind::Drums, "Drums"),
    (TrackKind::Bass, "Bass"),
    (TrackKind::Lead, "Lead"),
    (TrackKind::Pad, "Pad"),
    (TrackKind::Sampler, "Sampler"),
];

#[derive(Clone, Copy, PartialEq, Eq)]
enum View {
    Home,
    Session,
    Arrange,
    Piano,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Palette {
    /// Preset clips for an empty session cell.
    Clip { scene: usize, track: usize },
    /// Kind of track to add.
    NewTrack,
}

/// Buttons in the header that act on the current selection.
#[derive(Clone, Copy)]
enum Act {
    LaunchCell,
    StopTrack,
    NewClip,
    Duplicate,
    ClearCell,
    LaunchScene,
    AddTrack,
    MuteTrack,
    DeleteTrack,
    PlaceArr,
    ShorterArr,
    LongerArr,
    DeleteArr,
    ShorterNote,
    LongerNote,
    OctaveUp,
    OctaveDown,
    Loop,
    Back,
}

#[derive(Clone)]
enum Hit {
    Template(usize),
    View(View),
    Home,
    Play,
    Stop,
    Record,
    BpmDown,
    BpmUp,
    Export,
    Save,
    Scene(usize),
    Cell { scene: usize, track: usize },
    TrackHead(usize),
    AddTrack,
    Mute(usize),
    Solo(usize),
    Vol(usize),
    Pan(usize),
    Fx { track: usize, which: u8 },
    PaletteItem(usize),
    ArrClip(usize),
    ArrEmpty { track: usize, bar: u32 },
    PianoCell { row: u16, step: u16 },
    Browser(usize),
    Act(Act),
}

/// Arrange lane geometry from the last render (for drag math).
#[derive(Clone, Copy, Default)]
struct ArrGeom {
    lane_x: u16,
    top_y: u16,
    row_h: u16,
    tracks: usize,
}

thread_local! {
    /// Drawing is clipped to the window being rendered.
    static CLIP: Cell<Rect> = const { Cell::new(Rect::ZERO) };
}

fn clip_rect() -> Rect {
    CLIP.with(|c| c.get())
}

pub struct Studio {
    engine: Engine,
    path: Option<PathBuf>,
    view: View,
    /// View to return to when leaving the template screen.
    back_view: Option<View>,
    dirty: bool,
    status: String,
    message_ticks: u16,
    confirm: Option<(&'static str, u16)>,
    sel_scene: usize,
    sel_track: usize,
    selected_clip: Option<String>,
    /// Browser preset waiting for an empty cell: (track kind, preset index).
    armed: Option<(TrackKind, usize)>,
    palette: Option<Palette>,
    pal_sel: usize,
    piano_row: u16,
    piano_step: u16,
    piano_base: u8,
    piano_off: u16,
    /// Note being stretched by a mouse drag: (pitch, start step).
    piano_drag: Option<(u8, u16)>,
    sel_arr: Option<usize>,
    arr_off: u32,
    arr_geom: ArrGeom,
    /// Arrangement clip being dragged and the bar offset it was grabbed at.
    drag_arr: Option<(usize, u32)>,
    hits: Vec<(Rect, Hit)>,
    export_rx: Option<Receiver<Result<PathBuf, String>>>,
}

impl Studio {
    pub fn new(path: Option<PathBuf>) -> Self {
        let mut status = None;
        let (project, path, view) = match path {
            Some(p) if p.exists() => match open_project(&p) {
                Ok(proj) => (proj, Some(p), View::Session),
                // Keep the unreadable file untouched: don't make it the save target.
                Err(e) => {
                    status = Some(format!("cannot open {}: {e}", p.display()));
                    (Project::blank(), None, View::Home)
                }
            },
            Some(p) if is_studio_path(&p) => (Project::blank(), Some(p), View::Home),
            _ => (Project::blank(), None, View::Home),
        };
        let engine = Engine::start(project.clone());
        let status = status
            .or_else(|| engine.error.clone())
            .unwrap_or_else(|| "pick a template to start".into());
        let mut s = Studio {
            engine,
            path: None,
            view,
            back_view: None,
            dirty: false,
            status,
            message_ticks: MSG_TICKS,
            confirm: None,
            sel_scene: 0,
            sel_track: 0,
            selected_clip: None,
            armed: None,
            palette: None,
            pal_sel: 0,
            piano_row: 12,
            piano_step: 0,
            piano_base: 48,
            piano_off: 0,
            piano_drag: None,
            sel_arr: None,
            arr_off: 0,
            arr_geom: ArrGeom::default(),
            drag_arr: None,
            hits: Vec::new(),
            export_rx: None,
        };
        s.install(project, path);
        s
    }

    fn project(&self) -> std::sync::MutexGuard<'_, Project> {
        self.engine
            .shared
            .project
            .lock()
            .unwrap_or_else(|e| e.into_inner())
    }

    fn with_project<R>(&mut self, f: impl FnOnce(&mut Project) -> R) -> R {
        let r = {
            let mut p = self.project();
            f(&mut p)
        };
        self.engine.shared.bump();
        self.dirty = true;
        r
    }

    fn say(&mut self, s: impl Into<String>) {
        self.status = s.into();
        self.message_ticks = MSG_TICKS;
    }

    /// True when `tag` was armed by a previous press; otherwise arms it and
    /// shows `warning`.
    fn confirmed(&mut self, tag: &'static str, warning: &str) -> bool {
        if matches!(self.confirm, Some((t, _)) if t == tag) {
            self.confirm = None;
            return true;
        }
        self.confirm = Some((tag, CONFIRM_TICKS));
        self.say(warning.to_string());
        false
    }

    /// Replace the whole project (template, open, drop). Stops the transport
    /// so no clip ids from the old project keep playing.
    fn install(&mut self, project: Project, path: Option<PathBuf>) {
        self.stop();
        let samples = engine::load_project_samples(&project, None);
        let missing = project
            .clips
            .iter()
            .filter(|c| matches!(&c.kind, ClipKind::Audio { path, .. } if !path.is_empty() && !samples.contains_key(path)))
            .count();
        self.engine
            .shared
            .bpm_x100
            .store((project.bpm * 100.0) as u32, Ordering::Relaxed);
        *self.project() = project;
        if let Ok(mut map) = self.engine.shared.samples.lock() {
            *map = samples;
        }
        self.engine.shared.bump();
        self.path = path;
        self.dirty = false;
        self.sel_scene = 0;
        self.sel_track = 0;
        self.selected_clip = None;
        self.armed = None;
        self.sel_arr = None;
        self.arr_off = 0;
        self.palette = None;
        if missing > 0 {
            self.say(format!("{missing} sample(s) not found — keep samples next to the project"));
        }
    }

    fn apply_template(&mut self, i: usize) {
        let Some(t) = TEMPLATES.get(i) else { return };
        if self.dirty && !self.confirmed("template", "unsaved changes — pick again to replace them")
        {
            return;
        }
        self.install((t.build)(), None);
        // A template is a new song: it must not overwrite the file it replaced.
        self.dirty = true;
        self.view = View::Session;
        self.back_view = None;
        self.say(format!("{} — tap clips to launch them, ▶ plays everything", t.name));
    }

    fn set_view(&mut self, v: View) {
        if v == View::Piano {
            self.open_piano_for_selection();
            return;
        }
        self.view = v;
        // Piano keeps whichever playback mode was running.
        if v != View::Home {
            self.engine
                .shared
                .arrange_mode
                .store(v == View::Arrange, Ordering::Relaxed);
        }
    }

    fn toggle_play(&mut self) {
        let sh = &self.engine.shared;
        if sh.playing.load(Ordering::Relaxed) {
            sh.playing.store(false, Ordering::Relaxed);
            sh.push(Cmd::Pause);
        } else {
            sh.playing.store(true, Ordering::Relaxed);
            sh.push(Cmd::Play);
        }
    }

    fn ensure_playing(&mut self) {
        let sh = &self.engine.shared;
        if !sh.playing.load(Ordering::Relaxed) {
            sh.playing.store(true, Ordering::Relaxed);
            sh.push(Cmd::Play);
        }
    }

    fn stop(&mut self) {
        let sh = &self.engine.shared;
        sh.playing.store(false, Ordering::Relaxed);
        sh.recording.store(false, Ordering::Relaxed);
        sh.push(Cmd::Stop);
        if let Ok(mut a) = sh.active.lock() {
            a.iter_mut().for_each(|c| *c = None);
        }
    }

    fn toggle_record(&mut self) {
        let sh = &self.engine.shared;
        let on = !sh.recording.load(Ordering::Relaxed);
        sh.recording.store(on, Ordering::Relaxed);
        if on && !sh.playing.load(Ordering::Relaxed) {
            sh.playing.store(true, Ordering::Relaxed);
            sh.push(Cmd::Play);
        }
        self.say(if on {
            "recording launched clips into Arrange"
        } else {
            "record off"
        });
    }

    fn cell(&self, scene: usize, track: usize) -> Option<String> {
        self.project()
            .session
            .get(scene)
            .and_then(|r| r.get(track))
            .cloned()
            .flatten()
    }

    fn is_launched(&self, track: usize, id: &str) -> bool {
        self.engine.shared.playing.load(Ordering::Relaxed)
            && self
                .engine
                .shared
                .active
                .lock()
                .ok()
                .and_then(|a| a.get(track).cloned().flatten())
                .is_some_and(|a| a == id)
    }

    fn launch_cell(&mut self, scene: usize, track: usize) {
        let clip = self.cell(scene, track);
        self.engine.shared.push(Cmd::Launch { track, clip });
        self.ensure_playing();
        self.sel_scene = scene;
        self.sel_track = track;
    }

    fn stop_track(&mut self, track: usize) {
        self.engine.shared.push(Cmd::Launch { track, clip: None });
    }

    fn launch_scene(&mut self, scene: usize) {
        self.engine.shared.push(Cmd::LaunchScene(scene));
        self.ensure_playing();
        self.sel_scene = scene;
    }

    fn select_cell(&mut self, scene: usize, track: usize) {
        self.sel_scene = scene;
        self.sel_track = track;
        if let Some(id) = self.cell(scene, track) {
            self.selected_clip = Some(id);
        }
    }

    fn clip_room(&mut self) -> bool {
        if self.project().clips.len() >= MAX_CLIPS {
            self.say(format!("clip limit reached ({MAX_CLIPS}) — save to tidy up unused clips"));
            return false;
        }
        true
    }

    fn place_preset(&mut self, scene: usize, track: usize, preset: usize) {
        let Some(kind) = self.project().tracks.get(track).map(|t| t.kind) else {
            return;
        };
        let presets = preset_clips(kind);
        let Some((name, kind_clip)) = presets.get(preset).cloned() else {
            return;
        };
        if !self.clip_room() {
            return;
        }
        let id = self.with_project(|p| {
            let id = p.add_clip(name, kind_clip);
            p.ensure_grid();
            if let Some(cell) = p.session.get_mut(scene).and_then(|r| r.get_mut(track)) {
                *cell = Some(id.clone());
            }
            id
        });
        self.sel_scene = scene;
        self.sel_track = track;
        self.selected_clip = Some(id);
        self.palette = None;
        self.say(format!("placed {name} — click it to launch"));
    }

    fn new_empty_clip(&mut self, scene: usize, track: usize) {
        let Some(kind) = self.project().tracks.get(track).map(|t| t.kind) else {
            return;
        };
        if !self.clip_room() {
            return;
        }
        let id = self.with_project(|p| {
            let id = p.add_clip("New", empty_clip_for(kind));
            p.ensure_grid();
            if let Some(cell) = p.session.get_mut(scene).and_then(|r| r.get_mut(track)) {
                *cell = Some(id.clone());
            }
            id
        });
        self.selected_clip = Some(id);
        self.say("empty clip — 3 opens the piano roll");
    }

    /// Copy the selected cell's clip into the next empty scene of its track.
    fn duplicate_cell(&mut self) {
        let (sc, tr) = (self.sel_scene, self.sel_track);
        let Some(id) = self.cell(sc, tr) else {
            self.say("nothing to duplicate here");
            return;
        };
        let target = (sc + 1..SCENES).find(|&s| self.cell(s, tr).is_none());
        let Some(target) = target else {
            self.say("no empty scene below");
            return;
        };
        if !self.clip_room() {
            return;
        }
        let new_id = self.with_project(|p| {
            let new_id = p.duplicate_clip(&id)?;
            p.session[target][tr] = Some(new_id.clone());
            Some(new_id)
        });
        if let Some(new_id) = new_id {
            self.sel_scene = target;
            self.selected_clip = Some(new_id);
            self.say(format!("duplicated to S{}", target + 1));
        }
    }

    fn clear_cell(&mut self, scene: usize, track: usize) {
        let removed = self.cell(scene, track);
        self.with_project(|p| {
            if let Some(cell) = p.session.get_mut(scene).and_then(|r| r.get_mut(track)) {
                *cell = None;
            }
        });
        if removed.is_some() && removed == self.selected_clip {
            self.selected_clip = None;
        }
    }

    fn add_track(&mut self, kind: TrackKind) {
        let n = self.project().tracks.len();
        if n >= MAX_TRACKS {
            self.say(format!("track limit reached ({MAX_TRACKS})"));
            return;
        }
        let name = TRACK_KINDS
            .iter()
            .find(|(k, _)| *k == kind)
            .map(|(_, n)| *n)
            .unwrap_or("Track");
        let t = self.with_project(|p| {
            p.tracks.push(project::Track::new(name, kind));
            p.ensure_grid();
            p.tracks.len() - 1
        });
        self.sel_track = t;
        self.palette = None;
        self.say(format!("added {name} track"));
    }

    fn delete_track(&mut self) {
        let t = self.sel_track;
        let (n, name) = {
            let p = self.project();
            (
                p.tracks.len(),
                p.tracks.get(t).map(|t| t.name.clone()).unwrap_or_default(),
            )
        };
        if n <= 1 {
            self.say("a project needs at least one track");
            return;
        }
        if !self.confirmed("deltrack", &format!("delete track {name}? click 🗑 again")) {
            return;
        }
        self.stop_track(t);
        self.with_project(|p| p.remove_track(t));
        self.sel_track = t.min(n - 2);
        self.sel_arr = None;
        self.selected_clip = None;
        self.say(format!("deleted {name}"));
    }

    fn selected_clip_id(&self) -> Option<String> {
        if let Some(id) = &self.selected_clip {
            if self.project().clip(id).is_some() {
                return Some(id.clone());
            }
        }
        self.cell(self.sel_scene, self.sel_track)
    }

    fn nudge_bpm(&mut self, d: f32) {
        let bpm = self.with_project(|p| {
            p.bpm = (p.bpm + d).clamp(40.0, 240.0);
            p.bpm
        });
        self.engine
            .shared
            .bpm_x100
            .store((bpm * 100.0) as u32, Ordering::Relaxed);
    }

    fn nudge_track(&mut self, t: usize, f: impl FnOnce(&mut project::Track)) {
        self.with_project(|p| {
            if let Some(tr) = p.tracks.get_mut(t) {
                f(tr);
            }
        });
    }

    fn save(&mut self) {
        let dest = self.path.clone().unwrap_or_else(fresh_project_path);
        // Tidy clips no cell or lane refers to so the file stays within limits.
        let res = {
            let mut p = self.project();
            p.prune_unused_clips();
            p.save(&dest)
        };
        self.engine.shared.bump();
        match res {
            Ok(()) => {
                self.path = Some(dest.clone());
                self.dirty = false;
                self.say(format!("saved {}", dest.display()));
            }
            Err(e) => self.say(e),
        }
    }

    fn export(&mut self) {
        if self.export_rx.is_some() {
            self.say("export already running…");
            return;
        }
        let dir = self
            .path
            .as_ref()
            .and_then(|p| p.parent().map(|d| d.to_path_buf()))
            .unwrap_or_else(studio_dir);
        if let Err(e) = std::fs::create_dir_all(&dir) {
            self.say(format!("cannot create {}: {e}", dir.display()));
            return;
        }
        let stem = self
            .path
            .as_ref()
            .and_then(|p| p.file_stem())
            .map(|s| s.to_string_lossy().to_string())
            .unwrap_or_else(|| "studio".into());
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        let dest = dir.join(format!("{stem}-{stamp}.wav"));
        let project = self.project().clone();
        let live = self.engine.shared.samples.lock().ok().map(|g| g.clone());
        let (tx, rx) = mpsc::channel();
        // Rendering minutes of audio takes a moment — keep the UI live.
        std::thread::spawn(move || {
            let _ = tx.send(engine::export_wav(&project, &dest, None, live.as_ref()));
        });
        self.export_rx = Some(rx);
        self.say("exporting WAV…");
    }

    fn open_piano_for_selection(&mut self) {
        if let Some(id) = self.selected_clip_id() {
            self.selected_clip = Some(id);
            self.view = View::Piano;
            self.piano_off = 0;
            self.piano_drag = None;
        } else {
            self.say("select a clip first (click a cell)");
        }
    }

    fn drop_audio(&mut self, path: PathBuf) {
        let data = match engine::load_sample(&path) {
            Ok(d) => d,
            Err(e) => return self.say(e),
        };
        if !self.clip_room() {
            return;
        }
        let key = path.to_string_lossy().to_string();
        if let Ok(mut map) = self.engine.shared.samples.lock() {
            map.insert(key.clone(), Arc::new(data));
        }
        let name = path
            .file_stem()
            .map(|s| s.to_string_lossy().to_string())
            .unwrap_or_else(|| "sample".into());
        let (sel_scene, sel_track) = (self.sel_scene, self.sel_track);
        let placed = self.with_project(|p| {
            let empty_in = |p: &Project, t: usize| (0..SCENES).find(|&s| p.session[s][t].is_none());
            // The selected sampler cell, else any sampler track with room,
            // else a new sampler track.
            let sel_ok = p.tracks.get(sel_track).map(|t| t.kind) == Some(TrackKind::Sampler);
            let spot = if sel_ok && p.session[sel_scene][sel_track].is_none() {
                Some((sel_scene, sel_track))
            } else {
                (0..p.tracks.len())
                    .filter(|&t| p.tracks[t].kind == TrackKind::Sampler)
                    .find_map(|t| empty_in(p, t).map(|s| (s, t)))
            };
            let (scene, t) = match spot {
                Some(s) => s,
                None if p.tracks.len() < MAX_TRACKS => {
                    p.tracks.push(project::Track::new(&name, TrackKind::Sampler));
                    p.ensure_grid();
                    (0, p.tracks.len() - 1)
                }
                None => return None,
            };
            let id = p.add_clip(
                &name,
                ClipKind::Audio {
                    path: key,
                    looped: true,
                },
            );
            p.session[scene][t] = Some(id.clone());
            Some((scene, t, id))
        });
        match placed {
            Some((scene, t, id)) => {
                self.sel_scene = scene;
                self.sel_track = t;
                self.selected_clip = Some(id);
                self.say(format!("dropped {name} into S{} — click it to launch", scene + 1));
            }
            None => self.say("no free sampler cell and track limit reached"),
        }
    }

    fn piano_view_steps(&self, clip_len: u16) -> u16 {
        let cursor = (self.piano_step + 1).div_ceil(STEPS as u16) * STEPS as u16;
        clip_len.max(cursor).clamp(STEPS as u16, MAX_CLIP_STEPS)
    }

    fn row_pitch(&self, row: u16) -> u8 {
        self.piano_base
            .saturating_add(PIANO_ROWS as u8 - 1)
            .saturating_sub(row as u8)
            .min(127)
    }

    /// Which track should voice an audition of `id`.
    fn track_for_clip(&self, id: &str) -> Option<usize> {
        let p = self.project();
        let fits = |t: usize| {
            p.tracks
                .get(t)
                .zip(p.clip(id))
                .is_some_and(|(tr, c)| c.fits(tr.kind))
        };
        if fits(self.sel_track) {
            return Some(self.sel_track);
        }
        (0..p.tracks.len()).find(|&t| {
            fits(t)
                && (p.session.iter().any(|r| r.get(t).and_then(|c| c.as_deref()) == Some(id))
                    || p.arrangement.iter().any(|a| a.track == t && a.clip_id == id))
        })
    }

    fn audition(&mut self, note: Audition) {
        if let Some(id) = self.selected_clip_id() {
            if let Some(track) = self.track_for_clip(&id) {
                self.engine.shared.push(Cmd::Audition { track, note });
            }
        }
    }

    /// Toggle a drum step or a note starting at (row, step). Returns true when
    /// a MIDI note was created (so a drag can stretch it).
    fn toggle_piano_cell(&mut self, row: u16, step: u16) -> bool {
        let Some(id) = self.selected_clip_id() else {
            return false;
        };
        let pitch = self.row_pitch(row);
        let mut created = None;
        self.with_project(|p| {
            let Some(clip) = p.clip_mut(&id) else { return };
            match &mut clip.kind {
                ClipKind::Drum { steps } => {
                    let (v, s) = (row as usize, step as usize);
                    if v < DRUM_VOICES && s < STEPS {
                        steps[v][s] = !steps[v][s];
                        if steps[v][s] {
                            created = Some(Audition::Drum(v));
                        }
                    }
                }
                ClipKind::Midi { notes } => {
                    if step >= MAX_CLIP_STEPS {
                        return;
                    }
                    if let Some(i) = notes.iter().position(|n| n.pitch == pitch && n.start == step)
                    {
                        notes.remove(i);
                    } else if notes.len() < project::MAX_NOTES {
                        // Clicking inside a held note shortens it to end here.
                        for n in notes.iter_mut() {
                            if n.pitch == pitch && n.start < step && step < n.start + n.length {
                                n.length = step - n.start;
                            }
                        }
                        notes.push(MidiNote {
                            pitch,
                            start: step,
                            length: 1,
                            vel: 100,
                        });
                        created = Some(Audition::Pitch(pitch));
                    }
                }
                ClipKind::Audio { .. } => {}
            }
        });
        self.piano_row = row;
        self.piano_step = step;
        match created {
            Some(a) => {
                self.audition(a);
                matches!(a, Audition::Pitch(_))
            }
            None => false,
        }
    }

    /// Set the length of the note covering the cursor (or starting there).
    fn resize_note_at_cursor(&mut self, delta: i32) {
        let Some(id) = self.selected_clip_id() else { return };
        let pitch = self.row_pitch(self.piano_row);
        let step = self.piano_step;
        self.with_project(|p| {
            if let Some(ClipKind::Midi { notes }) = p.clip_mut(&id).map(|c| &mut c.kind) {
                if let Some(n) = notes
                    .iter_mut()
                    .find(|n| n.pitch == pitch && n.start <= step && step < n.start + n.length)
                {
                    let max = MAX_CLIP_STEPS - n.start;
                    n.length = (n.length as i32 + delta).clamp(1, max as i32) as u16;
                }
            }
        });
    }

    fn stretch_note(&mut self, pitch: u8, start: u16, to_step: u16) {
        let Some(id) = self.selected_clip_id() else { return };
        let len = to_step.saturating_sub(start) + 1;
        self.with_project(|p| {
            if let Some(ClipKind::Midi { notes }) = p.clip_mut(&id).map(|c| &mut c.kind) {
                if let Some(n) = notes.iter_mut().find(|n| n.pitch == pitch && n.start == start) {
                    n.length = len.clamp(1, MAX_CLIP_STEPS - start);
                }
            }
        });
    }

    fn toggle_audio_loop(&mut self) {
        let Some(id) = self.selected_clip_id() else { return };
        let mut now = None;
        self.with_project(|p| {
            if let Some(ClipKind::Audio { looped, .. }) = p.clip_mut(&id).map(|c| &mut c.kind) {
                *looped = !*looped;
                now = Some(*looped);
            }
        });
        if let Some(l) = now {
            self.say(if l { "sample loops" } else { "sample plays once per bar" });
        }
    }

    fn sync_recorded(&mut self) {
        let extra = {
            let Ok(mut rec) = self.engine.shared.recorded.lock() else {
                return;
            };
            if rec.is_empty() {
                return;
            }
            rec.drain(..).collect::<Vec<_>>()
        };
        self.with_project(|p| p.merge_recorded(&extra));
    }

    // ── arrangement editing ────────────────────────────────────────────────

    fn arr_bar_at(&self, col: u16) -> u32 {
        self.arr_off + col.saturating_sub(self.arr_geom.lane_x) as u32 / BAR_W as u32
    }

    fn arr_track_at(&self, row: u16) -> Option<usize> {
        let g = self.arr_geom;
        if row < g.top_y || g.row_h == 0 {
            return None;
        }
        let t = ((row - g.top_y) / g.row_h) as usize;
        (t < g.tracks).then_some(t)
    }

    fn place_in_arrangement(&mut self, track: usize, bar: u32) {
        let Some(id) = self.selected_clip_id() else {
            self.say("select a clip in Session first, then click a lane");
            return;
        };
        let (fits, bars, count) = {
            let p = self.project();
            let fits = p
                .clip(&id)
                .zip(p.tracks.get(track))
                .is_some_and(|(c, t)| c.fits(t.kind));
            let bars = p.clip(&id).map(|c| c.bars()).unwrap_or(1);
            (fits, bars, p.arrangement.len())
        };
        if !fits {
            self.sel_track = track;
            self.say("that clip doesn't fit this track — pick one from this track's column");
            return;
        }
        if count >= MAX_ARR_CLIPS || bar + bars > MAX_BARS {
            self.say("arrangement is full");
            return;
        }
        let i = self.with_project(|p| {
            p.clear_range(track, bar, bar + bars);
            p.arrangement.push(ArrClip {
                clip_id: id,
                track,
                start_bar: bar,
                length_bars: bars,
            });
            p.arrangement.len() - 1
        });
        self.sel_arr = Some(i);
        self.sel_track = track;
    }

    /// Re-seat arrangement clip `i` so it overwrites whatever it now overlaps.
    fn settle_arr(&mut self, i: usize) {
        let new_i = self.with_project(|p| {
            if i >= p.arrangement.len() {
                return None;
            }
            let a = p.arrangement.remove(i);
            p.clear_range(a.track, a.start_bar, a.start_bar + a.length_bars);
            p.arrangement.push(a);
            Some(p.arrangement.len() - 1)
        });
        self.sel_arr = new_i;
    }

    fn move_arr(&mut self, i: usize, start: u32, track: Option<usize>) {
        self.with_project(|p| {
            let kind = track.and_then(|t| p.tracks.get(t)).map(|t| t.kind);
            let fits = |p: &Project, a: &ArrClip, k: TrackKind| {
                p.clip(&a.clip_id).is_some_and(|c| c.fits(k))
            };
            let Some(a) = p.arrangement.get(i).cloned() else { return };
            let start = start.min(MAX_BARS.saturating_sub(a.length_bars));
            let new_track = match (track, kind) {
                (Some(t), Some(k)) if fits(p, &a, k) => t,
                _ => a.track,
            };
            if let Some(a) = p.arrangement.get_mut(i) {
                a.start_bar = start;
                a.track = new_track;
            }
        });
    }

    fn resize_arr(&mut self, i: usize, delta: i32) {
        self.with_project(|p| {
            if let Some(a) = p.arrangement.get_mut(i) {
                let max = MAX_BARS.saturating_sub(a.start_bar).max(1);
                a.length_bars = (a.length_bars as i32 + delta).clamp(1, max as i32) as u32;
            }
        });
        self.settle_arr(i);
    }

    fn delete_arr(&mut self, i: usize) {
        self.with_project(|p| {
            if i < p.arrangement.len() {
                p.arrangement.remove(i);
            }
        });
        self.sel_arr = None;
        self.drag_arr = None;
    }

    fn poll_export(&mut self) {
        let Some(rx) = &self.export_rx else { return };
        match rx.try_recv() {
            Ok(res) => {
                self.export_rx = None;
                match res {
                    Ok(p) => self.say(format!("exported {}", p.display())),
                    Err(e) => self.say(format!("export failed: {e}")),
                }
            }
            Err(mpsc::TryRecvError::Empty) => {}
            Err(mpsc::TryRecvError::Disconnected) => {
                self.export_rx = None;
                self.say("export failed");
            }
        }
    }

    fn push_hit(&mut self, r: Rect, h: Hit) {
        let r = r.intersection(clip_rect());
        if r.width > 0 && r.height > 0 {
            self.hits.push((r, h));
        }
    }
}

fn open_project(p: &Path) -> Result<Project, String> {
    if !is_studio_path(p) {
        return Err("not a .ttstudio project".into());
    }
    let mut proj = Project::load(p)?;
    proj.absolutize_samples(p.parent());
    Ok(proj)
}

fn fill(f: &mut Frame, r: Rect, bg: Color) {
    let r = r.intersection(clip_rect());
    let buf = f.buffer_mut();
    let r = r.intersection(buf.area);
    for y in r.y..r.y + r.height {
        for x in r.x..r.x + r.width {
            if let Some(c) = buf.cell_mut((x, y)) {
                c.reset();
                c.set_bg(bg);
                c.set_fg(FG);
                c.set_symbol(" ");
            }
        }
    }
}

fn put(f: &mut Frame, x: u16, y: u16, text: &str, fg: Color, bg: Color) {
    let clip = clip_rect().intersection(f.buffer_mut().area);
    if y < clip.y || y >= clip.y + clip.height {
        return;
    }
    let buf = f.buffer_mut();
    let right = clip.x + clip.width;
    let mut cx = x;
    for ch in text.chars() {
        let w = UnicodeWidthStr::width(ch.encode_utf8(&mut [0; 4])).max(1) as u16;
        if cx + w > right {
            break;
        }
        if cx >= clip.x {
            if let Some(c) = buf.cell_mut((cx, y)) {
                c.set_symbol(&ch.to_string());
                c.set_fg(fg);
                c.set_bg(bg);
            }
        }
        cx = cx.saturating_add(w);
    }
}

fn clip_label(s: &str, w: usize) -> String {
    if s.width() <= w {
        format!("{s:<w$}")
    } else {
        let mut out = String::new();
        for ch in s.chars() {
            if out.width() + UnicodeWidthStr::width(ch.encode_utf8(&mut [0; 4])) >= w {
                break;
            }
            out.push(ch);
        }
        if w > 0 {
            out.push('…');
        }
        format!("{out:<w$}")
    }
}

fn rgb(t: (u8, u8, u8)) -> Color {
    Color::Rgb(t.0, t.1, t.2)
}

fn dim_rgb(t: (u8, u8, u8), f: f32) -> Color {
    Color::Rgb(
        (t.0 as f32 * f) as u8,
        (t.1 as f32 * f) as u8,
        (t.2 as f32 * f) as u8,
    )
}

fn digit_index(c: char) -> Option<usize> {
    c.to_digit(10)?.checked_sub(1).map(|d| d as usize)
}

impl AppWindow for Studio {
    fn title(&self) -> String {
        let name = self
            .path
            .as_ref()
            .and_then(|p| p.file_name())
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_else(|| "untitled".into());
        if self.dirty {
            format!("Studio · {name} •")
        } else {
            format!("Studio · {name}")
        }
    }

    fn render(&mut self, f: &mut Frame, area: Rect, _focused: bool) {
        self.hits.clear();
        CLIP.with(|c| c.set(area));
        if area.width < 20 || area.height < 8 {
            fill(f, area, BG);
            put(f, area.x, area.y, "Studio: window too small", DIM, BG);
            return;
        }
        fill(f, area, BG);

        if self.view == View::Home {
            self.render_home(f, area);
            return;
        }

        let header_h = 1u16;
        let trans_h = 1u16;
        let mix_h = 3u16;
        let body_y = area.y + header_h;
        let body_h = area.height.saturating_sub(header_h + trans_h + mix_h);
        let trans_y = area.y + area.height.saturating_sub(mix_h + trans_h);
        let mix_y = area.y + area.height.saturating_sub(mix_h);

        self.render_header(f, Rect::new(area.x, area.y, area.width, header_h));
        let body = Rect::new(area.x, body_y, area.width, body_h);
        match self.view {
            View::Session => self.render_session(f, body),
            View::Arrange => self.render_arrange(f, body),
            View::Piano => self.render_piano(f, body),
            View::Home => {}
        }
        self.render_transport(f, Rect::new(area.x, trans_y, area.width, trans_h));
        self.render_mixer(f, Rect::new(area.x, mix_y, area.width, mix_h));

        if self.palette.is_some() {
            self.render_palette(f, area);
        }
    }

    fn on_key(&mut self, k: KeyEvent) -> Action {
        if let Some(pal) = self.palette {
            let n = self.palette_items(pal).len();
            match k.code {
                KeyCode::Esc => self.palette = None,
                KeyCode::Up => self.pal_sel = self.pal_sel.saturating_sub(1),
                KeyCode::Down => self.pal_sel = (self.pal_sel + 1).min(n.saturating_sub(1)),
                KeyCode::Enter => self.pick_palette(self.pal_sel),
                KeyCode::Char(c) => {
                    if let Some(i) = digit_index(c) {
                        self.pick_palette(i);
                    }
                }
                _ => {}
            }
            return Action::None;
        }
        if self.view == View::Home {
            match k.code {
                KeyCode::Char(c) if c.is_ascii_digit() => {
                    if let Some(i) = digit_index(c) {
                        self.apply_template(i);
                    }
                }
                KeyCode::Enter => self.apply_template(0),
                KeyCode::Esc => {
                    if let Some(v) = self.back_view.take() {
                        self.view = v;
                    }
                }
                KeyCode::Char('q') => return self.close(),
                _ => {}
            }
            return Action::None;
        }

        let ctrl = k.modifiers.contains(KeyModifiers::CONTROL);
        match k.code {
            KeyCode::Char('q') if !ctrl => return self.close(),
            KeyCode::Esc => {
                if self.view == View::Piano {
                    self.view = if self.engine.shared.arrange_mode.load(Ordering::Relaxed) {
                        View::Arrange
                    } else {
                        View::Session
                    };
                } else if self.view == View::Arrange {
                    self.sel_arr = None;
                } else if self.armed.take().is_some() {
                    self.say("preset disarmed");
                }
            }
            KeyCode::Char(' ') => self.toggle_play(),
            KeyCode::Char('r') | KeyCode::Char('R') if !ctrl => self.toggle_record(),
            KeyCode::Char('s') | KeyCode::Char('S') => self.save(),
            KeyCode::Char('e') | KeyCode::Char('E') => self.export(),
            KeyCode::Char('t') | KeyCode::Char('T') => {
                self.back_view = Some(self.view);
                self.view = View::Home;
            }
            KeyCode::Char('1') => self.set_view(View::Session),
            KeyCode::Char('2') => self.set_view(View::Arrange),
            KeyCode::Char('3') => self.set_view(View::Piano),
            KeyCode::Tab => {
                let next = match self.view {
                    View::Session => View::Arrange,
                    View::Arrange => View::Piano,
                    View::Piano | View::Home => View::Session,
                };
                self.set_view(next);
                if next == View::Piano && self.view != View::Piano {
                    self.set_view(View::Session);
                }
            }
            KeyCode::Char('+') | KeyCode::Char('=') => self.nudge_bpm(1.0),
            KeyCode::Char('-') | KeyCode::Char('_') => self.nudge_bpm(-1.0),
            KeyCode::Home => self.stop(),
            KeyCode::Char('a') | KeyCode::Char('A') => {
                self.palette = Some(Palette::NewTrack);
                self.pal_sel = 0;
            }
            KeyCode::Char('x') | KeyCode::Char('X') if self.view != View::Piano => {
                self.delete_track()
            }
            KeyCode::Char('m') | KeyCode::Char('M') if self.view != View::Piano => {
                let t = self.sel_track;
                self.nudge_track(t, |tr| tr.mute = !tr.mute);
            }
            _ => match self.view {
                View::Session => self.session_key(k),
                View::Arrange => self.arrange_key(k),
                View::Piano => self.piano_key(k),
                View::Home => {}
            },
        }
        Action::None
    }

    fn on_mouse(&mut self, m: MouseEvent, area: Rect) -> Action {
        let (col, row) = (m.column, m.row);
        if col < area.x || row < area.y {
            return Action::None;
        }
        let hit = self
            .hits
            .iter()
            .rev()
            .find(|(r, _)| contains(*r, col, row))
            .map(|(_, h)| h.clone());
        match m.kind {
            MouseEventKind::Down(MouseButton::Left) => {
                if self.palette.is_some() && !matches!(hit, Some(Hit::PaletteItem(_))) {
                    self.palette = None;
                    return Action::None;
                }
                if let Some(h) = hit {
                    self.handle_hit(h, col);
                }
            }
            MouseEventKind::Down(MouseButton::Right) => match hit {
                Some(Hit::ArrClip(i)) => self.delete_arr(i),
                Some(Hit::Cell { scene, track }) => self.clear_cell(scene, track),
                _ => {}
            },
            MouseEventKind::Drag(MouseButton::Left) => {
                if let Some((i, grab)) = self.drag_arr {
                    let start = self.arr_bar_at(col).saturating_sub(grab);
                    let track = self.arr_track_at(row);
                    self.move_arr(i, start, track);
                } else if let Some((pitch, start)) = self.piano_drag {
                    if let Some(Hit::PianoCell { step, .. }) = hit {
                        if step >= start {
                            self.stretch_note(pitch, start, step);
                        }
                    }
                }
            }
            MouseEventKind::Up(MouseButton::Left) => {
                if let Some((i, _)) = self.drag_arr.take() {
                    self.settle_arr(i);
                }
                self.piano_drag = None;
            }
            MouseEventKind::ScrollDown | MouseEventKind::ScrollUp => {
                let up = matches!(m.kind, MouseEventKind::ScrollUp);
                let d = if up { 1.0 } else { -1.0 };
                match hit {
                    Some(Hit::Vol(t)) => {
                        self.nudge_track(t, |tr| tr.vol = (tr.vol + 0.05 * d).clamp(0.0, 1.2))
                    }
                    Some(Hit::Pan(t)) => {
                        self.nudge_track(t, |tr| tr.pan = (tr.pan + 0.1 * d).clamp(-1.0, 1.0))
                    }
                    Some(Hit::Fx { track, which: 0 }) => self.nudge_track(track, |tr| {
                        tr.fx.filter = true;
                        tr.fx.filter_amt = (tr.fx.filter_amt + 0.05 * d).clamp(0.0, 1.0);
                    }),
                    Some(Hit::BpmUp) | Some(Hit::BpmDown) => self.nudge_bpm(d),
                    _ => match self.view {
                        View::Piano => {
                            self.piano_base = if up {
                                (self.piano_base + 1).min(127 - PIANO_ROWS as u8 + 1)
                            } else {
                                self.piano_base.saturating_sub(1)
                            };
                        }
                        View::Arrange => {
                            self.arr_off = if up {
                                self.arr_off.saturating_sub(1)
                            } else {
                                (self.arr_off + 1).min(MAX_BARS - 1)
                            };
                        }
                        _ => {}
                    },
                }
            }
            _ => {}
        }
        Action::None
    }

    fn on_tick(&mut self) {
        self.sync_recorded();
        self.poll_export();
        if self.message_ticks > 0 {
            self.message_ticks -= 1;
        }
        if let Some((_, t)) = &mut self.confirm {
            *t = t.saturating_sub(1);
            if *t == 0 {
                self.confirm = None;
            }
        }
    }

    fn accepts_drop(&self) -> bool {
        true
    }

    fn on_drop(&mut self, paths: &[PathBuf], _cut: bool) -> Action {
        if let Some(p) = paths.iter().find(|p| is_studio_path(p)) {
            if self.dirty && !self.confirmed("open", "unsaved changes — drop again to replace them")
            {
                return Action::None;
            }
            match open_project(p) {
                Ok(proj) => {
                    self.install(proj, Some(p.clone()));
                    self.view = View::Session;
                    self.back_view = None;
                    if self.message_ticks == 0 || !self.status.contains("not found") {
                        self.say(format!("opened {}", p.display()));
                    }
                }
                Err(e) => self.say(format!("cannot open {}: {e}", p.display())),
            }
            return Action::None;
        }
        let audio: Vec<&PathBuf> = paths.iter().filter(|p| is_audio_path(p)).collect();
        if audio.is_empty() {
            self.say("Studio takes .ttstudio projects and mp3/wav/flac/ogg samples");
        }
        for p in audio {
            self.drop_audio(p.clone());
        }
        if self.view == View::Home {
            self.view = View::Session;
        }
        Action::None
    }

    fn workspace_path(&self) -> Option<PathBuf> {
        self.path
            .as_ref()
            .and_then(|p| p.parent().map(|d| d.to_path_buf()))
    }

    fn session_launch(&self) -> Option<Launch> {
        Some(Launch::Studio(self.path.clone()))
    }
}

impl Studio {
    fn close(&mut self) -> Action {
        if self.dirty && !self.confirmed("quit", "unsaved changes — close again to discard them, or 💾 save first")
        {
            return Action::None;
        }
        self.stop();
        Action::Close
    }

    fn palette_items(&self, pal: Palette) -> Vec<String> {
        match pal {
            Palette::Clip { track, .. } => {
                let kind = self
                    .project()
                    .tracks
                    .get(track)
                    .map(|t| t.kind)
                    .unwrap_or(TrackKind::Drums);
                preset_clips(kind).into_iter().map(|(n, _)| n.to_string()).collect()
            }
            Palette::NewTrack => TRACK_KINDS.iter().map(|(_, n)| n.to_string()).collect(),
        }
    }

    fn pick_palette(&mut self, i: usize) {
        match self.palette {
            Some(Palette::Clip { scene, track }) => self.place_preset(scene, track, i),
            Some(Palette::NewTrack) => {
                if let Some((kind, _)) = TRACK_KINDS.get(i) {
                    self.add_track(*kind);
                }
            }
            None => {}
        }
    }

    fn session_key(&mut self, k: KeyEvent) {
        let n = self.project().tracks.len();
        match k.code {
            KeyCode::Enter => {
                let (sc, tr) = (self.sel_scene, self.sel_track);
                match self.cell(sc, tr) {
                    Some(id) if self.is_launched(tr, &id) => self.stop_track(tr),
                    Some(_) => self.launch_cell(sc, tr),
                    None => self.fill_empty_cell(sc, tr),
                }
            }
            KeyCode::Char('.') => self.stop_track(self.sel_track),
            KeyCode::Delete | KeyCode::Backspace => self.clear_cell(self.sel_scene, self.sel_track),
            KeyCode::Char('n') | KeyCode::Char('N') => {
                self.new_empty_clip(self.sel_scene, self.sel_track)
            }
            KeyCode::Char('d') | KeyCode::Char('D') => self.duplicate_cell(),
            KeyCode::Char('l') | KeyCode::Char('L') => self.launch_scene(self.sel_scene),
            KeyCode::Char('[') => {
                let t = self.sel_track;
                self.nudge_track(t, |tr| tr.vol = (tr.vol - 0.05).clamp(0.0, 1.2));
            }
            KeyCode::Char(']') => {
                let t = self.sel_track;
                self.nudge_track(t, |tr| tr.vol = (tr.vol + 0.05).clamp(0.0, 1.2));
            }
            KeyCode::Left => self.select_cell(self.sel_scene, self.sel_track.saturating_sub(1)),
            KeyCode::Right if n > 0 => {
                self.select_cell(self.sel_scene, (self.sel_track + 1).min(n - 1))
            }
            KeyCode::Up => self.select_cell(self.sel_scene.saturating_sub(1), self.sel_track),
            KeyCode::Down => self.select_cell((self.sel_scene + 1).min(SCENES - 1), self.sel_track),
            _ => {}
        }
    }

    /// An empty cell takes the armed browser preset, or opens the palette.
    fn fill_empty_cell(&mut self, scene: usize, track: usize) {
        let kind = self.project().tracks.get(track).map(|t| t.kind);
        match (self.armed, kind) {
            (Some((ak, preset)), Some(k)) if ak == k => {
                self.place_preset(scene, track, preset);
            }
            _ => {
                self.sel_scene = scene;
                self.sel_track = track;
                self.palette = Some(Palette::Clip { scene, track });
                self.pal_sel = 0;
            }
        }
    }

    fn arrange_key(&mut self, k: KeyEvent) {
        let n = self.project().tracks.len();
        match (k.code, self.sel_arr) {
            (KeyCode::Delete | KeyCode::Backspace, Some(i)) => self.delete_arr(i),
            (KeyCode::Char('['), Some(i)) => self.resize_arr(i, -1),
            (KeyCode::Char(']'), Some(i)) => self.resize_arr(i, 1),
            (KeyCode::Left, Some(i)) => {
                let start = self.project().arrangement.get(i).map(|a| a.start_bar);
                if let Some(s) = start {
                    self.move_arr(i, s.saturating_sub(1), None);
                    self.settle_arr(i);
                }
            }
            (KeyCode::Right, Some(i)) => {
                let start = self.project().arrangement.get(i).map(|a| a.start_bar);
                if let Some(s) = start {
                    self.move_arr(i, s + 1, None);
                    self.settle_arr(i);
                }
            }
            (KeyCode::Left, None) => self.arr_off = self.arr_off.saturating_sub(4),
            (KeyCode::Right, None) => self.arr_off = (self.arr_off + 4).min(MAX_BARS - 1),
            (KeyCode::Up, _) => {
                self.sel_track = self.sel_track.saturating_sub(1);
                self.sel_arr = None;
            }
            (KeyCode::Down, _) if n > 0 => {
                self.sel_track = (self.sel_track + 1).min(n - 1);
                self.sel_arr = None;
            }
            (KeyCode::Enter, _) => {
                let bar = self.engine.shared.bar.load(Ordering::Relaxed);
                self.place_in_arrangement(self.sel_track, bar.max(self.arr_off));
            }
            _ => {}
        }
    }

    fn piano_key(&mut self, k: KeyEvent) {
        let is_drum = self
            .selected_clip_id()
            .and_then(|id| self.project().clip(&id).map(|c| matches!(c.kind, ClipKind::Drum { .. })))
            .unwrap_or(false);
        let max_row = if is_drum { DRUM_VOICES as u16 - 1 } else { PIANO_ROWS - 1 };
        let max_step = if is_drum { STEPS as u16 - 1 } else { MAX_CLIP_STEPS - 1 };
        match k.code {
            KeyCode::Enter => {
                self.toggle_piano_cell(self.piano_row, self.piano_step);
            }
            KeyCode::Char('[') => self.resize_note_at_cursor(-1),
            KeyCode::Char(']') => self.resize_note_at_cursor(1),
            KeyCode::Char('l') | KeyCode::Char('L') => self.toggle_audio_loop(),
            KeyCode::Left => self.piano_step = self.piano_step.saturating_sub(1),
            KeyCode::Right => self.piano_step = (self.piano_step + 1).min(max_step),
            KeyCode::Up => {
                if self.piano_row == 0 && !is_drum {
                    self.piano_base = (self.piano_base + 1).min(127 - PIANO_ROWS as u8 + 1);
                } else {
                    self.piano_row = self.piano_row.saturating_sub(1);
                }
            }
            KeyCode::Down => {
                if self.piano_row >= max_row && !is_drum {
                    self.piano_base = self.piano_base.saturating_sub(1);
                } else {
                    self.piano_row = (self.piano_row + 1).min(max_row);
                }
            }
            KeyCode::PageUp if !is_drum => {
                self.piano_base = (self.piano_base + 12).min(127 - PIANO_ROWS as u8 + 1)
            }
            KeyCode::PageDown if !is_drum => self.piano_base = self.piano_base.saturating_sub(12),
            _ => {}
        }
        self.piano_row = self.piano_row.min(max_row);
    }

    fn handle_hit(&mut self, h: Hit, col: u16) {
        match h {
            Hit::Act(a) => self.run_act(a),
            Hit::Template(i) => self.apply_template(i),
            Hit::View(v) => self.set_view(v),
            Hit::Home => {
                self.back_view = Some(self.view);
                self.view = View::Home;
            }
            Hit::Play => self.toggle_play(),
            Hit::Stop => self.stop(),
            Hit::Record => self.toggle_record(),
            Hit::BpmDown => self.nudge_bpm(-1.0),
            Hit::BpmUp => self.nudge_bpm(1.0),
            Hit::Export => self.export(),
            Hit::Save => self.save(),
            Hit::Scene(s) => self.launch_scene(s),
            Hit::Cell { scene, track } => match self.cell(scene, track) {
                // Clicking the playing clip stops that track.
                Some(id) if self.is_launched(track, &id) => {
                    self.selected_clip = Some(id);
                    self.sel_scene = scene;
                    self.sel_track = track;
                    self.stop_track(track);
                }
                Some(id) => {
                    self.selected_clip = Some(id);
                    self.launch_cell(scene, track);
                }
                None => self.fill_empty_cell(scene, track),
            },
            Hit::TrackHead(t) => self.sel_track = t,
            Hit::AddTrack => {
                self.palette = Some(Palette::NewTrack);
                self.pal_sel = 0;
            }
            Hit::Mute(t) => self.nudge_track(t, |tr| tr.mute = !tr.mute),
            Hit::Solo(t) => self.nudge_track(t, |tr| tr.solo = !tr.solo),
            Hit::Vol(t) => {
                let rect = self
                    .hits
                    .iter()
                    .rev()
                    .find_map(|(r, h)| matches!(h, Hit::Vol(i) if *i == t).then_some(*r));
                if let Some(r) = rect {
                    let frac = (col.saturating_sub(r.x) + 1) as f32 / r.width.max(1) as f32;
                    let vol = (frac * 1.2).clamp(0.05, 1.2);
                    self.nudge_track(t, |tr| tr.vol = vol);
                }
            }
            Hit::Pan(t) => {
                self.sel_track = t;
                self.say("scroll over a track name to pan it");
            }
            Hit::Fx { track, which } => self.nudge_track(track, |tr| match which {
                0 => tr.fx.filter = !tr.fx.filter,
                1 => tr.fx.delay = !tr.fx.delay,
                2 => tr.fx.reverb = !tr.fx.reverb,
                3 => tr.fx.pump = !tr.fx.pump,
                _ => {}
            }),
            Hit::PaletteItem(i) => self.pick_palette(i),
            Hit::ArrClip(i) => {
                let info = self
                    .project()
                    .arrangement
                    .get(i)
                    .map(|a| (a.clip_id.clone(), a.track, a.start_bar));
                if let Some((id, track, start)) = info {
                    let grab = self.arr_bar_at(col).saturating_sub(start);
                    self.drag_arr = Some((i, grab));
                    self.sel_arr = Some(i);
                    self.selected_clip = Some(id);
                    self.sel_track = track;
                }
            }
            Hit::ArrEmpty { track, bar } => {
                self.sel_arr = None;
                self.place_in_arrangement(track, bar);
            }
            Hit::PianoCell { row, step } => {
                if self.toggle_piano_cell(row, step) {
                    self.piano_drag = Some((self.row_pitch(row), step));
                }
            }
            Hit::Browser(i) => {
                let kind = self.project().tracks.get(self.sel_track).map(|t| t.kind);
                if let Some(kind) = kind {
                    if let Some((name, _)) = preset_clips(kind).get(i) {
                        if self.armed == Some((kind, i)) {
                            self.armed = None;
                            self.say("preset disarmed");
                        } else {
                            self.armed = Some((kind, i));
                            self.say(format!("{name} armed — click empty cells in this column"));
                        }
                    }
                }
            }
        }
    }

    fn render_home(&mut self, f: &mut Frame, area: Rect) {
        put(
            f,
            area.x + 2,
            area.y,
            " Studio  ·  tap a genre to start  ·  as simple as CapCut ",
            ACCENT,
            BG,
        );
        put(
            f,
            area.x + 2,
            area.y + 1,
            " session clips · piano roll · arrange · one-tap FX · WAV export ",
            DIM,
            BG,
        );
        let cols = 3u16;
        let gap = 2u16;
        let tw = (area.width.saturating_sub(4 + (cols - 1) * gap) / cols).max(16);
        let th = 5u16;
        const COLORS: [(u8, u8, u8); 6] = [
            (40, 18, 70),
            (18, 28, 70),
            (18, 50, 48),
            (70, 22, 40),
            (20, 40, 70),
            (28, 28, 36),
        ];
        for (i, t) in TEMPLATES.iter().enumerate() {
            let c = (i as u16) % cols;
            let r = (i as u16) / cols;
            let x = area.x + 2 + c * (tw + gap);
            let y = area.y + 3 + r * (th + 1);
            let rect = Rect::new(x, y, tw, th);
            let bg = rgb(COLORS[i % COLORS.len()]);
            fill(f, rect, bg);
            put(f, x + 1, y + 1, &format!("{} {}", i + 1, t.name), FG, bg);
            put(f, x + 1, y + 2, &format!("{} BPM", t.bpm), ACCENT, bg);
            put(f, x + 1, y + 3, t.blurb, DIM, bg);
            self.push_hit(rect, Hit::Template(i));
        }
        let msg = if self.message_ticks > 0 {
            self.status.clone()
        } else {
            "click a template · or drop a .ttstudio project or a sample".to_string()
        };
        let y = area.y + area.height.saturating_sub(1);
        let mut x = area.x + 2;
        if self.back_view.is_some() {
            x += self.act_button(f, x, y, "◀ Back", Act::Back);
        }
        put(
            f,
            x,
            y,
            &clip_label(&msg, (area.x + area.width).saturating_sub(x + 2) as usize),
            if self.confirm.is_some() { SEL } else { DIM },
            BG,
        );
    }

    fn render_header(&mut self, f: &mut Frame, r: Rect) {
        fill(f, r, BAR_BG);
        let mut x = r.x + 1;
        put(f, x, r.y, " ⌂ ", DIM, BAR_BG);
        self.push_hit(Rect::new(x, r.y, 3, 1), Hit::Home);
        x += 4;
        let tabs = [
            (View::Session, "Session"),
            (View::Arrange, "Arrange"),
            (View::Piano, "Piano"),
        ];
        for (v, label) in tabs {
            let on = self.view == v;
            let fg = if on { SEL } else { DIM };
            let w = label.width() as u16 + 2;
            put(f, x, r.y, &format!(" {label} "), fg, BAR_BG);
            self.push_hit(Rect::new(x, r.y, w, 1), Hit::View(v));
            x += w + 1;
        }
        let bpm = self.engine.shared.bpm_x100.load(Ordering::Relaxed) as f32 / 100.0;
        let bpm_s = format!("{bpm:.0} BPM");
        put(f, x + 1, r.y, &bpm_s, ACCENT, BAR_BG);
        x += bpm_s.width() as u16 + 3;
        // what can be done with the selection, as buttons
        let end = r.x + r.width;
        for (label, act) in self.actions() {
            let w = label.width() as u16 + 2;
            if x + w + 1 > end {
                break;
            }
            x += self.act_button(f, x, r.y, label, act) + 1;
        }
        if self.message_ticks > 0 {
            let fg = if self.confirm.is_some() { SEL } else { FG };
            let room = end.saturating_sub(x + 1) as usize;
            put(f, x + 1, r.y, &clip_label(&self.status, room), fg, BAR_BG);
        }
    }

    /// One header button; returns its width.
    fn act_button(&mut self, f: &mut Frame, x: u16, y: u16, label: &str, act: Act) -> u16 {
        let w = label.width() as u16 + 2;
        put(f, x, y, &format!(" {label} "), FG, PANEL);
        self.push_hit(Rect::new(x, y, w, 1), Hit::Act(act));
        w
    }

    /// Header actions for the current view and selection.
    fn actions(&self) -> Vec<(&'static str, Act)> {
        match self.view {
            View::Session => {
                let (sc, tr) = (self.sel_scene, self.sel_track);
                let mut v = Vec::new();
                match self.cell(sc, tr) {
                    Some(id) if self.is_launched(tr, &id) => v.push(("■ Stop clip", Act::StopTrack)),
                    Some(_) => {
                        v.push(("▶ Launch", Act::LaunchCell));
                        v.push(("⧉ Duplicate", Act::Duplicate));
                        v.push(("✕ Clear", Act::ClearCell));
                    }
                    None => v.push(("+ New clip", Act::NewClip)),
                }
                v.push(("▶ Scene", Act::LaunchScene));
                v.push(("+ Track", Act::AddTrack));
                v.push(("🔇 Mute", Act::MuteTrack));
                v.push(("🗑 Track", Act::DeleteTrack));
                v
            }
            View::Arrange => {
                if self.sel_arr.is_some() {
                    vec![
                        ("◂ Shorter", Act::ShorterArr),
                        ("Longer ▸", Act::LongerArr),
                        ("✕ Remove", Act::DeleteArr),
                    ]
                } else {
                    vec![("+ Place clip", Act::PlaceArr), ("+ Track", Act::AddTrack)]
                }
            }
            View::Piano => {
                let mut v = vec![
                    ("◂ Note", Act::ShorterNote),
                    ("Note ▸", Act::LongerNote),
                    ("Oct ▲", Act::OctaveUp),
                    ("Oct ▼", Act::OctaveDown),
                    ("⟲ Loop", Act::Loop),
                ];
                if self.back_view.is_some() {
                    v.push(("◀ Back", Act::Back));
                }
                v
            }
            View::Home => Vec::new(),
        }
    }

    fn run_act(&mut self, a: Act) {
        let (sc, tr) = (self.sel_scene, self.sel_track);
        match a {
            Act::LaunchCell => self.launch_cell(sc, tr),
            Act::StopTrack => self.stop_track(tr),
            Act::NewClip => self.new_empty_clip(sc, tr),
            Act::Duplicate => self.duplicate_cell(),
            Act::ClearCell => self.clear_cell(sc, tr),
            Act::LaunchScene => self.launch_scene(sc),
            Act::AddTrack => {
                self.palette = Some(Palette::NewTrack);
                self.pal_sel = 0;
            }
            Act::MuteTrack => self.nudge_track(tr, |t| t.mute = !t.mute),
            Act::DeleteTrack => self.delete_track(),
            Act::PlaceArr => {
                let bar = self.engine.shared.bar.load(Ordering::Relaxed);
                self.place_in_arrangement(tr, bar.max(self.arr_off));
            }
            Act::ShorterArr | Act::LongerArr | Act::DeleteArr => {
                if let Some(i) = self.sel_arr {
                    match a {
                        Act::ShorterArr => self.resize_arr(i, -1),
                        Act::LongerArr => self.resize_arr(i, 1),
                        _ => self.delete_arr(i),
                    }
                }
            }
            Act::ShorterNote => self.resize_note_at_cursor(-1),
            Act::LongerNote => self.resize_note_at_cursor(1),
            Act::OctaveUp => {
                self.piano_base = (self.piano_base + 12).min(127 - PIANO_ROWS as u8 + 1)
            }
            Act::OctaveDown => self.piano_base = self.piano_base.saturating_sub(12),
            Act::Loop => self.toggle_audio_loop(),
            Act::Back => {
                if let Some(v) = self.back_view.take() {
                    self.view = v;
                }
            }
        }
    }

    fn render_session(&mut self, f: &mut Frame, area: Rect) {
        let p = self.project().clone();
        let n = p.tracks.len().max(1);
        let label_w = 4u16;

        let brow_w = 16u16.min(area.width / 5).max(12);
        let grid = Rect::new(
            area.x + brow_w,
            area.y,
            area.width.saturating_sub(brow_w),
            area.height,
        );
        self.render_browser(f, Rect::new(area.x, area.y, brow_w, area.height), &p);

        let add_w = if p.tracks.len() < MAX_TRACKS { 4 } else { 0 };
        let col_w = (grid.width.saturating_sub(label_w + add_w) / n as u16).max(8);
        let row_h = if area.height > 18 { 2u16 } else { 1u16 };

        for (ti, tr) in p.tracks.iter().enumerate() {
            let x = grid.x + label_w + ti as u16 * col_w;
            let sel = ti == self.sel_track;
            let label = format!("{}{}", if sel { "▸" } else { " " }, tr.name);
            let fg = if tr.mute { DIM } else { rgb(tr.kind.color()) };
            put(f, x, grid.y, &clip_label(&label, col_w.saturating_sub(1) as usize), fg, BG);
            self.push_hit(Rect::new(x, grid.y, col_w, 1), Hit::TrackHead(ti));
        }
        if add_w > 0 {
            let x = grid.x + label_w + p.tracks.len() as u16 * col_w;
            put(f, x, grid.y, " + ", ACCENT, PANEL);
            self.push_hit(Rect::new(x, grid.y, 3, 1), Hit::AddTrack);
        }

        let active = self
            .engine
            .shared
            .active
            .lock()
            .ok()
            .map(|a| a.clone())
            .unwrap_or_default();
        let playing = self.engine.shared.playing.load(Ordering::Relaxed);

        for scene in 0..SCENES {
            let y = grid.y + 1 + scene as u16 * row_h;
            if y >= grid.y + grid.height {
                break;
            }
            let scene_sel = scene == self.sel_scene;
            put(
                f,
                grid.x,
                y,
                &format!("▶S{}", scene + 1),
                if scene_sel { SEL } else { DIM },
                BG,
            );
            self.push_hit(Rect::new(grid.x, y, label_w, row_h), Hit::Scene(scene));
            for (ti, tr) in p.tracks.iter().enumerate() {
                let x = grid.x + label_w + ti as u16 * col_w;
                let cell = p.session.get(scene).and_then(|r| r.get(ti)).and_then(|c| c.as_ref());
                let selected = scene_sel && ti == self.sel_track;
                let launched =
                    playing && cell.is_some() && active.get(ti).and_then(|a| a.as_ref()) == cell;
                let col = tr.kind.color();
                let bg = match (cell, launched, selected) {
                    (Some(_), true, _) => rgb(col),
                    (Some(_), false, true) => dim_rgb(col, 0.55),
                    (Some(_), false, false) => dim_rgb(col, 0.32),
                    (None, _, true) => Color::Rgb(36, 40, 58),
                    (None, _, false) => PANEL,
                };
                let fg = if launched { Color::Rgb(8, 10, 16) } else { FG };
                let name = match cell {
                    Some(id) => {
                        let nm = p.clip(id).map(|c| c.name.as_str()).unwrap_or("?");
                        format!("{}{nm}", if launched { "▶ " } else { "" })
                    }
                    None => {
                        let armed = self.armed.is_some_and(|(k, _)| k == tr.kind);
                        if armed { " ✚" } else { "  +" }.to_string()
                    }
                };
                let rect = Rect::new(x, y, col_w.saturating_sub(1), row_h);
                fill(f, rect, bg);
                put(f, x, y, &clip_label(&name, col_w.saturating_sub(1) as usize), fg, bg);
                self.push_hit(rect, Hit::Cell { scene, track: ti });
            }
        }
    }

    fn render_browser(&mut self, f: &mut Frame, r: Rect, p: &Project) {
        fill(f, r, BAR_BG);
        let track = p.tracks.get(self.sel_track);
        let kind = track.map(|t| t.kind).unwrap_or(TrackKind::Drums);
        let title = track
            .map(|t| format!("{} clips", t.name))
            .unwrap_or_else(|| "clips".into());
        put(f, r.x + 1, r.y, &clip_label(&title, r.width.saturating_sub(2) as usize), ACCENT, BAR_BG);
        for (i, (name, _)) in preset_clips(kind).iter().enumerate() {
            let y = r.y + 2 + i as u16;
            if y >= r.y + r.height.saturating_sub(2) {
                break;
            }
            let armed = self.armed == Some((kind, i));
            let (fg, bg) = if armed { (BG, SEL) } else { (FG, BAR_BG) };
            put(
                f,
                r.x + 1,
                y,
                &clip_label(&format!("{} {name}", if armed { "✚" } else { "·" }), r.width.saturating_sub(2) as usize),
                fg,
                bg,
            );
            self.push_hit(Rect::new(r.x, y, r.width, 1), Hit::Browser(i));
        }
        put(f, r.x + 1, r.y + r.height.saturating_sub(1), "drop wav/mp3", DIM, BAR_BG);
    }

    fn render_arrange(&mut self, f: &mut Frame, area: Rect) {
        let p = self.project().clone();
        let name_w = 8u16;
        let lane_x = area.x + name_w;
        let visible = (area.width.saturating_sub(name_w) / BAR_W) as u32;
        let play_bar = self.engine.shared.bar.load(Ordering::Relaxed);
        let playing = self.engine.shared.playing.load(Ordering::Relaxed);
        let arr_mode = self.engine.shared.arrange_active.load(Ordering::Relaxed);
        // Follow the playhead.
        if playing && arr_mode && visible > 0 {
            if play_bar < self.arr_off || play_bar >= self.arr_off + visible {
                self.arr_off = play_bar.saturating_sub(1);
            }
        }
        let row_h = if area.height > (p.tracks.len() as u16 * 2 + 2) { 2 } else { 1 };
        let top_y = area.y + 1;
        self.arr_geom = ArrGeom {
            lane_x,
            top_y,
            row_h,
            tracks: p.tracks.len(),
        };

        // ruler
        for b in 0..visible {
            let bar = self.arr_off + b;
            if bar >= MAX_BARS {
                break;
            }
            if bar % 4 == 0 {
                put(f, lane_x + b as u16 * BAR_W, area.y, &format!("{}", bar + 1), DIM, BG);
            }
        }
        if p.arrangement.is_empty() {
            // Below the lanes, so the ruler stays readable.
            let y = top_y + p.tracks.len() as u16 * row_h + 1;
            if y < area.y + area.height {
                put(
                    f,
                    lane_x,
                    y,
                    "empty — select a clip in Session, then click a lane (or R records a jam)",
                    DIM,
                    BG,
                );
            }
        }

        for (ti, tr) in p.tracks.iter().enumerate() {
            let y = top_y + ti as u16 * row_h;
            if y >= area.y + area.height {
                break;
            }
            let sel = ti == self.sel_track;
            put(
                f,
                area.x,
                y,
                &clip_label(&format!("{}{}", if sel { "▸" } else { " " }, tr.name), name_w as usize - 1),
                rgb(tr.kind.color()),
                BG,
            );
            for b in 0..visible {
                let bar = self.arr_off + b;
                if bar >= MAX_BARS {
                    break;
                }
                let x = lane_x + b as u16 * BAR_W;
                let rect = Rect::new(x, y, BAR_W, row_h);
                let bg = if playing && arr_mode && bar == play_bar {
                    Color::Rgb(40, 48, 36)
                } else if bar % 4 == 0 {
                    Color::Rgb(20, 22, 32)
                } else {
                    PANEL
                };
                fill(f, rect, bg);
                self.push_hit(rect, Hit::ArrEmpty { track: ti, bar });
            }
        }
        for (i, a) in p.arrangement.iter().enumerate() {
            if a.track >= p.tracks.len() {
                continue;
            }
            let end = a.start_bar + a.length_bars;
            if end <= self.arr_off || a.start_bar >= self.arr_off + visible {
                continue;
            }
            let vis_start = a.start_bar.max(self.arr_off);
            let vis_end = end.min(self.arr_off + visible);
            let y = top_y + a.track as u16 * row_h;
            let x = lane_x + ((vis_start - self.arr_off) as u16) * BAR_W;
            let w = ((vis_end - vis_start) as u16 * BAR_W).saturating_sub(1).max(1);
            let col = p.tracks[a.track].kind.color();
            let sel = self.sel_arr == Some(i);
            let bg = if sel { rgb(col) } else { dim_rgb(col, 0.55) };
            let fg = if sel { Color::Rgb(8, 10, 16) } else { FG };
            let rect = Rect::new(x, y, w, row_h);
            fill(f, rect, bg);
            let name = p.clip(&a.clip_id).map(|c| c.name.as_str()).unwrap_or("?");
            put(f, x, y, &clip_label(name, w as usize), fg, bg);
            self.push_hit(rect, Hit::ArrClip(i));
        }
    }

    fn render_piano(&mut self, f: &mut Frame, area: Rect) {
        let Some(id) = self.selected_clip_id() else {
            put(f, area.x + 2, area.y + 2, "select a clip in Session first", DIM, BG);
            return;
        };
        let p = self.project().clone();
        let Some(clip) = p.clip(&id) else { return };
        match &clip.kind {
            ClipKind::Drum { steps } => {
                put(
                    f,
                    area.x + 1,
                    area.y,
                    &format!(" {}  ·  drum steps  ·  click toggles ", clip.name),
                    ACCENT,
                    BG,
                );
                let step_w = ((area.width.saturating_sub(8)) / STEPS as u16).max(2);
                let play_step = self.playing_step(&id, STEPS as u32);
                for v in 0..DRUM_VOICES {
                    let y = area.y + 2 + v as u16;
                    if y >= area.y + area.height {
                        break;
                    }
                    put(f, area.x + 1, y, DRUM_NAMES[v], DIM, BG);
                    for s in 0..STEPS {
                        let x = area.x + 8 + s as u16 * step_w;
                        let on = steps[v][s];
                        let sel = self.piano_row as usize == v && self.piano_step as usize == s;
                        let bg = if on {
                            rgb(TrackKind::Drums.color())
                        } else if sel {
                            Color::Rgb(70, 56, 36)
                        } else if play_step == Some(s as u32) {
                            Color::Rgb(40, 48, 36)
                        } else if s % 4 == 0 {
                            Color::Rgb(24, 26, 36)
                        } else {
                            PANEL
                        };
                        let rect = Rect::new(x, y, step_w.saturating_sub(1), 1);
                        fill(f, rect, bg);
                        if sel && on {
                            put(f, x, y, "▪", BG, bg);
                        }
                        self.push_hit(
                            rect,
                            Hit::PianoCell {
                                row: v as u16,
                                step: s as u16,
                            },
                        );
                    }
                }
            }
            ClipKind::Midi { notes } => {
                let total = self.piano_view_steps(clip.length_steps());
                let key_w = 5u16;
                let step_w = (area.width.saturating_sub(key_w + 1) / total).clamp(2, 6);
                let visible = (area.width.saturating_sub(key_w + 1) / step_w).min(total).max(1);
                // keep the cursor on screen
                if self.piano_step < self.piano_off {
                    self.piano_off = self.piano_step;
                } else if self.piano_step >= self.piano_off + visible {
                    self.piano_off = self.piano_step + 1 - visible;
                }
                self.piano_off = self.piano_off.min(total.saturating_sub(visible));
                put(
                    f,
                    area.x + 1,
                    area.y,
                    &format!(
                        " {}  ·  {} bar{}  ·  steps {}–{} ",
                        clip.name,
                        clip.bars(),
                        if clip.bars() == 1 { "" } else { "s" },
                        self.piano_off + 1,
                        self.piano_off + visible
                    ),
                    ACCENT,
                    BG,
                );
                let color = p
                    .tracks
                    .get(self.track_for_clip(&id).unwrap_or(self.sel_track))
                    .map(|t| t.kind.color())
                    .unwrap_or(TrackKind::Lead.color());
                let play_step = self.playing_step(&id, clip.length_steps() as u32);
                let h = area.height.saturating_sub(2).min(PIANO_ROWS);
                for row in 0..h {
                    let y = area.y + 2 + row;
                    let pitch = self.row_pitch(row);
                    let black = matches!(pitch % 12, 1 | 3 | 6 | 8 | 10);
                    put(
                        f,
                        area.x + 1,
                        y,
                        &note_name(pitch),
                        if black { DIM } else { FG },
                        if black { BG } else { PANEL },
                    );
                    for vs in 0..visible {
                        let s = self.piano_off + vs;
                        let x = area.x + key_w + vs * step_w;
                        let note = notes
                            .iter()
                            .find(|n| n.pitch == pitch && n.start <= s && s < n.start + n.length);
                        let head = note.is_some_and(|n| n.start == s);
                        let sel = self.piano_row == row && self.piano_step == s;
                        let outside = s >= clip.length_steps();
                        let bg = if head {
                            rgb(color)
                        } else if note.is_some() {
                            dim_rgb(color, 0.6)
                        } else if sel {
                            Color::Rgb(60, 70, 90)
                        } else if play_step == Some(s as u32) {
                            Color::Rgb(40, 48, 36)
                        } else if outside {
                            Color::Rgb(14, 15, 20)
                        } else if s % 16 == 0 {
                            Color::Rgb(30, 32, 46)
                        } else if s % 4 == 0 {
                            Color::Rgb(22, 24, 34)
                        } else if black {
                            Color::Rgb(16, 16, 22)
                        } else {
                            PANEL
                        };
                        // Held notes read as one bar: only the last cell of
                        // a note keeps the gap.
                        let joined = note.is_some_and(|n| s + 1 < n.start + n.length);
                        let w = if joined { step_w } else { step_w.saturating_sub(1) };
                        let rect = Rect::new(x, y, w.max(1), 1);
                        fill(f, rect, bg);
                        if sel {
                            put(f, x, y, "▸", if note.is_some() { BG } else { SEL }, bg);
                        }
                        self.push_hit(Rect::new(x, y, step_w, 1), Hit::PianoCell { row, step: s });
                    }
                }
            }
            ClipKind::Audio { path, looped } => {
                put(f, area.x + 2, area.y + 1, &clip.name, ACCENT, BG);
                put(f, area.x + 2, area.y + 3, &format!("sample  {path}"), FG, BG);
                let have = self
                    .engine
                    .shared
                    .samples
                    .lock()
                    .ok()
                    .and_then(|m| m.get(path).map(|d| d.len() / 2));
                let info = match have {
                    Some(frames) => format!(
                        "length  {:.2}s",
                        frames as f32 / engine::SR as f32
                    ),
                    None => "length  — sample not loaded (missing file?)".into(),
                };
                put(f, area.x + 2, area.y + 4, &info, if have.is_some() { FG } else { REC }, BG);
                put(
                    f,
                    area.x + 2,
                    area.y + 5,
                    &format!("mode    {}  (L toggles)", if *looped { "loop" } else { "one-shot per bar" }),
                    FG,
                    BG,
                );
            }
        }
    }

    /// Current step within clip `id` if it is audibly playing right now.
    fn playing_step(&self, id: &str, len: u32) -> Option<u32> {
        let sh = &self.engine.shared;
        if !sh.playing.load(Ordering::Relaxed) || len == 0 {
            return None;
        }
        let step = sh.step.load(Ordering::Relaxed);
        if sh.arrange_active.load(Ordering::Relaxed) {
            let bar = step / STEPS as u32;
            let p = self.project();
            let a = p
                .arrangement
                .iter()
                .find(|a| a.clip_id == id && bar >= a.start_bar && bar < a.start_bar + a.length_bars)?;
            Some((step - a.start_bar * STEPS as u32) % len)
        } else {
            let active = sh.active.lock().ok()?;
            active.iter().any(|a| a.as_deref() == Some(id)).then_some(step % len)
        }
    }

    fn render_transport(&mut self, f: &mut Frame, r: Rect) {
        fill(f, r, TRANS_BG);
        let playing = self.engine.shared.playing.load(Ordering::Relaxed);
        let rec = self.engine.shared.recording.load(Ordering::Relaxed);
        let bar = self.engine.shared.bar.load(Ordering::Relaxed);
        let step = self.engine.shared.step.load(Ordering::Relaxed) % STEPS as u32;
        let mut x = r.x + 1;
        let btn = |this: &mut Studio, f: &mut Frame, x: u16, label: &str, fg: Color, hit: Hit| {
            put(f, x, r.y, label, fg, TRANS_BG);
            this.push_hit(Rect::new(x, r.y, label.width() as u16, 1), hit);
            x + label.width() as u16 + 1
        };
        x = btn(self, f, x, if playing { " ⏸ " } else { " ▶ " }, PLAY, Hit::Play);
        x = btn(self, f, x, " ⏹ ", DIM, Hit::Stop);
        x = btn(self, f, x, " ● ", if rec { REC } else { DIM }, Hit::Record);
        x = btn(self, f, x, " − ", DIM, Hit::BpmDown);
        x = btn(self, f, x, " + ", DIM, Hit::BpmUp);
        let mode = if self.engine.shared.arrange_active.load(Ordering::Relaxed) {
            "ARR"
        } else {
            "SES"
        };
        let pos = format!(" {:03}.{:02} {mode} ", bar + 1, step + 1);
        put(f, x, r.y, &pos, ACCENT, TRANS_BG);
        x += pos.width() as u16 + 1;
        x = btn(self, f, x, " Save ", SEL, Hit::Save);
        let exporting = self.export_rx.is_some();
        let _ = btn(
            self,
            f,
            x,
            if exporting { " Exporting… " } else { " Export " },
            ACCENT,
            Hit::Export,
        );
    }

    fn render_mixer(&mut self, f: &mut Frame, r: Rect) {
        fill(f, r, MIX_BG);
        let p = self.project().clone();
        let n = p.tracks.len().max(1);
        let col = (r.width / n as u16).max(14);
        for (i, tr) in p.tracks.iter().enumerate() {
            let x = r.x + i as u16 * col;
            let peak = self
                .engine
                .shared
                .peaks
                .get(i)
                .map(|a| a.load(Ordering::Relaxed) as f32 / 1000.0)
                .unwrap_or(0.0);
            let meters = ((peak * 6.0).ceil() as usize).min(6);
            let meter = format!("{}{}", "▮".repeat(meters), "▯".repeat(6 - meters));
            let name_w = col.saturating_sub(8) as usize;
            let label = clip_label(&tr.name, name_w);
            let fg = if i == self.sel_track { SEL } else { rgb(tr.kind.color()) };
            put(f, x, r.y, &label, fg, MIX_BG);
            let mcol = if peak > 0.95 { REC } else { PLAY };
            put(f, x + name_w as u16, r.y, &meter, mcol, MIX_BG);
            self.push_hit(Rect::new(x, r.y, name_w as u16, 1), Hit::Pan(i));

            let mfg = if tr.mute { REC } else { DIM };
            let sfg = if tr.solo { SEL } else { DIM };
            put(f, x, r.y + 1, "M", mfg, MIX_BG);
            put(f, x + 2, r.y + 1, "S", sfg, MIX_BG);
            self.push_hit(Rect::new(x, r.y + 1, 2, 1), Hit::Mute(i));
            self.push_hit(Rect::new(x + 2, r.y + 1, 2, 1), Hit::Solo(i));
            let filled = ((tr.vol / 1.2) * 5.0).round().clamp(0.0, 5.0) as usize;
            let vol = format!("{}{}", "█".repeat(filled), "░".repeat(5 - filled));
            put(f, x + 4, r.y + 1, &vol, ACCENT, MIX_BG);
            self.push_hit(Rect::new(x + 4, r.y + 1, 5, 1), Hit::Vol(i));
            put(f, x + 10, r.y + 1, &pan_label(tr.pan), DIM, MIX_BG);
            self.push_hit(Rect::new(x + 10, r.y + 1, 3, 1), Hit::Pan(i));

            let chips = [
                (0u8, "Flt", tr.fx.filter),
                (1, "Dly", tr.fx.delay),
                (2, "Rvb", tr.fx.reverb),
                (3, "Pmp", tr.fx.pump),
            ];
            if r.height > 2 {
                let mut cx = x;
                for (w, lab, on) in chips {
                    if cx + 3 > x + col {
                        break;
                    }
                    let fg = if on { PLAY } else { DIM };
                    put(f, cx, r.y + 2, lab, fg, MIX_BG);
                    self.push_hit(Rect::new(cx, r.y + 2, 3, 1), Hit::Fx { track: i, which: w });
                    cx += 3;
                }
            }
        }
    }

    fn render_palette(&mut self, f: &mut Frame, area: Rect) {
        let Some(pal) = self.palette else { return };
        let items = self.palette_items(pal);
        let title = match pal {
            Palette::Clip { scene, track } => {
                let name = self
                    .project()
                    .tracks
                    .get(track)
                    .map(|t| t.name.clone())
                    .unwrap_or_default();
                format!(" add clip · S{} · {name} ", scene + 1)
            }
            Palette::NewTrack => " add track ".to_string(),
        };
        let w = 30u16.min(area.width);
        let h = (items.len() as u16 + 4).min(area.height);
        let x = area.x + area.width.saturating_sub(w) / 2;
        let y = area.y + area.height.saturating_sub(h) / 2;
        let r = Rect::new(x, y, w, h);
        let clear = r.intersection(f.buffer_mut().area);
        f.render_widget(Clear, clear);
        fill(f, r, POP_BG);
        put(f, x + 1, y, &title, SEL, POP_BG);
        for (i, name) in items.iter().enumerate() {
            let yy = y + 2 + i as u16;
            let sel = i == self.pal_sel;
            let (fg, bg) = if sel { (BG, ACCENT) } else { (FG, POP_BG) };
            let line = clip_label(&format!(" {}. {name}", i + 1), w.saturating_sub(2) as usize);
            put(f, x + 1, yy, &line, fg, bg);
            self.push_hit(Rect::new(x, yy, w, 1), Hit::PaletteItem(i));
        }
        put(f, x + 1, y + h.saturating_sub(1), " click to add · outside cancels ", DIM, POP_BG);
    }
}

fn contains(r: Rect, col: u16, row: u16) -> bool {
    col >= r.x && col < r.x + r.width && row >= r.y && row < r.y + r.height
}

fn note_name(p: u8) -> String {
    const N: [&str; 12] = ["C", "C#", "D", "D#", "E", "F", "F#", "G", "G#", "A", "A#", "B"];
    format!("{:<4}", format!("{}{}", N[(p % 12) as usize], (p as i16 / 12) - 1))
}

fn pan_label(pan: f32) -> String {
    let v = (pan * 9.0).round() as i32;
    match v {
        0 => " C ".into(),
        v if v < 0 => format!("L{}", -v),
        v => format!("R{v}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn digit_keys_never_underflow() {
        assert_eq!(digit_index('0'), None);
        assert_eq!(digit_index('1'), Some(0));
        assert_eq!(digit_index('9'), Some(8));
        assert_eq!(digit_index('x'), None);
    }

    #[test]
    fn labels_fit_their_width() {
        assert_eq!(clip_label("Kick", 6).width(), 6);
        assert_eq!(clip_label("Very long clip name", 6).width(), 6);
        assert_eq!(pan_label(0.0), " C ");
        assert_eq!(pan_label(-1.0), "L9");
        assert_eq!(pan_label(0.5), "R5");
    }
}
