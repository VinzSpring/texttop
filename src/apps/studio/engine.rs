//! Live mixer (rodio Source), synths, FX, sample decode, WAV bounce.

use std::collections::HashMap;
use std::fs::File;
use std::io::{BufReader, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use rodio::{Decoder, OutputStream, OutputStreamHandle, Sink, Source};

use super::project::{
    resolve_sample_path, ArrClip, Clip, ClipKind, FxChips, Project, Track, TrackKind,
    DRUM_VOICES, MAX_BARS, MAX_TRACKS, STEPS,
};

pub const SR: u32 = 44100;
const SR_F: f32 = SR as f32;
const TAU: f32 = std::f32::consts::PI * 2.0;
const MAX_SAMPLE_BYTES: u64 = 24 * 1024 * 1024;
const MAX_SAMPLE_FRAMES: usize = SR as usize * 180;

pub fn samples_per_step(bpm: f32) -> f64 {
    (SR as f64) * 60.0 / (bpm.max(40.0) as f64) / 4.0
}

pub fn midi_hz(n: u8) -> f32 {
    440.0 * 2f32.powf((n as f32 - 69.0) / 12.0)
}

#[derive(Clone)]
pub enum Cmd {
    Play,
    Stop,
    Pause,
    Launch { track: usize, clip: Option<String> },
    LaunchScene(usize),
    /// Preview a note or drum hit on a track (piano-roll feedback).
    Audition { track: usize, note: Audition },
}

#[derive(Clone, Copy)]
pub enum Audition {
    Drum(usize),
    Pitch(u8),
}

/// How long an auditioned pitch is held before release.
const AUDITION_SAMPLES: u32 = SR / 4;

pub struct Shared {
    pub playing: AtomicBool,
    pub recording: AtomicBool,
    /// Requested by the UI (Arrange view).
    pub arrange_mode: AtomicBool,
    /// What the mixer is actually playing from.
    pub arrange_active: AtomicBool,
    pub sample_pos: AtomicU64,
    pub bar: AtomicU32,
    pub step: AtomicU32,
    pub bpm_x100: AtomicU32,
    pub master: AtomicU32,
    pub gen: AtomicU64,
    pub cmds: Mutex<Vec<Cmd>>,
    pub project: Mutex<Project>,
    pub peaks: [AtomicU32; MAX_TRACKS],
    pub active: Mutex<Vec<Option<String>>>,
    pub recorded: Mutex<Vec<ArrClip>>,
    pub samples: Mutex<HashMap<String, Arc<Vec<f32>>>>,
}

impl Shared {
    fn new(project: Project) -> Self {
        let n = project.tracks.len();
        let bpm = project.bpm;
        Shared {
            playing: AtomicBool::new(false),
            recording: AtomicBool::new(false),
            arrange_mode: AtomicBool::new(false),
            arrange_active: AtomicBool::new(false),
            sample_pos: AtomicU64::new(0),
            bar: AtomicU32::new(0),
            step: AtomicU32::new(0),
            bpm_x100: AtomicU32::new((bpm * 100.0) as u32),
            master: AtomicU32::new(850),
            gen: AtomicU64::new(1),
            cmds: Mutex::new(Vec::new()),
            project: Mutex::new(project),
            peaks: std::array::from_fn(|_| AtomicU32::new(0)),
            active: Mutex::new(vec![None; n]),
            recorded: Mutex::new(Vec::new()),
            samples: Mutex::new(HashMap::new()),
        }
    }

    pub fn push(&self, c: Cmd) {
        if let Ok(mut q) = self.cmds.lock() {
            q.push(c);
        }
    }

    pub fn bump(&self) {
        self.gen.fetch_add(1, Ordering::Relaxed);
    }
}

pub struct Engine {
    _stream: Option<OutputStream>,
    _handle: Option<OutputStreamHandle>,
    _sink: Option<Sink>,
    pub shared: Arc<Shared>,
    pub error: Option<String>,
}

impl Engine {
    pub fn start(project: Project) -> Self {
        let shared = Arc::new(Shared::new(project));
        match OutputStream::try_default() {
            Ok((stream, handle)) => match Sink::try_new(&handle) {
                Ok(sink) => {
                    sink.append(MixSource::new(Arc::clone(&shared)));
                    sink.play();
                    Engine {
                        _stream: Some(stream),
                        _handle: Some(handle),
                        _sink: Some(sink),
                        shared,
                        error: None,
                    }
                }
                Err(e) => Engine {
                    _stream: Some(stream),
                    _handle: Some(handle),
                    _sink: None,
                    shared,
                    error: Some(format!("audio error: {e}")),
                },
            },
            Err(e) => Engine {
                _stream: None,
                _handle: None,
                _sink: None,
                shared,
                error: Some(format!("no audio device: {e}")),
            },
        }
    }
}

struct DrumVoice {
    env: f32,
    phase: f32,
    freq: f32,
    noise: u32,
    kind: usize,
}

impl DrumVoice {
    fn new(kind: usize) -> Self {
        DrumVoice {
            env: 0.0,
            phase: 0.0,
            freq: 80.0,
            noise: 0xA341316C ^ (kind as u32).wrapping_mul(0x9E3779B9),
            kind,
        }
    }

    fn trigger(&mut self) {
        self.env = 1.0;
        self.phase = 0.0;
        self.freq = match self.kind {
            0 => 170.0,
            1 => 190.0,
            6 => 140.0,
            7 => 800.0,
            _ => 200.0,
        };
    }

    fn noise(&mut self) -> f32 {
        self.noise ^= self.noise << 13;
        self.noise ^= self.noise >> 17;
        self.noise ^= self.noise << 5;
        (self.noise as i32 as f32) / (i32::MAX as f32)
    }

    fn tick(&mut self) -> f32 {
        if self.env < 0.0008 {
            return 0.0;
        }
        let n = self.noise();
        let s = match self.kind {
            0 => {
                // kick: sine + pitch drop
                let v = (self.phase * TAU).sin() * self.env;
                self.phase += self.freq / SR_F;
                self.freq *= 0.99955;
                self.env *= 0.99915;
                v
            }
            1 => {
                let tone = (self.phase * TAU).sin() * 0.35;
                self.phase += self.freq / SR_F;
                self.env *= 0.9986;
                (tone + n * 0.55) * self.env
            }
            2 => {
                self.env *= 0.993;
                n * self.env * 0.45
            }
            3 => {
                self.env *= 0.9978;
                n * self.env * 0.4
            }
            4 => {
                self.env *= 0.9965;
                n * self.env * 0.55
            }
            5 => {
                let t = (self.phase * TAU).sin();
                self.phase += 720.0 / SR_F;
                self.env *= 0.997;
                (t * 0.4 + n * 0.3) * self.env
            }
            6 => {
                let v = (self.phase * TAU).sin() * self.env;
                self.phase += self.freq / SR_F;
                self.freq *= 0.9997;
                self.env *= 0.9989;
                v * 0.7
            }
            _ => {
                self.env *= 0.988;
                n * self.env * 0.5
            }
        };
        s.clamp(-1.0, 1.0)
    }
}

#[derive(Clone, Copy)]
enum OscKind {
    Saw,
    Square,
    DualSaw,
}

struct OscVoice {
    phase: f32,
    phase2: f32,
    freq: f32,
    pitch: u8,
    env: f32,
    gate: bool,
    attack: f32,
    release: f32,
    detune: f32,
    kind: OscKind,
    vel: f32,
}

impl OscVoice {
    fn new(kind: OscKind, attack: f32, release: f32, detune: f32) -> Self {
        OscVoice {
            phase: 0.0,
            phase2: 0.0,
            freq: 110.0,
            pitch: 0,
            env: 0.0,
            gate: false,
            attack,
            release,
            detune,
            kind,
            vel: 1.0,
        }
    }

    fn trigger(&mut self, pitch: u8, vel: f32) {
        self.freq = midi_hz(pitch);
        self.pitch = pitch;
        self.vel = vel;
        self.gate = true;
        if self.env < 0.05 {
            self.phase = 0.0;
        }
    }

    fn release_gate(&mut self) {
        self.gate = false;
    }

    fn tick(&mut self) -> f32 {
        if self.gate {
            self.env = (self.env + self.attack).min(1.0);
        } else {
            self.env *= self.release;
        }
        if self.env < 1e-4 {
            return 0.0;
        }
        let s = match self.kind {
            OscKind::Saw => self.phase * 2.0 - 1.0,
            OscKind::Square => {
                if self.phase < 0.5 {
                    1.0
                } else {
                    -1.0
                }
            }
            OscKind::DualSaw => (self.phase * 2.0 - 1.0) * 0.7 + (self.phase2 * 2.0 - 1.0) * 0.55,
        };
        self.phase += self.freq / SR_F;
        self.phase -= self.phase.floor();
        self.phase2 += self.freq * self.detune / SR_F;
        self.phase2 -= self.phase2.floor();
        s * self.env * self.vel
    }
}

struct Svf {
    lp: f32,
    bp: f32,
}

impl Svf {
    fn tick(&mut self, x: f32, cutoff: f32, res: f32) -> f32 {
        let f = cutoff.clamp(0.01, 0.95);
        let q = res.clamp(0.05, 0.95);
        self.lp += f * self.bp;
        let hp = x - self.lp - q * self.bp;
        self.bp += f * hp;
        self.lp
    }
}

struct Delay {
    buf: Vec<f32>,
    pos: usize,
}

impl Delay {
    fn new() -> Self {
        Delay {
            buf: vec![0.0; SR as usize],
            pos: 0,
        }
    }

    fn tick(&mut self, x: f32, taps: usize, mix: f32, fb: f32) -> f32 {
        let n = self.buf.len();
        let tap = taps.clamp(64, n - 1);
        let ri = (self.pos + n - tap) % n;
        let y = self.buf[ri];
        self.buf[self.pos] = x + y * fb;
        self.pos = (self.pos + 1) % n;
        x * (1.0 - mix) + y * mix
    }
}

struct Reverb {
    comb: [DelayLine; 4],
    ap: [DelayLine; 2],
}

struct DelayLine {
    buf: Vec<f32>,
    pos: usize,
}

impl DelayLine {
    fn new(n: usize) -> Self {
        DelayLine {
            buf: vec![0.0; n.max(2)],
            pos: 0,
        }
    }

    fn tick(&mut self, x: f32, fb: f32) -> f32 {
        let y = self.buf[self.pos];
        self.buf[self.pos] = x + y * fb;
        self.pos = (self.pos + 1) % self.buf.len();
        y
    }
}

impl Reverb {
    fn new() -> Self {
        Reverb {
            comb: [
                DelayLine::new(1116),
                DelayLine::new(1188),
                DelayLine::new(1277),
                DelayLine::new(1356),
            ],
            ap: [DelayLine::new(225), DelayLine::new(556)],
        }
    }

    fn tick(&mut self, x: f32, mix: f32) -> f32 {
        let mut s = 0.0;
        for c in &mut self.comb {
            s += c.tick(x, 0.72);
        }
        s *= 0.25;
        s = self.ap[0].tick(s, 0.5);
        s = self.ap[1].tick(s, 0.5);
        x * (1.0 - mix) + s * mix
    }
}

struct SamplerVoice {
    data: Arc<Vec<f32>>,
    pos: f64,
    playing: bool,
    looped: bool,
}

impl SamplerVoice {
    /// Start `data` at the next clip boundary. A looped sample that is
    /// already running keeps going so loops longer than the clip aren't
    /// chopped; a finished or different sample restarts.
    fn launch(&mut self, data: Arc<Vec<f32>>, looped: bool) {
        if self.playing && self.looped && looped && Arc::ptr_eq(&self.data, &data) {
            return;
        }
        self.trigger(data, looped);
    }

    fn trigger(&mut self, data: Arc<Vec<f32>>, looped: bool) {
        self.data = data;
        self.pos = 0.0;
        self.playing = true;
        self.looped = looped;
    }

    fn tick(&mut self) -> (f32, f32) {
        if !self.playing || self.data.len() < 2 {
            return (0.0, 0.0);
        }
        let frames = self.data.len() / 2;
        let i = self.pos as usize;
        if i >= frames {
            if self.looped {
                self.pos = 0.0;
            } else {
                self.playing = false;
                return (0.0, 0.0);
            }
        }
        let i = self.pos as usize;
        let f = (self.pos.fract()) as f32;
        let i2 = (i + 1).min(frames.saturating_sub(1));
        let l = self.data[i * 2] * (1.0 - f) + self.data[i2 * 2] * f;
        let r = self.data[i * 2 + 1] * (1.0 - f) + self.data[i2 * 2 + 1] * f;
        self.pos += 1.0;
        (l, r)
    }
}

struct TrackState {
    drums: [DrumVoice; DRUM_VOICES],
    osc: [OscVoice; 4],
    sampler: SamplerVoice,
    filter: Svf,
    delay: Delay,
    reverb: Reverb,
    peak: f32,
}

impl TrackState {
    fn new(kind: TrackKind) -> Self {
        let (osc_kind, atk, rel, det) = match kind {
            TrackKind::Bass => (OscKind::Square, 0.004, 0.9994, 1.0),
            TrackKind::Lead => (OscKind::Saw, 0.002, 0.9992, 1.0),
            TrackKind::Pad => (OscKind::DualSaw, 0.00015, 0.99985, 1.007),
            _ => (OscKind::Saw, 0.003, 0.9993, 1.0),
        };
        TrackState {
            drums: std::array::from_fn(DrumVoice::new),
            osc: std::array::from_fn(|_| OscVoice::new(osc_kind, atk, rel, det)),
            sampler: SamplerVoice {
                data: Arc::new(Vec::new()),
                pos: 0.0,
                playing: false,
                looped: false,
            },
            filter: Svf { lp: 0.0, bp: 0.0 },
            delay: Delay::new(),
            reverb: Reverb::new(),
            peak: 0.0,
        }
    }

    /// Release held notes and stop the sampler (clip stopped / transport stop).
    /// Drum and envelope tails ring out naturally.
    fn silence(&mut self) {
        for o in &mut self.osc {
            o.release_gate();
        }
        self.sampler.playing = false;
    }
}

/// The per-sample view of a track the mixer needs (all `Copy`).
struct TrackMix {
    kind: TrackKind,
    vol: f32,
    pan: f32,
    mute: bool,
    solo: bool,
    fx: FxChips,
}

impl TrackMix {
    fn of(t: &Track) -> Self {
        TrackMix {
            kind: t.kind,
            vol: t.vol,
            pan: t.pan,
            mute: t.mute,
            solo: t.solo,
            fx: t.fx,
        }
    }
}

struct Mixer {
    project: Project,
    gen: u64,
    /// Transport position; frozen while paused.
    sample: u64,
    /// Free-running frame counter for housekeeping (command polling, meters),
    /// which must keep going while the transport is paused.
    frames: u64,
    auditions: Vec<(usize, u8, u32)>,
    last_step: i64,
    last_bar: i64,
    playing: bool,
    arrange: bool,
    active: Vec<Option<String>>,
    pending: Vec<Option<Option<String>>>,
    tracks: Vec<TrackState>,
    track_kinds: Vec<TrackKind>,
    /// Cached `project.arrangement_bars()` (the mix loop runs per sample).
    arr_end: u32,
    pump: f32,
    pending_r: Option<f32>,
}

impl Mixer {
    fn new(project: Project) -> Self {
        let tracks = project
            .tracks
            .iter()
            .map(|t| TrackState::new(t.kind))
            .collect();
        let track_kinds = project.tracks.iter().map(|t| t.kind).collect();
        let arr_end = project.arrangement_bars();
        let n = project.tracks.len();
        Mixer {
            project,
            gen: 0,
            sample: 0,
            frames: 0,
            auditions: Vec::new(),
            last_step: -1,
            last_bar: -1,
            playing: false,
            arrange: false,
            active: vec![None; n],
            pending: vec![None; n],
            tracks,
            track_kinds,
            arr_end,
            pump: 0.0,
            pending_r: None,
        }
    }

    fn resize_tracks(&mut self) {
        let n = self.project.tracks.len().min(MAX_TRACKS);
        // A track removed or replaced mid-list shifts the rest: rebuild any
        // voice state whose kind no longer matches its slot.
        let kinds: Vec<TrackKind> = self.project.tracks.iter().take(n).map(|t| t.kind).collect();
        let stale = self
            .tracks
            .iter()
            .zip(&self.track_kinds)
            .zip(&kinds)
            .any(|((_, a), b)| a != b);
        if stale {
            self.tracks.clear();
            self.active = vec![None; n];
            self.pending = vec![None; n];
        }
        while self.tracks.len() < n {
            let kind = kinds[self.tracks.len()];
            self.tracks.push(TrackState::new(kind));
        }
        self.tracks.truncate(n);
        self.track_kinds = kinds;
        self.active.resize(n, None);
        self.pending.resize(n, None);
    }

    fn silence_all(&mut self) {
        for t in &mut self.tracks {
            t.silence();
        }
        self.auditions.clear();
    }

    fn apply_cmds(&mut self, cmds: Vec<Cmd>) {
        for c in cmds {
            match c {
                Cmd::Play => self.playing = true,
                Cmd::Stop => {
                    self.playing = false;
                    self.sample = 0;
                    self.last_step = -1;
                    self.last_bar = -1;
                    self.active.fill(None);
                    self.pending.fill(None);
                    self.pump = 0.0;
                    self.silence_all();
                }
                Cmd::Pause => {
                    self.playing = false;
                    self.silence_all();
                }
                Cmd::Audition { track, note } => {
                    if track >= self.tracks.len() {
                        continue;
                    }
                    match note {
                        Audition::Drum(v) if v < DRUM_VOICES => self.tracks[track].drums[v].trigger(),
                        Audition::Drum(_) => {}
                        Audition::Pitch(p) => {
                            self.alloc_osc(track, p, 100);
                            self.auditions.push((track, p, AUDITION_SAMPLES));
                        }
                    }
                }
                Cmd::Launch { track, clip } => {
                    if track < self.pending.len() {
                        self.pending[track] = Some(clip);
                    }
                }
                Cmd::LaunchScene(scene) => {
                    if let Some(row) = self.project.session.get(scene) {
                        for (t, cell) in row.iter().enumerate() {
                            if t < self.pending.len() {
                                self.pending[t] = Some(cell.clone());
                            }
                        }
                    }
                }
            }
        }
    }

    fn apply_pending_at_bar(&mut self, bar: u32, shared: Option<&Shared>) {
        for t in 0..self.pending.len() {
            if let Some(next) = self.pending[t].take() {
                if next != self.active[t] {
                    if let Some(ts) = self.tracks.get_mut(t) {
                        ts.silence();
                    }
                }
                self.active[t] = next;
            }
        }
        if let Some(sh) = shared {
            // Recording stops at the arrangement cap so the project stays loadable.
            if bar >= MAX_BARS {
                sh.recording.store(false, Ordering::Relaxed);
            }
            if sh.recording.load(Ordering::Relaxed) && !self.arrange {
                if let Ok(mut rec) = sh.recorded.lock() {
                    for (t, id) in self.active.iter().enumerate() {
                        if let Some(id) = id {
                            rec.push(ArrClip {
                                clip_id: id.clone(),
                                track: t,
                                start_bar: bar,
                                length_bars: 1,
                            });
                        }
                    }
                }
            }
            if let Ok(mut a) = sh.active.lock() {
                *a = self.active.clone();
            }
        }
    }

    fn session_clip(&self, track: usize) -> Option<&Clip> {
        let id = self.active.get(track)?.as_ref()?;
        self.project.clip(id)
    }

    fn arrange_clip(&self, track: usize, bar: u32) -> Option<(&Clip, u32)> {
        let a = self.project.arrangement.iter().find(|a| {
            a.track == track && bar >= a.start_bar && bar < a.start_bar + a.length_bars
        })?;
        let clip = self.project.clip(&a.clip_id)?;
        Some((clip, bar - a.start_bar))
    }

    fn trigger_clip_step(
        &mut self,
        track: usize,
        clip: &Clip,
        local_step: u32,
        samples: &HashMap<String, Arc<Vec<f32>>>,
    ) {
        match &clip.kind {
            ClipKind::Drum { steps } => {
                for v in 0..DRUM_VOICES {
                    if steps[v][local_step as usize % STEPS] {
                        self.tracks[track].drums[v].trigger();
                        if v == 0 {
                            self.pump = 1.0;
                        }
                    }
                }
            }
            ClipKind::Midi { notes } => {
                let len = clip.length_steps().max(1) as u32;
                let local = local_step % len;
                // Note-offs first so a note ending where the same pitch starts
                // again (including across the loop point) retriggers cleanly.
                for n in notes {
                    let end = (n.start as u32 + n.length.max(1) as u32) % len;
                    if end == local {
                        self.release_pitch(track, n.pitch);
                    }
                }
                for n in notes {
                    if n.start as u32 == local {
                        self.alloc_osc(track, n.pitch, n.vel);
                    }
                }
            }
            ClipKind::Audio { path, looped } => {
                if local_step.is_multiple_of(STEPS as u32) {
                    if let Some(data) = samples.get(path).cloned() {
                        self.tracks[track].sampler.launch(data, *looped);
                    }
                }
            }
        }
    }

    fn alloc_osc(&mut self, track: usize, pitch: u8, vel: u8) {
        let v = vel as f32 / 127.0;
        let osc = &mut self.tracks[track].osc;
        let mut best = 0;
        let mut best_env = f32::MAX;
        for (i, o) in osc.iter().enumerate() {
            if !o.gate && o.env < 0.02 {
                best = i;
                break;
            }
            if o.env < best_env {
                best_env = o.env;
                best = i;
            }
        }
        osc[best].trigger(pitch, v);
    }

    fn release_pitch(&mut self, track: usize, pitch: u8) {
        for o in &mut self.tracks[track].osc {
            if o.gate && o.pitch == pitch {
                o.release_gate();
            }
        }
    }

    fn on_step(
        &mut self,
        step: u64,
        samples: &HashMap<String, Arc<Vec<f32>>>,
        shared: Option<&Shared>,
    ) {
        let bar = (step / STEPS as u64) as u32;
        let step_in_bar = (step % STEPS as u64) as u32;
        if bar as i64 != self.last_bar {
            self.last_bar = bar as i64;
            self.apply_pending_at_bar(bar, shared);
        }
        let n = self.tracks.len();
        // clone clip data we need so we can mutate voices without borrow issues
        let jobs: Vec<(usize, Option<(Clip, u32)>)> = (0..n)
            .map(|t| {
                let job = if self.arrange {
                    self.arrange_clip(t, bar).map(|(clip, bar_in)| {
                        (clip.clone(), bar_in * STEPS as u32 + step_in_bar)
                    })
                } else {
                    self.session_clip(t).map(|clip| {
                        let len = clip.length_steps().max(1) as u64;
                        let local = ((bar as u64 * STEPS as u64 + step_in_bar as u64) % len) as u32;
                        (clip.clone(), local)
                    })
                };
                (t, job)
            })
            .collect();
        for (t, job) in jobs {
            match job {
                Some((clip, local)) => self.trigger_clip_step(t, &clip, local, samples),
                // Nothing scheduled here (clip stopped, gap in the
                // arrangement): don't let notes or loops hang on.
                None => self.tracks[t].silence(),
            }
        }
    }

    fn render_stereo(&mut self, samples: &HashMap<String, Arc<Vec<f32>>>, shared: Option<&Shared>) -> (f32, f32) {
        if self.playing {
            let bpm = shared
                .map(|s| s.bpm_x100.load(Ordering::Relaxed) as f32 / 100.0)
                .unwrap_or(self.project.bpm)
                .clamp(40.0, 240.0);
            let sps = samples_per_step(bpm);
            let mut step = (self.sample as f64 / sps).floor() as i64;
            // Live arrangement playback loops back to the top at the end.
            let end_bar = self.arr_end as i64;
            if shared.is_some() && self.arrange && end_bar > 0 && step >= end_bar * STEPS as i64 {
                self.sample = 0;
                self.last_bar = -1;
                step = 0;
                self.last_step = -1;
            }
            if step != self.last_step && step >= 0 {
                self.last_step = step;
                self.on_step(step as u64, samples, shared);
            }
            self.sample += 1;
        }
        self.pump *= 0.99915;
        if !self.auditions.is_empty() {
            let mut done = Vec::new();
            self.auditions.retain_mut(|(t, p, left)| {
                *left = left.saturating_sub(1);
                if *left == 0 {
                    done.push((*t, *p));
                }
                *left > 0
            });
            for (t, p) in done {
                if t < self.tracks.len() {
                    self.release_pitch(t, p);
                }
            }
        }

        let any_solo = self.project.tracks.iter().any(|t| t.solo);
        let mut ml = 0.0f32;
        let mut mr = 0.0f32;
        let n = self.tracks.len().min(self.project.tracks.len());
        for t in 0..n {
            // Copy out the few mixer fields: cloning the Track (with its name)
            // would allocate on the audio thread every sample.
            let tr = TrackMix::of(&self.project.tracks[t]);
            if tr.mute || (any_solo && !tr.solo) {
                let _ = self.tracks[t].drums.iter_mut().map(|d| d.tick()).sum::<f32>();
                for o in &mut self.tracks[t].osc {
                    let _ = o.tick();
                }
                let _ = self.tracks[t].sampler.tick();
                continue;
            }
            let mut sig = 0.0f32;
            match tr.kind {
                TrackKind::Drums => {
                    for d in &mut self.tracks[t].drums {
                        sig += d.tick();
                    }
                }
                TrackKind::Sampler => {
                    let (l, r) = self.tracks[t].sampler.tick();
                    let pan = tr.pan.clamp(-1.0, 1.0);
                    let g = tr.vol.clamp(0.0, 1.5);
                    let (mut tl, mut trr) = (
                        l * g * ((1.0 - pan) * 0.5 + 0.5),
                        r * g * ((1.0 + pan) * 0.5 + 0.5),
                    );
                    (tl, trr) = self.fx_stereo(t, &tr, tl, trr, shared);
                    self.tracks[t].peak = self.tracks[t].peak.max(tl.abs().max(trr.abs()));
                    ml += tl;
                    mr += trr;
                    continue;
                }
                _ => {
                    for o in &mut self.tracks[t].osc {
                        sig += o.tick() * 0.28;
                    }
                }
            }
            if tr.kind != TrackKind::Drums && tr.fx.pump {
                sig *= 1.0 - 0.72 * self.pump;
            }
            if tr.fx.filter {
                let cut = 0.04 + tr.fx.filter_amt.clamp(0.0, 1.0) * 0.45;
                sig = self.tracks[t].filter.tick(sig, cut, 0.25);
            }
            if tr.fx.delay {
                let tap = self.delay_taps(shared);
                sig = self.tracks[t].delay.tick(sig, tap, 0.28, 0.32);
            }
            if tr.fx.reverb {
                sig = self.tracks[t].reverb.tick(sig, 0.28);
            }
            let g = tr.vol.clamp(0.0, 1.5);
            sig *= g;
            let pan = tr.pan.clamp(-1.0, 1.0);
            let l = sig * ((1.0 - pan) * 0.5 + 0.5);
            let r = sig * ((1.0 + pan) * 0.5 + 0.5);
            self.tracks[t].peak = self.tracks[t].peak.max(sig.abs());
            ml += l;
            mr += r;
        }
        (ml.clamp(-1.2, 1.2), mr.clamp(-1.2, 1.2))
    }

    /// Dotted-eighth delay (3 sixteenths) at the current tempo.
    fn delay_taps(&self, shared: Option<&Shared>) -> usize {
        let bpm = shared
            .map(|s| s.bpm_x100.load(Ordering::Relaxed) as f32 / 100.0)
            .unwrap_or(self.project.bpm);
        (samples_per_step(bpm) as usize * 3).max(128)
    }

    fn fx_stereo(
        &mut self,
        t: usize,
        tr: &TrackMix,
        mut l: f32,
        mut r: f32,
        shared: Option<&Shared>,
    ) -> (f32, f32) {
        if !tr.fx.pump && !tr.fx.filter && !tr.fx.delay && !tr.fx.reverb {
            return (l, r);
        }
        if tr.fx.pump {
            let g = 1.0 - 0.72 * self.pump;
            l *= g;
            r *= g;
        }
        if !tr.fx.filter && !tr.fx.delay && !tr.fx.reverb {
            return (l, r);
        }
        // The FX run on the mid channel; the side (stereo width of the
        // sample) is carried through untouched.
        let dry = (l + r) * 0.5;
        let side = (l - r) * 0.5;
        let mut mid = dry;
        if tr.fx.filter {
            let cut = 0.04 + tr.fx.filter_amt.clamp(0.0, 1.0) * 0.45;
            mid = self.tracks[t].filter.tick(mid, cut, 0.2);
        }
        if tr.fx.delay {
            let tap = self.delay_taps(shared);
            mid = self.tracks[t].delay.tick(mid, tap, 0.25, 0.3);
        }
        if tr.fx.reverb {
            mid = self.tracks[t].reverb.tick(mid, 0.22);
        }
        let side = if tr.fx.filter { side * 0.5 } else { side };
        (mid + side, mid - side)
    }

    fn tick_live(&mut self, shared: &Shared) -> f32 {
        if let Some(r) = self.pending_r.take() {
            return r;
        }
        self.frames = self.frames.wrapping_add(1);
        // Housekeeping runs on the free-running counter: the transport
        // position stands still while paused, and polling on it would never
        // see the Play that resumes it.
        if self.frames % 64 == 1 {
            let cmds = shared
                .cmds
                .try_lock()
                .map(|mut q| q.drain(..).collect())
                .unwrap_or_default();
            self.apply_cmds(cmds);
            self.playing = shared.playing.load(Ordering::Relaxed);
            let gen = shared.gen.load(Ordering::Relaxed);
            if gen != self.gen {
                // try_lock: never stall the audio thread behind the UI; a
                // busy lock just means the update lands 64 frames later.
                if let Ok(p) = shared.project.try_lock() {
                    self.project = p.clone();
                    self.arr_end = self.project.arrangement_bars();
                    self.gen = gen;
                    self.resize_tracks();
                }
            }
            // An empty arrangement has nothing to play: keep the session going
            // while the first clips are laid out.
            let arrange = shared.arrange_mode.load(Ordering::Relaxed) && self.arr_end > 0;
            if arrange != self.arrange {
                self.arrange = arrange;
                self.silence_all();
                self.last_bar = -1;
            }
            shared.arrange_active.store(arrange, Ordering::Relaxed);
        }
        let samples = shared.samples.lock().ok();
        let empty = HashMap::new();
        let map = samples.as_deref().unwrap_or(&empty);
        let (l, r) = self.render_stereo(map, Some(shared));
        let master = shared.master.load(Ordering::Relaxed) as f32 / 1000.0;
        let l = (l * master).clamp(-1.0, 1.0);
        let r = (r * master).clamp(-1.0, 1.0);
        shared.sample_pos.store(self.sample, Ordering::Relaxed);
        let bpm = shared.bpm_x100.load(Ordering::Relaxed) as f32 / 100.0;
        let sps = samples_per_step(bpm);
        let step = if sps > 0.0 {
            (self.sample as f64 / sps).floor() as u32
        } else {
            0
        };
        shared.step.store(step, Ordering::Relaxed);
        shared.bar.store(step / STEPS as u32, Ordering::Relaxed);
        if self.frames.is_multiple_of(256) {
            for (i, ts) in self.tracks.iter_mut().enumerate() {
                if i < MAX_TRACKS {
                    shared.peaks[i].store((ts.peak * 1000.0) as u32, Ordering::Relaxed);
                }
                // ~20 dB per 300 ms: slow enough for a 40 ms UI frame to see.
                ts.peak *= 0.91;
            }
        }
        self.pending_r = Some(r);
        l
    }
}

struct MixSource {
    shared: Arc<Shared>,
    mix: Mixer,
}

impl MixSource {
    fn new(shared: Arc<Shared>) -> Self {
        let project = shared.project.lock().map(|p| p.clone()).unwrap_or_else(|_| Project::blank());
        MixSource {
            mix: Mixer::new(project),
            shared,
        }
    }
}

impl Iterator for MixSource {
    type Item = f32;

    fn next(&mut self) -> Option<f32> {
        Some(self.mix.tick_live(&self.shared))
    }
}

impl Source for MixSource {
    fn current_frame_len(&self) -> Option<usize> {
        Some(64)
    }
    fn channels(&self) -> u16 {
        2
    }
    fn sample_rate(&self) -> u32 {
        SR
    }
    fn total_duration(&self) -> Option<Duration> {
        None
    }
}

/// Decode any rodio-supported file to interleaved stereo @ 44.1 kHz.
pub fn load_sample(path: &Path) -> Result<Vec<f32>, String> {
    let meta = std::fs::metadata(path).map_err(|e| format!("cannot open: {e}"))?;
    if meta.len() > MAX_SAMPLE_BYTES {
        return Err(format!("sample too large ({} bytes)", meta.len()));
    }
    let file = File::open(path).map_err(|e| format!("cannot open: {e}"))?;
    let dec = Decoder::new(BufReader::new(file)).map_err(|e| format!("cannot decode: {e}"))?;
    let ch = dec.channels() as usize;
    let sr = dec.sample_rate();
    let limit = MAX_SAMPLE_FRAMES.saturating_mul(ch.max(1));
    let mut raw = Vec::new();
    for s in dec.convert_samples::<f32>() {
        raw.push(s);
        if raw.len() > limit {
            return Err("sample too long".into());
        }
    }
    if raw.is_empty() {
        return Err("empty sample".into());
    }
    let stereo = if ch == 1 {
        raw.iter().flat_map(|s| [*s, *s]).collect()
    } else if ch >= 2 {
        raw.chunks(ch)
            .flat_map(|f| [f[0], f[1]])
            .collect::<Vec<_>>()
    } else {
        raw
    };
    if sr == SR {
        return Ok(stereo);
    }
    let frames = stereo.len() / 2;
    let ratio = SR as f64 / sr as f64;
    let out_frames = ((frames as f64) * ratio).round() as usize;
    let mut out = Vec::with_capacity(out_frames * 2);
    for i in 0..out_frames {
        let src = i as f64 / ratio;
        let i0 = src.floor() as usize;
        let f = src.fract() as f32;
        let i1 = (i0 + 1).min(frames.saturating_sub(1));
        let i0 = i0.min(frames.saturating_sub(1));
        out.push(stereo[i0 * 2] * (1.0 - f) + stereo[i1 * 2] * f);
        out.push(stereo[i0 * 2 + 1] * (1.0 - f) + stereo[i1 * 2 + 1] * f);
    }
    Ok(out)
}

/// Load every audio clip into a map keyed by the path string stored on the clip.
pub fn load_project_samples(
    project: &Project,
    project_dir: Option<&Path>,
) -> HashMap<String, Arc<Vec<f32>>> {
    let mut map = HashMap::new();
    for clip in &project.clips {
        if let ClipKind::Audio { path, .. } = &clip.kind {
            if path.is_empty() || map.contains_key(path) {
                continue;
            }
            let fs_path = resolve_sample_path(path, project_dir);
            if let Ok(data) = load_sample(&fs_path) {
                map.insert(path.clone(), Arc::new(data));
            }
        }
    }
    map
}

pub fn write_wav(path: &Path, samples: &[f32], sample_rate: u32) -> Result<(), String> {
    let n = samples.len() as u32;
    let data_bytes = n * 2;
    let mut out = Vec::with_capacity(44 + data_bytes as usize);
    out.extend(b"RIFF");
    out.extend(&(36 + data_bytes).to_le_bytes());
    out.extend(b"WAVE");
    out.extend(b"fmt ");
    out.extend(&16u32.to_le_bytes());
    out.extend(&1u16.to_le_bytes());
    out.extend(&2u16.to_le_bytes());
    out.extend(&sample_rate.to_le_bytes());
    out.extend(&(sample_rate * 4).to_le_bytes());
    out.extend(&4u16.to_le_bytes());
    out.extend(&16u16.to_le_bytes());
    out.extend(b"data");
    out.extend(&data_bytes.to_le_bytes());
    for s in samples {
        let v = (s.clamp(-1.0, 1.0) * 32767.0) as i16;
        out.extend(&v.to_le_bytes());
    }
    let mut f = File::create(path).map_err(|e| format!("cannot write wav: {e}"))?;
    f.write_all(&out).map_err(|e| format!("cannot write wav: {e}"))
}

/// Offline mix: play scene 0 (or the arrangement) for `bars` bars.
pub fn render_offline(
    project: &Project,
    bars: u32,
    samples: &HashMap<String, Arc<Vec<f32>>>,
) -> Vec<f32> {
    let bars = bars.clamp(1, MAX_BARS);
    let mut mix = Mixer::new(project.clone());
    mix.playing = true;
    mix.arrange = !project.arrangement.is_empty();
    if !mix.arrange {
        if let Some(row) = project.session.first() {
            for (t, cell) in row.iter().enumerate() {
                if t < mix.pending.len() {
                    mix.pending[t] = Some(cell.clone());
                }
            }
        }
    }
    let n = (samples_per_step(project.bpm) * STEPS as f64 * bars as f64) as usize * 2;
    let mut out = Vec::with_capacity(n);
    let frames = n / 2;
    for _ in 0..frames {
        let (l, r) = mix.render_stereo(samples, None);
        out.push(l.clamp(-1.0, 1.0));
        out.push(r.clamp(-1.0, 1.0));
    }
    out
}

pub fn export_wav(
    project: &Project,
    dest: &Path,
    project_file: Option<&Path>,
    live: Option<&HashMap<String, Arc<Vec<f32>>>>,
) -> Result<PathBuf, String> {
    let arr = project.arrangement_bars();
    if arr > MAX_BARS {
        return Err(format!("arrangement too long ({arr} bars, max {MAX_BARS})"));
    }
    // The arrangement plus one bar of FX tail, or 8 bars of scene 1.
    let bars = if arr > 0 { arr + 1 } else { 8 };
    let dir = project_file.and_then(|p| p.parent()).or_else(|| dest.parent());
    let mut samples = load_project_samples(project, dir);
    if let Some(live) = live {
        for (k, v) in live {
            samples.entry(k.clone()).or_insert_with(|| Arc::clone(v));
        }
    }
    let pcm = render_offline(project, bars, &samples);
    if pcm.iter().all(|s| s.abs() < 1e-6) && project.clips.is_empty() {
        return Err("nothing to export — add clips first".into());
    }
    write_wav(dest, &pcm, SR)?;
    Ok(dest.to_path_buf())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::apps::studio::project::template_house;

    #[test]
    fn step_timing_120() {
        let sps = samples_per_step(120.0);
        assert!((sps - 5512.5).abs() < 0.01);
    }

    #[test]
    fn house_mix_has_energy() {
        let p = template_house();
        let samples = render_offline(&p, 1, &HashMap::new());
        assert!(samples.len() > 1000);
        let peak = samples.iter().map(|s| s.abs()).fold(0.0f32, f32::max);
        assert!(peak > 0.02, "peak was {peak}");
        assert!(samples.iter().all(|s| s.is_finite()));
    }

    fn run_frames(m: &mut Mixer, sh: &Shared, frames: usize) -> f32 {
        let mut peak = 0.0f32;
        for _ in 0..frames {
            peak = peak.max(m.tick_live(sh).abs());
            peak = peak.max(m.tick_live(sh).abs());
        }
        peak
    }

    #[test]
    fn resumes_after_pausing_mid_block() {
        let sh = Shared::new(template_house());
        let mut m = Mixer::new(template_house());
        sh.playing.store(true, Ordering::Relaxed);
        sh.push(Cmd::Play);
        run_frames(&mut m, &sh, 1001);
        sh.playing.store(false, Ordering::Relaxed);
        sh.push(Cmd::Pause);
        run_frames(&mut m, &sh, 200);
        // Park the transport off a 64-frame boundary — where polling on the
        // transport position used to stop seeing commands for good.
        m.sample += 7;
        let paused_at = m.sample;
        sh.playing.store(true, Ordering::Relaxed);
        sh.push(Cmd::Play);
        run_frames(&mut m, &sh, 200);
        assert!(m.sample > paused_at, "transport never resumed");
    }

    #[test]
    fn stop_releases_held_notes() {
        let mut p = Project::blank();
        let id = p.add_clip(
            "drone",
            ClipKind::Midi {
                notes: vec![super::super::project::MidiNote {
                    pitch: 48,
                    start: 0,
                    length: 16,
                    vel: 100,
                }],
            },
        );
        p.session[0][3] = Some(id);
        let sh = Shared::new(p.clone());
        let mut m = Mixer::new(p);
        sh.playing.store(true, Ordering::Relaxed);
        sh.push(Cmd::Play);
        sh.push(Cmd::LaunchScene(0));
        assert!(run_frames(&mut m, &sh, SR as usize) > 0.01, "pad never sounded");
        sh.playing.store(false, Ordering::Relaxed);
        sh.push(Cmd::Stop);
        run_frames(&mut m, &sh, SR as usize * 3);
        let tail = run_frames(&mut m, &sh, 4096);
        assert!(tail < 1e-3, "note still ringing after stop: {tail}");
    }

    #[test]
    fn wav_roundtrip_header() {
        let dir = std::env::temp_dir().join("texttop_studio_test.wav");
        let samples = vec![0.1f32, -0.1, 0.2, -0.2];
        write_wav(&dir, &samples, SR).unwrap();
        let bytes = std::fs::read(&dir).unwrap();
        assert_eq!(&bytes[0..4], b"RIFF");
        assert_eq!(&bytes[8..12], b"WAVE");
        let _ = std::fs::remove_file(&dir);
    }

    fn unique_dir(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!(
            "texttop_studio_{tag}_{}_{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|t| t.as_nanos())
                .unwrap_or(0)
        ));
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    fn loud_wav(path: &Path) {
        let n = (SR as usize / 5) * 2;
        let data: Vec<f32> = (0..n).map(|_| 0.8).collect();
        write_wav(path, &data, SR).unwrap();
    }

    fn sampler_project(wav: &Path) -> Project {
        let mut p = Project::blank();
        p.tracks.push(Track::new("Smp", TrackKind::Sampler));
        p.ensure_grid();
        let t = p.tracks.len() - 1;
        let id = p.add_clip(
            "hit",
            ClipKind::Audio {
                path: wav.to_string_lossy().into(),
                looped: true,
            },
        );
        p.session[0][t] = Some(id);
        p
    }

    #[test]
    fn export_includes_dropped_sample() {
        let dir = unique_dir("export");
        let wav = dir.join("hit.wav");
        loud_wav(&wav);
        let decoded = load_sample(&wav).expect("decode written wav");
        assert!(decoded.iter().any(|s| s.abs() > 0.1), "decoded wav silent");
        let p = sampler_project(&wav);
        let out = dir.join("bounce.wav");
        export_wav(&p, &out, None, None).unwrap();
        let pcm = load_sample(&out).unwrap();
        let peak = pcm.iter().map(|s| s.abs()).fold(0.0f32, f32::max);
        assert!(peak > 0.1, "exported peak was {peak}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn reload_resolves_relative_sample() {
        let dir = unique_dir("reload");
        let wav = dir.join("hit.wav");
        loud_wav(&wav);
        let p = sampler_project(&wav);
        let proj = dir.join("song.ttstudio");
        p.save(&proj).unwrap();
        let loaded = Project::load(&proj).unwrap();
        match &loaded.clips[0].kind {
            ClipKind::Audio { path, .. } => {
                assert!(!Path::new(path).is_absolute(), "{path}");
            }
            other => panic!("expected audio clip, got {other:?}"),
        }
        let map = load_project_samples(&loaded, proj.parent());
        assert!(!map.is_empty(), "sample map empty after reload");
        let pcm = render_offline(&loaded, 1, &map);
        let peak = pcm.iter().map(|s| s.abs()).fold(0.0f32, f32::max);
        assert!(peak > 0.1, "reload peak was {peak}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn export_rejects_huge_arrangement() {
        let mut p = Project::blank();
        p.arrangement.push(ArrClip {
            clip_id: "c1".into(),
            track: 0,
            start_bar: 0,
            length_bars: 10_000,
        });
        let dest = std::env::temp_dir().join("texttop_studio_huge.wav");
        let err = export_wav(&p, &dest, None, None).unwrap_err();
        assert!(err.contains("too long"), "{err}");
    }
}
