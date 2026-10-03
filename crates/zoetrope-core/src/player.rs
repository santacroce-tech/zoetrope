//! The runtime: a stateful tree of clocks, used for preview playback and the
//! exported player, and driven by scripts (see `script`).
//!
//! Timing rules (see docs/FORMAT.md, "Symbol timing" and "Scripting"):
//! - The root timeline advances one frame per `tick` while playing, looping.
//! - Every movie-clip instance has its own clock: it shows frame 0 and is
//!   playing on the tick it appears, then advances one frame per tick while
//!   playing, looping. An instance is identified by its `InstancePath`
//!   (layer + track at each level), so it keeps its clock (and any scripted
//!   property overrides) across keyframes that continue its track, and
//!   across the root looping, if it is still there.
//! - Graphic instances follow their parent (stateless rule).
//! - Buttons show Up, Over (pointer over the hit area) or Down (pressed on
//!   it); the Hit frame defines the hit area and is never drawn.
//! - A timeline *enters* a frame when it advances to it, when a script
//!   jumps to a different frame, or when the instance appears. Entering
//!   queues that frame's scripts and fires its event sounds.
//!
//! Without scripts or interaction, a `Player` started at frame 0 and ticked n
//! times renders exactly like the stateless evaluator at frame n, until the
//! root first loops (asserted in tests).

use crate::geom::Rect;
use crate::math::{Matrix, Point};
use crate::model::*;
use crate::query::{content_bounds, hits};
use crate::render::{render_with_clock, Clock, InstancePath, RenderOptions, Renderer, Stateless};
use crate::timeline::{evaluate_layer_shown, instance_frame, Shown};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};

/// Button frames.
pub const BUTTON_UP: u32 = 0;
pub const BUTTON_OVER: u32 = 1;
pub const BUTTON_DOWN: u32 = 2;
pub const BUTTON_HIT: u32 = 3;

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum PlayerEvent {
    /// Pointer pressed on a button.
    Press { path: InstancePath, name: String },
    /// Pointer released (anywhere) after pressing a button.
    Release { path: InstancePath, name: String },
    /// Pressed and released on the same button.
    Click { path: InstancePath, name: String },
}

/// Properties a script has taken over from the timeline. Each set field
/// replaces the animated value from then on (Flash semantics).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Override {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub x: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub y: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rotation: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub scale_x: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub scale_y: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub alpha: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub visible: Option<bool>,
    /// Text elements only.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
}

impl Override {
    pub fn transform(&self, t: &Transform) -> Transform {
        Transform {
            x: self.x.unwrap_or(t.x),
            y: self.y.unwrap_or(t.y),
            rotation: self.rotation.unwrap_or(t.rotation),
            scale_x: self.scale_x.unwrap_or(t.scale_x),
            scale_y: self.scale_y.unwrap_or(t.scale_y),
            ..*t
        }
    }

    /// `e` with the overridden properties applied.
    pub fn apply(&self, e: &Element) -> Element {
        let mut e = e.clone();
        e.transform = self.transform(&e.transform);
        if let Some(a) = self.alpha {
            e.opacity = a;
        }
        if let (Some(text), ElementKind::Text(t)) = (&self.text, &mut e.kind) {
            t.text = text.clone();
        }
        e
    }

    fn merge(&mut self, o: Override) {
        macro_rules! take {
            ($($f:ident),*) => { $( if o.$f.is_some() { self.$f = o.$f; } )* };
        }
        take!(x, y, rotation, scale_x, scale_y, alpha, visible, text);
    }

    fn validate(&self) -> crate::Result<()> {
        let nums = [self.x, self.y, self.rotation, self.scale_x, self.scale_y, self.alpha];
        if nums.iter().flatten().any(|v| !v.is_finite()) {
            return Err(crate::Error::Invalid("properties must be finite numbers".into()));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
struct Clip {
    frame: u32,
    playing: bool,
}

/// A script attached to a frame that has just been entered.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FrameScript {
    /// The timeline (root = empty path).
    pub path: InstancePath,
    pub symbol: SymbolId,
    pub layer: LayerId,
    pub frame: u32,
    pub script: String,
}

/// A symbol script to run for an instance that has just appeared.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct InstanceScript {
    pub path: InstancePath,
    pub symbol: SymbolId,
    pub script: String,
}

/// A timeline's playhead, as scripts see it.
#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TimelineState {
    pub frame: u32,
    pub length: u32,
    pub playing: bool,
}

/// Where a script jumps to.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(untagged)]
pub enum FrameTarget {
    /// 0-based frame index.
    Index(u32),
    Label(String),
}

/// An element as currently displayed, found by path.
#[derive(Debug, Clone)]
pub struct Resolved {
    /// Evaluated (tweened) element with overrides applied.
    pub element: Element,
    /// Parent space → stage.
    pub parent: Matrix,
    /// For instances: the frame their symbol shows.
    pub child_frame: u32,
    /// Effective visibility (an invisible ancestor hides it too).
    pub visible: bool,
}

impl Resolved {
    pub fn matrix(&self) -> Matrix {
        self.parent * self.element.transform.matrix()
    }
}

#[derive(Debug, Clone)]
pub struct Player {
    /// Root timeline frame.
    pub frame: u32,
    root_playing: bool,
    clips: HashMap<InstancePath, Clip>,
    overrides: HashMap<InstancePath, Override>,
    /// Every instance on stage (any symbol kind), from the last walk.
    on_stage: HashSet<InstancePath>,
    pointer: Option<Point>,
    down: bool,
    /// Topmost visible button under the pointer.
    over: Option<(InstancePath, String)>,
    /// Button the current press started on.
    pressed: Option<(InstancePath, String)>,
    /// Stream sounds active at the current frame.
    streams: Vec<SoundCue>,
    /// Event sounds triggered since the last `take_sound_events`.
    events: Vec<SoundCue>,
    /// Timelines that entered a frame since the last `sync`.
    entered: Vec<InstancePath>,
    /// Timelines whose frame scripts are due.
    due: Vec<InstancePath>,
    appeared: Vec<InstancePath>,
    removed: Vec<InstancePath>,
}

impl Clock for Player {
    fn frame(&self, path: &[(u32, u32)], kind: SymbolKind, stateless: u32) -> u32 {
        match kind {
            SymbolKind::Graphic => stateless,
            SymbolKind::MovieClip => self.clips.get(path).map_or(stateless, |c| c.frame),
            SymbolKind::Button => match &self.over {
                Some((p, _)) if p.as_slice() == path => {
                    if self.down && self.pressed.as_ref().is_some_and(|(q, _)| q.as_slice() == path) {
                        BUTTON_DOWN
                    } else {
                        BUTTON_OVER
                    }
                }
                _ => BUTTON_UP,
            },
        }
    }

    fn element_override(&self, path: &[(u32, u32)]) -> Option<&Override> {
        self.overrides.get(path)
    }

    fn has_overrides(&self) -> bool {
        !self.overrides.is_empty()
    }
}

/// One instance visited while walking the display tree.
struct Visit<'a> {
    path: &'a [(u32, u32)],
    symbol: &'a Symbol,
    name: &'a str,
    matrix: Matrix,
    visible: bool,
}

/// What a walk of the display tree reports.
enum Seen<'a> {
    /// An instance, with the frame it shows.
    Instance(Visit<'a>, u32),
    /// A keyframe sound on a timeline at `path` (layer `layer`), `offset`
    /// frames into its keyframe (which starts at `start`).
    Sound { path: &'a [(u32, u32)], layer: LayerId, start: u32, offset: u32, sound: &'a SoundRef },
}

/// A sound the platform should be playing (or start now).
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SoundCue {
    /// Identifies this sound placement (timeline path + layer + keyframe).
    pub key: String,
    pub asset: AssetId,
    pub sync: SoundSync,
    /// Seconds into the clip it should be at now.
    pub position: f64,
    pub volume: f64,
    pub loops: u32,
}

fn cue(path: &[(u32, u32)], layer: LayerId, start: u32, offset: u32, sound: &SoundRef, fps: f64) -> SoundCue {
    let key = path.iter().map(|(l, t)| format!("{l}.{t}")).chain([format!("{}@{start}", layer.0)]).collect::<Vec<_>>().join("/");
    SoundCue { key, asset: sound.asset, sync: sound.sync, position: offset as f64 / fps, volume: sound.volume, loops: sound.loops }
}

impl Player {
    /// A player showing root frame `frame`, with movie clips where the
    /// stateless rule puts them. Everything on stage counts as entered.
    pub fn new(p: &Project, frame: u32) -> Player {
        let mut player = Player {
            frame,
            root_playing: true,
            clips: HashMap::new(),
            overrides: HashMap::new(),
            on_stage: HashSet::new(),
            pointer: None,
            down: false,
            over: None,
            pressed: None,
            streams: Vec::new(),
            events: Vec::new(),
            entered: vec![Vec::new()],
            due: Vec::new(),
            appeared: Vec::new(),
            removed: Vec::new(),
        };
        // A fresh player starts each movie clip where the stateless rule shows it.
        player.sync_with(p, Advance::Stateless);
        player
    }

    /// Stream sounds that should be playing now, with their positions.
    pub fn sound_streams(&self) -> &[SoundCue] {
        &self.streams
    }

    /// Event sounds triggered since the last call.
    pub fn take_sound_events(&mut self) -> Vec<SoundCue> {
        std::mem::take(&mut self.events)
    }

    /// Advances every playing timeline by one frame.
    pub fn tick(&mut self, p: &Project) {
        let len = root_length(p);
        if self.root_playing && len > 1 {
            self.frame = (self.frame + 1) % len;
            self.entered.push(Vec::new());
        } else {
            self.frame = self.frame.min(len - 1);
        }
        self.sync_with(p, Advance::Tick);
        self.refresh_hover(p);
    }

    /// The local frame of every movie clip instance currently on stage.
    pub fn clip_frames(&self) -> HashMap<InstancePath, u32> {
        self.clips.iter().map(|(k, c)| (k.clone(), c.frame)).collect()
    }

    /// Re-walks the display tree without advancing: picks up instances
    /// that appeared or vanished after a jump, and fires entered frames'
    /// event sounds.
    pub fn settle(&mut self, p: &Project) {
        self.sync_with(p, Advance::None);
        self.refresh_hover(p);
    }

    fn sync_with(&mut self, p: &Project, advance: Advance) {
        let fps = p.stage.fps;
        let old = std::mem::take(&mut self.clips);
        let mut entered: Vec<InstancePath> = std::mem::take(&mut self.entered);
        let mut entered_set: HashSet<InstancePath> = entered.iter().cloned().collect();
        let mut clips = HashMap::new();
        let mut on_stage = HashSet::new();
        let mut appeared = Vec::new();
        let (mut streams, mut events) = (Vec::new(), Vec::new());
        let me: &Player = self;
        let frame_of = |path: &[(u32, u32)], sym: &Symbol, stateless: u32| match sym.kind {
            SymbolKind::MovieClip => match (old.get(path), advance) {
                (None, Advance::Stateless) => stateless,
                (None, _) => 0,
                (Some(c), Advance::Tick) if c.playing && sym.length() > 1 => (c.frame + 1) % sym.length(),
                (Some(c), _) => c.frame,
            },
            kind => Clock::frame(me, path, kind, stateless),
        };
        walk(p, self.frame, &frame_of, &self.overrides, &mut |seen| match seen {
            Seen::Instance(v, f) => {
                let path = v.path.to_vec();
                if !me.on_stage.contains(&path) {
                    appeared.push(path.clone());
                }
                if v.symbol.kind == SymbolKind::MovieClip {
                    let prev = old.get(&path);
                    if prev.is_none_or(|c| c.frame != f) && entered_set.insert(path.clone()) {
                        entered.push(path.clone());
                    }
                    clips.insert(path.clone(), Clip { frame: f, playing: prev.is_none_or(|c| c.playing) });
                }
                on_stage.insert(path);
            }
            Seen::Sound { path, layer, start, offset, sound } => match sound.sync {
                SoundSync::Stream => streams.push(cue(path, layer, start, offset, sound, fps)),
                // Events fire when their keyframe is entered.
                SoundSync::Event if offset == 0 && entered_set.contains(path) => events.push(cue(path, layer, start, offset, sound, fps)),
                SoundSync::Event => {}
            },
        });
        let removed: Vec<InstancePath> = self.on_stage.iter().filter(|q| !on_stage.contains(*q)).cloned().collect();
        if !removed.is_empty() {
            // A removed instance takes its scripted state (and its children's) with it.
            self.overrides.retain(|k, _| !removed.iter().any(|r| k.starts_with(r)));
        }
        self.clips = clips;
        self.on_stage = on_stage;
        self.streams = streams;
        self.events.extend(events);
        self.appeared.extend(appeared);
        self.removed.extend(removed);
        // Only timelines still on stage run their scripts.
        let on_stage = &self.on_stage;
        self.due.extend(entered.into_iter().filter(|q| q.is_empty() || on_stage.contains(q)));
    }

    // ------------------------------------------------------------ scripting

    /// Frame scripts of the frames entered since the last call, in order:
    /// timelines in the order they were entered (parents before children),
    /// layers bottom first. Guide layers' scripts don't run.
    pub fn take_frame_scripts(&mut self, p: &Project) -> Vec<FrameScript> {
        let mut seen = HashSet::new();
        let due: Vec<InstancePath> = std::mem::take(&mut self.due).into_iter().filter(|q| seen.insert(q.clone())).collect();
        let mut out = Vec::new();
        for path in due {
            let Some((symbol, frame)) = self.timeline_at(p, &path) else { continue };
            let Some(sym) = p.symbol(symbol) else { continue };
            for (layer, _, _) in sym.content_layers() {
                if layer.kind == LayerKind::Guide {
                    continue;
                }
                if let Some((i, start)) = layer.keyframe_at(frame) {
                    if let (true, Some(script)) = (start == frame, &layer.keyframes[i].script) {
                        out.push(FrameScript { path: path.clone(), symbol, layer: layer.id, frame, script: script.clone() });
                    }
                }
            }
        }
        out
    }

    /// Symbol scripts for instances that appeared since the last call.
    pub fn take_instance_scripts(&mut self, p: &Project) -> Vec<InstanceScript> {
        std::mem::take(&mut self.appeared)
            .into_iter()
            .filter_map(|path| {
                let r = self.resolve(p, &path)?;
                let ElementKind::Instance { symbol, .. } = r.element.kind else { return None };
                let script = p.symbol(symbol)?.script.clone()?;
                Some(InstanceScript { path, symbol, script })
            })
            .collect()
    }

    /// Instances that left the stage since the last call.
    pub fn take_removed(&mut self) -> Vec<InstancePath> {
        std::mem::take(&mut self.removed)
    }

    /// The symbol and frame of the timeline at `path` (root = `[]`; else a
    /// movie clip instance on stage).
    fn timeline_at(&self, p: &Project, path: &[(u32, u32)]) -> Option<(SymbolId, u32)> {
        if path.is_empty() {
            return Some((p.root, self.frame));
        }
        let clip = self.clips.get(path)?;
        match self.resolve(p, path)?.element.kind {
            ElementKind::Instance { symbol, .. } => Some((symbol, clip.frame)),
            _ => None,
        }
    }

    /// Playhead of the root (`[]`) or of a movie clip; `None` otherwise
    /// (graphics follow their parent and buttons the pointer).
    pub fn timeline(&self, p: &Project, path: &[(u32, u32)]) -> Option<TimelineState> {
        let (symbol, frame) = self.timeline_at(p, path)?;
        let length = p.symbol(symbol)?.length();
        let playing = if path.is_empty() { self.root_playing } else { self.clips[path].playing };
        Some(TimelineState { frame, length, playing })
    }

    fn require_timeline(&self, p: &Project, path: &[(u32, u32)]) -> crate::Result<(SymbolId, u32)> {
        self.timeline_at(p, path).ok_or_else(|| crate::Error::Invalid("not a movie clip (only the main timeline and movie clips have playheads)".into()))
    }

    pub fn set_playing(&mut self, p: &Project, path: &[(u32, u32)], playing: bool) -> crate::Result<()> {
        self.require_timeline(p, path)?;
        if path.is_empty() {
            self.root_playing = playing;
        } else if let Some(c) = self.clips.get_mut(path) {
            c.playing = playing;
        }
        Ok(())
    }

    /// Jumps a timeline to `target` and sets whether it plays on. Entering a
    /// different frame queues its scripts (run after the current script).
    pub fn goto(&mut self, p: &Project, path: &[(u32, u32)], target: &FrameTarget, play: bool) -> crate::Result<()> {
        let (symbol, current) = self.require_timeline(p, path)?;
        let sym = p.require_symbol(symbol)?;
        let frame = match target {
            FrameTarget::Index(i) => (*i).min(sym.length() - 1),
            FrameTarget::Label(name) => frame_of_label(sym, name).ok_or_else(|| crate::Error::NotFound(format!("frame label {name:?}")))?,
        };
        if path.is_empty() {
            self.frame = frame;
            self.root_playing = play;
        } else if let Some(c) = self.clips.get_mut(path) {
            *c = Clip { frame, playing: play };
        }
        if frame != current {
            self.entered.push(path.to_vec());
        }
        self.settle(p);
        Ok(())
    }

    /// Finds the element at `path` as currently displayed.
    pub fn resolve(&self, p: &Project, path: &[(u32, u32)]) -> Option<Resolved> {
        let mut symbol = p.root;
        let mut frame = self.frame;
        let mut parent = Matrix::IDENTITY;
        let mut visible = true;
        for depth in 0..path.len() {
            let (layer_id, track) = path[depth];
            let sym = p.symbol(symbol)?;
            let (layer, _, _) = sym.content_layers().into_iter().find(|(l, vis, _)| l.id.0 == layer_id && *vis && l.kind != LayerKind::Guide)?;
            let shown: Shown = evaluate_layer_shown(layer, frame).into_iter().find(|s| s.element.track() == track)?;
            let prefix = &path[..=depth];
            let ov = self.overrides.get(prefix);
            let element = ov.map_or_else(|| shown.element.clone().into_owned(), |o| o.apply(&shown.element));
            visible &= ov.and_then(|o| o.visible) != Some(false);
            let child_frame = match element.kind {
                ElementKind::Instance { symbol: child, .. } => {
                    let child = p.symbol(child)?;
                    Clock::frame(self, prefix, child.kind, instance_frame(child, &element.kind, &shown, frame)).min(child.length() - 1)
                }
                _ => 0,
            };
            if depth + 1 == path.len() {
                return Some(Resolved { element, parent, child_frame, visible });
            }
            let ElementKind::Instance { symbol: child, .. } = element.kind else { return None };
            parent = parent * element.transform.matrix();
            symbol = child;
            frame = child_frame;
        }
        None
    }

    /// Changes scripted properties of the element at `path`.
    pub fn set_properties(&mut self, p: &Project, path: &[(u32, u32)], props: Override) -> crate::Result<()> {
        props.validate()?;
        let r = self.resolve(p, path).ok_or_else(|| crate::Error::NotFound("that object is no longer on stage".into()))?;
        if props.text.is_some() && !matches!(r.element.kind, ElementKind::Text(_)) {
            return Err(crate::Error::Invalid("only text has a text property".into()));
        }
        self.overrides.entry(path.to_vec()).or_default().merge(props);
        self.refresh_hover(p);
        Ok(())
    }

    /// The named children shown on the timeline at `path` (root = `[]`), in
    /// render order: `(name, path)`.
    pub fn named_children(&self, p: &Project, path: &[(u32, u32)]) -> Vec<(String, InstancePath)> {
        let (symbol, frame) = if path.is_empty() {
            (p.root, self.frame)
        } else {
            match self.resolve(p, path) {
                Some(Resolved { element: Element { kind: ElementKind::Instance { symbol, .. }, .. }, child_frame, .. }) => (symbol, child_frame),
                _ => return Vec::new(),
            }
        };
        let Some(sym) = p.symbol(symbol) else { return Vec::new() };
        let mut out = Vec::new();
        for (layer, vis, _) in sym.content_layers() {
            if !vis || layer.kind == LayerKind::Guide {
                continue;
            }
            for shown in evaluate_layer_shown(layer, frame) {
                if !shown.element.name.is_empty() {
                    let mut child = path.to_vec();
                    child.push((layer.id.0, shown.element.track()));
                    out.push((shown.element.name.clone(), child));
                }
            }
        }
        out
    }

    /// The topmost child named `name` of the timeline at `path`.
    pub fn child_named(&self, p: &Project, path: &[(u32, u32)], name: &str) -> Option<InstancePath> {
        self.named_children(p, path).into_iter().rev().find(|(n, _)| n == name).map(|(_, q)| q)
    }

    /// Stage-space bounds of the element at `path`.
    pub fn bounds(&self, p: &Project, path: &[(u32, u32)]) -> Option<Rect> {
        let r = self.resolve(p, path)?;
        content_bounds(p, &r.element.kind, &r.matrix(), r.child_frame, 0)
    }

    /// Whether stage point `pt` is over the element at `path`: on its drawn
    /// shapes (`shape`), or else anywhere in its bounds. Invisible → false.
    pub fn hit_test_point(&self, p: &Project, path: &[(u32, u32)], pt: Point, shape: bool) -> bool {
        let Some(r) = self.resolve(p, path).filter(|r| r.visible) else { return false };
        if shape {
            hits(p, &r.element.kind, &r.matrix(), pt, 0.0, r.child_frame, 0)
        } else {
            content_bounds(p, &r.element.kind, &r.matrix(), r.child_frame, 0)
                .is_some_and(|b| pt.x >= b.min.x && pt.x <= b.max.x && pt.y >= b.min.y && pt.y <= b.max.y)
        }
    }

    /// Whether two elements' stage bounds overlap (both visible).
    pub fn hit_test_object(&self, p: &Project, a: &[(u32, u32)], b: &[(u32, u32)]) -> bool {
        let vis = |q: &[(u32, u32)]| self.resolve(p, q).is_some_and(|r| r.visible);
        match (self.bounds(p, a), self.bounds(p, b)) {
            (Some(x), Some(y)) => vis(a) && vis(b) && x.intersects(&y),
            _ => false,
        }
    }

    // ------------------------------------------------------------ pointer

    /// Feeds pointer state (stage coordinates). Returns button events.
    pub fn pointer(&mut self, p: &Project, pt: Option<Point>, down: bool) -> Vec<PlayerEvent> {
        self.pointer = pt;
        self.refresh_hover(p);
        let mut events = Vec::new();
        if down && !self.down {
            self.pressed = self.over.clone();
            if let Some((path, name)) = &self.over {
                events.push(PlayerEvent::Press { path: path.clone(), name: name.clone() });
            }
        }
        if !down && self.down {
            if let Some((path, name)) = self.pressed.take() {
                let click = self.over.as_ref().is_some_and(|(q, _)| *q == path);
                events.push(PlayerEvent::Release { path: path.clone(), name: name.clone() });
                if click {
                    events.push(PlayerEvent::Click { path, name });
                }
            }
        }
        self.down = down;
        events
    }

    /// Whether the pointer is over a button (for a hand cursor).
    pub fn over_button(&self) -> bool {
        self.over.is_some()
    }

    fn refresh_hover(&mut self, p: &Project) {
        let Some(pt) = self.pointer else {
            self.over = None;
            return;
        };
        let mut hit: Option<(InstancePath, String)> = None;
        let me: &Player = self;
        walk(p, self.frame, &|path, sym, stateless| Clock::frame(me, path, sym.kind, stateless), &self.overrides, &mut |seen| {
            let Seen::Instance(v, _) = seen else { return };
            if v.symbol.kind != SymbolKind::Button || !v.visible {
                return;
            }
            let hit_frame = if v.symbol.length() > BUTTON_HIT { BUTTON_HIT } else { BUTTON_UP };
            let kind = ElementKind::instance(v.symbol.id);
            // Later visits draw on top, so the last hit wins.
            if hits(p, &kind, &v.matrix, pt, 0.0, hit_frame, 0) {
                hit = Some((v.path.to_vec(), v.name.to_string()));
            }
        });
        self.over = hit;
    }

    pub fn render(&self, p: &Project, opts: RenderOptions, r: &mut dyn Renderer) {
        render_with_clock(p, self.frame, opts, self, r);
    }

    /// Fingerprint of what a viewer gets right now: the picture (as the
    /// exported player draws it, at stage scale) and the stream sounds.
    /// Two engines playing the same project the same way agree on it at
    /// every tick (editor ↔ export parity checks).
    pub fn digest(&self, p: &Project) -> u64 {
        let mut r = crate::render::RecordingRenderer::default();
        self.render(p, RenderOptions::player(Matrix::IDENTITY), &mut r);
        self.streams.iter().fold(crate::render::digest(&r.ops), |h, c| h.rotate_left(7) ^ c.position.to_bits() ^ c.asset.0 as u64)
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum Advance {
    /// Starting up: clips take their stateless frames.
    Stateless,
    /// One tick: playing clips advance.
    Tick,
    /// Settling after a jump: nothing advances; new clips start at 0.
    None,
}

fn root_length(p: &Project) -> u32 {
    p.symbol(p.root).map_or(1, |s| s.length())
}

/// The first frame labelled `name` (layers bottom first).
pub fn frame_of_label(sym: &Symbol, name: &str) -> Option<u32> {
    sym.content_layers()
        .into_iter()
        .filter_map(|(l, _, _)| l.keyframes.iter().position(|k| k.label.as_deref() == Some(name)).map(|i| l.keyframe_start(i)))
        .min()
}

/// Chooses an instance's frame during a walk: `(path, symbol, stateless frame) → frame`.
type FrameOf<'a> = dyn Fn(&[(u32, u32)], &Symbol, u32) -> u32 + 'a;

/// Walks the display tree of the root at `frame` (hidden and guide layers
/// excluded, as in the player). `frame_of(path, symbol, stateless)` picks
/// each instance's frame; `visit` sees every instance (in render order, a
/// parent before its children) with that frame, and every keyframe sound.
fn walk(p: &Project, frame: u32, frame_of: &FrameOf, overrides: &HashMap<InstancePath, Override>, visit: &mut dyn FnMut(Seen)) {
    #[allow(clippy::too_many_arguments)]
    fn go(
        p: &Project,
        symbol: SymbolId,
        frame: u32,
        m: Matrix,
        visible: bool,
        path: &mut InstancePath,
        frame_of: &FrameOf,
        overrides: &HashMap<InstancePath, Override>,
        visit: &mut dyn FnMut(Seen),
    ) {
        if path.len() > MAX_NESTING_DEPTH {
            return;
        }
        let Some(sym) = p.symbol(symbol) else { return };
        for (layer, vis, _) in sym.content_layers() {
            if !vis || layer.kind == LayerKind::Guide {
                continue;
            }
            if let Some((i, start)) = layer.keyframe_at(frame) {
                if let Some(sound) = &layer.keyframes[i].sound {
                    visit(Seen::Sound { path, layer: layer.id, start, offset: frame - start, sound });
                }
            }
            for shown in evaluate_layer_shown(layer, frame) {
                let el = &shown.element;
                let ElementKind::Instance { symbol: child_id, .. } = el.kind else { continue };
                let Some(child) = p.symbol(child_id) else { continue };
                path.push((layer.id.0, el.track()));
                let ov = overrides.get(path.as_slice());
                let transform = ov.map_or(el.transform, |o| o.transform(&el.transform));
                let vis = visible && ov.and_then(|o| o.visible) != Some(false);
                let stateless = instance_frame(child, &el.kind, &shown, frame);
                let f = frame_of(path, child, stateless).min(child.length() - 1);
                let em = m * transform.matrix();
                visit(Seen::Instance(Visit { path, symbol: child, name: &el.name, matrix: em, visible: vis }, f));
                go(p, child_id, f, em, vis, path, frame_of, overrides, visit);
                path.pop();
            }
        }
    }
    go(p, p.root, frame, Matrix::IDENTITY, true, &mut Vec::new(), frame_of, overrides, visit);
}

/// Renders a stateless frame (convenience for comparisons).
pub fn render_stateless(p: &Project, frame: u32, opts: RenderOptions, r: &mut dyn Renderer) {
    render_with_clock(p, frame, opts, &Stateless, r);
}
