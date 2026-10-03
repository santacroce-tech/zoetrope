//! The runtime: a stateful tree of clocks, used for preview playback now and
//! by the exported player (Phase 8) and scripting (Phase 7) later.
//!
//! Timing rules (see docs/FORMAT.md, "Symbol timing"):
//! - The root timeline advances one frame per `tick` and loops.
//! - Every movie-clip instance has its own clock: it shows frame 0 on the
//!   tick it appears, then advances one frame per tick, looping. An instance
//!   is identified by its `InstancePath` (layer + track at each level), so it
//!   keeps its clock across keyframes that continue its track — and across
//!   the root looping, if it is still there.
//! - Graphic instances follow their parent (stateless rule).
//! - Buttons show Up, Over (pointer over the hit area) or Down (pressed on
//!   it); the Hit frame defines the hit area and is never drawn.
//!
//! Without interaction, a `Player` started at frame 0 and ticked n times
//! renders exactly like the stateless evaluator at frame n, until the root
//! first loops (asserted in tests).

use crate::math::{Matrix, Point};
use crate::model::*;
use crate::query::hits;
use crate::render::{render_with_clock, Clock, InstancePath, RenderOptions, Renderer, Stateless};
use crate::timeline::{evaluate_layer_shown, instance_frame};
use serde::Serialize;
use std::collections::HashMap;

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
    /// Pressed and released on the same button.
    Click { path: InstancePath, name: String },
}

#[derive(Debug, Clone)]
pub struct Player {
    /// Root timeline frame.
    pub frame: u32,
    clips: HashMap<InstancePath, u32>,
    pointer: Option<Point>,
    down: bool,
    /// Topmost button under the pointer.
    over: Option<(InstancePath, String)>,
    /// Button the current press started on.
    pressed: Option<InstancePath>,
    /// Stream sounds active at the current frame.
    streams: Vec<SoundCue>,
    /// Event sounds triggered since the last `take_sound_events`.
    events: Vec<SoundCue>,
}

impl Clock for Player {
    fn frame(&self, path: &[(u32, u32)], kind: SymbolKind, stateless: u32) -> u32 {
        match kind {
            SymbolKind::Graphic => stateless,
            SymbolKind::MovieClip => self.clips.get(path).copied().unwrap_or(stateless),
            SymbolKind::Button => match &self.over {
                Some((p, _)) if p.as_slice() == path => {
                    if self.down && self.pressed.as_deref() == Some(path) {
                        BUTTON_DOWN
                    } else {
                        BUTTON_OVER
                    }
                }
                _ => BUTTON_UP,
            },
        }
    }
}

/// One instance visited while walking the display tree.
struct Visit<'a> {
    path: &'a [(u32, u32)],
    symbol: &'a Symbol,
    name: &'a str,
    matrix: Matrix,
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
    /// stateless rule puts them.
    pub fn new(p: &Project, frame: u32) -> Player {
        let mut player = Player {
            frame,
            clips: HashMap::new(),
            pointer: None,
            down: false,
            over: None,
            pressed: None,
            streams: Vec::new(),
            events: Vec::new(),
        };
        let fps = p.stage.fps;
        let (mut clips, mut streams, mut events) = (HashMap::new(), Vec::new(), Vec::new());
        walk(p, frame, &|_, _, stateless| stateless, &mut |seen| collect(seen, fps, &mut clips, &mut streams, &mut events));
        (player.clips, player.streams, player.events) = (clips, streams, events);
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

    /// Advances the root and every movie clip by one frame.
    pub fn tick(&mut self, p: &Project) {
        let len = p.symbol(p.root).map_or(1, |s| s.length());
        self.frame = (self.frame + 1) % len;
        let old = std::mem::take(&mut self.clips);
        let mut clips = HashMap::new();
        let advance = |path: &[(u32, u32)], sym: &Symbol, stateless: u32| match sym.kind {
            SymbolKind::MovieClip => old.get(path).map_or(0, |f| (f + 1) % sym.length()),
            _ => stateless,
        };
        // Buttons keep their pointer state; graphics stay stateless.
        let me: &Player = self;
        let frame_of = |path: &[(u32, u32)], sym: &Symbol, stateless: u32| match sym.kind {
            SymbolKind::MovieClip => advance(path, sym, stateless),
            kind => Clock::frame(me, path, kind, stateless),
        };
        let fps = p.stage.fps;
        let (mut streams, mut events) = (Vec::new(), Vec::new());
        walk(p, self.frame, &frame_of, &mut |seen| collect(seen, fps, &mut clips, &mut streams, &mut events));
        self.clips = clips;
        self.streams = streams;
        self.events.extend(events);
        self.refresh_hover(p);
    }

    /// The local frame of every movie clip instance currently on stage.
    pub fn clip_frames(&self) -> &HashMap<InstancePath, u32> {
        &self.clips
    }

    /// Feeds pointer state (stage coordinates). Returns button events.
    pub fn pointer(&mut self, p: &Project, pt: Option<Point>, down: bool) -> Vec<PlayerEvent> {
        self.pointer = pt;
        self.refresh_hover(p);
        let mut events = Vec::new();
        if down && !self.down {
            self.pressed = self.over.as_ref().map(|(path, _)| path.clone());
            if let Some((path, name)) = &self.over {
                events.push(PlayerEvent::Press { path: path.clone(), name: name.clone() });
            }
        }
        if !down && self.down {
            if let (Some(pressed), Some((path, name))) = (&self.pressed, &self.over) {
                if pressed == path {
                    events.push(PlayerEvent::Click { path: path.clone(), name: name.clone() });
                }
            }
            self.pressed = None;
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
        walk(p, self.frame, &|path, sym, stateless| Clock::frame(me, path, sym.kind, stateless), &mut |seen| {
            let Seen::Instance(v, _) = seen else { return };
            if v.symbol.kind != SymbolKind::Button {
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
}

/// Chooses an instance's frame during a walk: `(path, symbol, stateless frame) → frame`.
type FrameOf<'a> = dyn Fn(&[(u32, u32)], &Symbol, u32) -> u32 + 'a;

/// Walks the display tree of the root at `frame` (guide layers excluded, as
/// in the player). `frame_of(path, symbol, stateless)` picks each instance's
/// frame; `visit` sees every instance (in render order) with that frame.
/// Records movie-clip frames and sound cues from a walk.
fn collect(seen: Seen, fps: f64, clips: &mut HashMap<InstancePath, u32>, streams: &mut Vec<SoundCue>, events: &mut Vec<SoundCue>) {
    match seen {
        Seen::Instance(v, f) => {
            if v.symbol.kind == SymbolKind::MovieClip {
                clips.insert(v.path.to_vec(), f);
            }
        }
        Seen::Sound { path, layer, start, offset, sound } => match sound.sync {
            SoundSync::Stream => streams.push(cue(path, layer, start, offset, sound, fps)),
            // Events fire on the frame their keyframe is entered.
            SoundSync::Event if offset == 0 => events.push(cue(path, layer, start, offset, sound, fps)),
            SoundSync::Event => {}
        },
    }
}

fn walk(p: &Project, frame: u32, frame_of: &FrameOf, visit: &mut dyn FnMut(Seen)) {
    #[allow(clippy::too_many_arguments)]
    fn go(
        p: &Project,
        symbol: SymbolId,
        frame: u32,
        m: Matrix,
        path: &mut InstancePath,
        depth: usize,
        frame_of: &FrameOf,
        visit: &mut dyn FnMut(Seen),
    ) {
        if depth > MAX_NESTING_DEPTH {
            return;
        }
        let Some(sym) = p.symbol(symbol) else { return };
        for (layer, visible, _) in sym.content_layers() {
            if !visible || layer.kind == LayerKind::Guide {
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
                let stateless = instance_frame(child, &el.kind, &shown, frame);
                let f = frame_of(path, child, stateless).min(child.length() - 1);
                let em = m * el.transform.matrix();
                visit(Seen::Instance(Visit { path, symbol: child, name: &el.name, matrix: em }, f));
                go(p, child_id, f, em, path, depth + 1, frame_of, visit);
                path.pop();
            }
        }
    }
    go(p, p.root, frame, Matrix::IDENTITY, &mut Vec::new(), 0, frame_of, visit);
}

/// Renders a stateless frame (convenience for comparisons).
pub fn render_stateless(p: &Project, frame: u32, opts: RenderOptions, r: &mut dyn Renderer) {
    render_with_clock(p, frame, opts, &Stateless, r);
}
