//! Hardcoded demo scene. Three levels of nesting:
//! Scene → Flower (instanced 3×) → Petal (instanced 6× per flower) → shape,
//! plus a layer folder and a guide layer.

use crate::color::Color;
use crate::math::Point;
use crate::vector::VectorPath;
use crate::model::*;

struct Builder {
    project: Project,
}

/// Where to put a new layer: a symbol's top level or inside a folder.
#[derive(Clone, Copy)]
enum Parent {
    Symbol(SymbolId),
    Folder(LayerId),
}

impl Builder {
    fn id(&mut self) -> u32 {
        self.project.alloc_id()
    }

    fn symbol(&mut self, name: &str, kind: SymbolKind) -> SymbolId {
        let id = SymbolId(self.id());
        self.project.symbols.push(Symbol { id, name: name.into(), kind, layers: Vec::new(), script: None });
        id
    }

    fn layer(&mut self, parent: Parent, name: &str, kind: LayerKind) -> LayerId {
        let id = LayerId(self.id());
        let layer = Layer::new(id, name, kind);
        match parent {
            Parent::Symbol(s) => self.project.symbol_mut(s).unwrap().layers.push(layer),
            Parent::Folder(f) => self.project.layer_mut(f).unwrap().children.push(layer),
        }
        id
    }

    fn add(&mut self, layer: LayerId, name: &str, transform: Transform, kind: ElementKind) {
        let mut e = Element::new(ElementId(self.id()), kind);
        e.name = name.into();
        e.transform = transform;
        self.project.layer_mut(layer).unwrap().keyframes[0].elements.push(e);
    }
}

fn shape(geometry: Geometry, fill: Option<Color>, stroke: Option<(f64, Color)>) -> ElementKind {
    painted(geometry, fill.map(Paint::solid), stroke.map(|(width, color)| Stroke::solid(width, color)))
}

fn painted(geometry: Geometry, fill: Option<Paint>, stroke: Option<Stroke>) -> ElementKind {
    ElementKind::Shape(Shape::new(geometry, fill, stroke))
}

fn stops(colors: &[(f64, Color)]) -> Vec<GradientStop> {
    colors.iter().map(|&(offset, color)| GradientStop { offset, color }).collect()
}

/// A puffy cloud outline drawn with smooth anchors.
fn cloud() -> VectorPath {
    let p = Point::new;
    VectorPath::smooth_through(
        &[p(-90.0, 20.0), p(-70.0, -10.0), p(-35.0, -15.0), p(-10.0, -40.0), p(30.0, -35.0), p(55.0, -12.0), p(90.0, -5.0), p(95.0, 22.0), p(0.0, 28.0)],
        true,
    )
}

pub fn demo_project() -> Project {
    let mut b = Builder {
        project: Project {
            next_id: 1,
            stage: Stage { width: 960.0, height: 540.0, background: Color::rgb(0xf4, 0xef, 0xe6), fps: 24.0 },
            root: SymbolId(0),
            symbols: Vec::new(),
            assets: Vec::new(),
            publish: None,
        },
    };

    let scene = b.symbol("Scene 1", SymbolKind::MovieClip);
    b.project.root = scene;

    let flower = flower_symbol(&mut b);

    let scenery = b.layer(Parent::Symbol(scene), "Scenery", LayerKind::Folder);
    let bg = b.layer(Parent::Folder(scenery), "Background", LayerKind::Normal);
    let sky_gradient = Paint::Linear {
        start: Point::new(0.0, -190.0),
        end: Point::new(0.0, 190.0),
        stops: stops(&[(0.0, Color::rgb(0x7f, 0xc4, 0xec)), (1.0, Color::rgb(0xe4, 0xf4, 0xfa))]),
    };
    b.add(bg, "sky", Transform::at(480.0, 190.0), painted(Geometry::Rect { width: 960.0, height: 380.0 }, Some(sky_gradient), None));
    b.add(bg, "ground", Transform::at(480.0, 460.0), shape(Geometry::Rect { width: 960.0, height: 160.0 }, Some(Color::rgb(0x8c, 0xc0, 0x6b)), None));
    let sky = b.layer(Parent::Folder(scenery), "Sky", LayerKind::Normal);
    let sun_gradient = Paint::Radial {
        center: Point::new(0.0, 0.0),
        radius: 55.0,
        focal: Some(Point::new(-15.0, -15.0)),
        stops: stops(&[(0.0, Color::rgb(0xff, 0xf6, 0xc8)), (0.6, Color::rgb(0xff, 0xd8, 0x4d)), (1.0, Color::rgb(0xf5, 0xa6, 0x23))]),
    };
    b.add(sky, "sun", Transform::at(820.0, 90.0), painted(Geometry::Ellipse { width: 110.0, height: 110.0 }, Some(sun_gradient), None));
    let cloud_stroke = Stroke { dash: vec![6.0, 4.0], ..Stroke::solid(2.0, Color::rgba(0x8a, 0xa8, 0xc0, 0xcc)) };
    b.add(sky, "cloud", Transform::at(600.0, 85.0), painted(Geometry::Path(cloud()), Some(Paint::solid(Color::WHITE)), Some(cloud_stroke)));

    let flowers = b.layer(Parent::Symbol(scene), "Flowers", LayerKind::Normal);
    let place = |x, y, s: f64, rotation, skew_x| Transform { x, y, scale_x: s, scale_y: s, rotation, skew_x, ..Default::default() };
    b.add(flowers, "flower A", place(220.0, 250.0, 0.8, -8.0, 0.0), ElementKind::instance(flower));
    b.add(flowers, "flower B", place(480.0, 210.0, 1.0, 0.0, 0.0), ElementKind::instance(flower));
    b.add(flowers, "flower C", place(730.0, 290.0, 0.6, 12.0, -15.0), ElementKind::instance(flower));

    let guides = b.layer(Parent::Symbol(scene), "Layout guide", LayerKind::Guide);
    b.add(
        guides,
        "horizon",
        Transform::at(480.0, 380.0),
        shape(Geometry::Line { dx: 960.0, dy: 0.0 }, None, Some((1.0, Color::rgb(0x00, 0x99, 0xff)))),
    );

    animate(&mut b, scene, sky);
    bees_and_button(&mut b, scene);
    title_and_tune(&mut b, scene);
    let actions = b.layer(Parent::Symbol(scene), "Actions", LayerKind::Normal);
    b.project.layer_mut(actions).unwrap().keyframes = vec![Keyframe { script: Some(DEMO_SCRIPT.into()), ..Keyframe::blank(48) }];

    debug_assert!(b.project.validate().is_ok());
    b.project
}

/// Two seconds of animation at 24 fps: every scene layer spans 48 frames;
/// on the Sky layer the sun rises and grows and the cloud drifts, as an
/// eased motion tween from frame 0 to frame 36, then holds.
fn animate(b: &mut Builder, scene: SymbolId, sky: LayerId) {
    const LENGTH: u32 = 48;
    const TWEEN: u32 = 36;
    let mut ids = Vec::new();
    walk_layers(&b.project.symbol(scene).unwrap().layers, &mut |l| {
        if !l.is_folder() {
            ids.push(l.id);
        }
    });
    for id in ids {
        b.project.layer_mut(id).unwrap().keyframes[0].duration = LENGTH;
    }
    let start = b.project.layer(sky).unwrap().keyframes[0].elements.clone();
    let end: Vec<Element> = start
        .iter()
        .map(|e| {
            let mut c = e.clone();
            c.track = Some(e.id.0);
            c.id = ElementId(b.id());
            match e.name.as_str() {
                "sun" => c.transform = Transform { y: 60.0, scale_x: 1.15, scale_y: 1.15, ..c.transform },
                "cloud" => c.transform.x = 700.0,
                _ => {}
            }
            c
        })
        .collect();
    let layer = b.project.layer_mut(sky).unwrap();
    layer.keyframes[0].duration = TWEEN;
    layer.keyframes[0].tween = Some(Tween { kind: TweenKind::Motion, easing: Easing::Preset { name: EasePreset::EaseInOutSine }, rotate: 0 });
    layer.keyframes.push(Keyframe::with(LENGTH - TWEEN, end));
}

/// Keyframes for one layer from `(duration, elements, tweened)` triples.
fn keyframes(spans: Vec<(u32, Vec<Element>, bool)>) -> Vec<Keyframe> {
    spans
        .into_iter()
        .map(|(duration, elements, tweened)| Keyframe {
            tween: tweened.then_some(Tween { kind: TweenKind::Motion, easing: Easing::Preset { name: EasePreset::EaseInOutSine }, rotate: 0 }),
            ..Keyframe::with(duration, elements)
        })
        .collect()
}

/// A "Bee" movie clip (its own 13-frame hover loop) instanced three times,
/// appearing at frames 0, 8 and 16 — so each runs its own clock, out of
/// phase with the others — and a four-state "Button".
fn bees_and_button(b: &mut Builder, scene: SymbolId) {
    let bee = bee_symbol(b);

    // --- Three bee instances arriving at frames 0, 8 and 16.
    let bees = b.layer(Parent::Symbol(scene), "Bees", LayerKind::Normal);
    let place = |x: f64, y: f64, s: f64| Transform { x, y, scale_x: s, scale_y: s, ..Default::default() };
    let bee1 = el(b, "bee1", place(150.0, 130.0, 1.0), ElementKind::instance(bee));
    let bee2 = el(b, "bee2", place(370.0, 70.0, 0.8), ElementKind::instance(bee));
    let bee3 = el(b, "bee3", place(640.0, 160.0, 1.2), ElementKind::instance(bee));
    let k1 = vec![copy(b, &bee1, &|_| {}), bee2.clone()];
    let k2 = vec![copy(b, &bee1, &|_| {}), copy(b, &bee2, &|_| {}), bee3];
    b.project.layer_mut(bees).unwrap().keyframes = keyframes(vec![(8, vec![bee1], false), (8, k1, false), (32, k2, false)]);

    let button = button_symbol(b);
    let ui = b.layer(Parent::Symbol(scene), "UI", LayerKind::Normal);
    let btn = el(b, "playButton", Transform::at(100.0, 500.0), ElementKind::instance(button));
    b.project.layer_mut(ui).unwrap().keyframes = keyframes(vec![(48, vec![btn], false)]);
}

fn el(b: &mut Builder, name: &str, t: Transform, kind: ElementKind) -> Element {
    let mut e = Element::new(ElementId(b.id()), kind);
    e.name = name.into();
    e.transform = t;
    e
}

/// A copy of `e` on the same track (so tweens pair them), adjusted by `f`.
fn copy(b: &mut Builder, e: &Element, f: &dyn Fn(&mut Element)) -> Element {
    let mut c = e.clone();
    c.track = Some(e.track());
    c.id = ElementId(b.id());
    f(&mut c);
    c
}

/// "Bee" (movie clip): body, stripes and wings bobbing up and back down
/// over a 13-frame loop.
fn bee_symbol(b: &mut Builder) -> SymbolId {
    let bee = b.symbol("Bee", SymbolKind::MovieClip);
    let body_layer = b.layer(Parent::Symbol(bee), "Body", LayerKind::Normal);
    let yellow = Color::rgb(0xff, 0xc8, 0x2e);
    let dark = Color::rgb(0x2b, 0x22, 0x1a);
    let body = el(b, "body", Transform::default(), shape(Geometry::Ellipse { width: 46.0, height: 30.0 }, Some(yellow), Some((2.0, dark))));
    let stripe1 = el(b, "", Transform::at(-6.0, 0.0), shape(Geometry::Rect { width: 5.0, height: 26.0 }, Some(dark), None));
    let stripe2 = el(b, "", Transform::at(6.0, 0.0), shape(Geometry::Rect { width: 5.0, height: 24.0 }, Some(dark), None));
    let wing_fill = Color::rgba(0xff, 0xff, 0xff, 0xb0);
    let wing1 = el(b, "", Transform { x: -6.0, y: -20.0, rotation: -20.0, ..Default::default() }, shape(Geometry::Ellipse { width: 18.0, height: 26.0 }, Some(wing_fill), Some((1.0, dark))));
    let wing2 = el(b, "", Transform { x: 8.0, y: -18.0, rotation: 25.0, ..Default::default() }, shape(Geometry::Ellipse { width: 16.0, height: 22.0 }, Some(wing_fill), Some((1.0, dark))));
    let parts = vec![body, stripe1, stripe2, wing1, wing2];
    let up: Vec<Element> = parts.iter().map(|e| copy(b, e, &|c| c.transform.y -= 14.0)).collect();
    let down: Vec<Element> = parts.iter().map(|e| copy(b, e, &|_| {})).collect();
    b.project.layer_mut(body_layer).unwrap().keyframes = keyframes(vec![(6, parts, true), (6, up, true), (1, down, false)]);
    bee
}

/// "Button": Up / Over / Down / Hit frames of a blue ▶ pill.
fn button_symbol(b: &mut Builder) -> SymbolId {
    let button = b.symbol("Button", SymbolKind::Button);
    let face = b.layer(Parent::Symbol(button), "Face", LayerKind::Normal);
    let state = |b: &mut Builder, fill: Color, y: f64| {
        let base = el(b, "", Transform::at(0.0, y), shape(Geometry::Rect { width: 140.0, height: 44.0 }, Some(fill), Some((2.0, Color::rgb(0x1d, 0x3c, 0x78)))));
        let tri = Geometry::Path(VectorPath::polyline(&[Point::new(-8.0, -10.0), Point::new(10.0, 0.0), Point::new(-8.0, 10.0)], true));
        let icon = el(b, "", Transform::at(0.0, y), shape(tri, Some(Color::WHITE), None));
        vec![base, icon]
    };
    let up_state = state(b, Color::rgb(0x3a, 0x7b, 0xd5), 0.0);
    let over_state = state(b, Color::rgb(0x5a, 0x9b, 0xf5), 0.0);
    let down_state = state(b, Color::rgb(0x25, 0x5a, 0xa8), 2.0);
    let hit = vec![el(b, "", Transform::default(), shape(Geometry::Rect { width: 140.0, height: 44.0 }, Some(Color::BLACK), None))];
    b.project.layer_mut(face).unwrap().keyframes = keyframes(vec![(1, up_state, false), (1, over_state, false), (1, down_state, false), (1, hit, false)]);
    button
}

/// "Flower" (graphic): a stem, six "Petal" graphic instances and a center.
fn flower_symbol(b: &mut Builder) -> SymbolId {
    let petal = b.symbol("Petal", SymbolKind::Graphic);
    let pl = b.layer(Parent::Symbol(petal), "Petal", LayerKind::Normal);
    b.add(
        pl,
        "",
        Transform::at(0.0, -52.0),
        shape(
            Geometry::Ellipse { width: 44.0, height: 96.0 },
            Some(Color::rgb(0xe8, 0x6a, 0x92)),
            Some((2.0, Color::rgb(0xa8, 0x3a, 0x62))),
        ),
    );

    let flower = b.symbol("Flower", SymbolKind::Graphic);
    let stem = b.layer(Parent::Symbol(flower), "Stem", LayerKind::Normal);
    b.add(stem, "stem", Transform::at(0.0, 150.0), shape(Geometry::Rect { width: 10.0, height: 300.0 }, Some(Color::rgb(0x4c, 0x8c, 0x4a)), None));
    let petals = b.layer(Parent::Symbol(flower), "Petals", LayerKind::Normal);
    for i in 0..6 {
        let t = Transform { rotation: i as f64 * 60.0, ..Default::default() };
        b.add(petals, "", t, ElementKind::instance(petal));
    }
    let center = b.layer(Parent::Symbol(flower), "Center", LayerKind::Normal);
    b.add(
        center,
        "center",
        Transform::default(),
        shape(Geometry::Ellipse { width: 56.0, height: 56.0 }, Some(Color::rgb(0xf5, 0xc5, 0x18)), Some((3.0, Color::rgb(0xb0, 0x7d, 0x10)))),
    );
    flower
}


/// A title in the bundled font and a two-second chime melody streamed in
/// sync with the main timeline (both embedded in the project).
fn title_and_tune(b: &mut Builder, scene: SymbolId) {
    use crate::asset::{Asset, AssetKind, Bytes};
    use crate::text::{TextAlign, TextBlock};

    let font = default_font(b);
    let title = b.layer(Parent::Symbol(scene), "Title", LayerKind::Normal);
    let block = TextBlock {
        text: "Spring in Zoetrope".into(),
        font,
        size: 44.0,
        fill: Paint::Linear {
            start: Point::new(0.0, 0.0),
            end: Point::new(0.0, 50.0),
            stops: stops(&[(0.0, Color::rgb(0x2b, 0x3a, 0x67)), (1.0, Color::rgb(0x6b, 0x3f, 0x8f))]),
        },
        align: TextAlign::Left,
        letter_spacing: 1.0,
        line_height: 1.25,
        width: None,
    };
    let mut t = Element::new(ElementId(b.id()), ElementKind::Text(block));
    t.name = "title".into();
    t.transform = Transform::at(270.0, 16.0);
    b.project.layer_mut(title).unwrap().keyframes = vec![Keyframe::with(48, vec![t])];

    let tune = AssetId(b.id());
    b.project.assets.push(Asset {
        id: tune,
        name: "chime.wav".into(),
        kind: AssetKind::Audio { mime: "audio/wav".into(), duration: 2.0, data: Bytes(chime_wav().into()) },
    });
    let sound = b.layer(Parent::Symbol(scene), "Sound", LayerKind::Normal);
    b.project.layer_mut(sound).unwrap().keyframes =
        vec![Keyframe { sound: Some(SoundRef { asset: tune, sync: SoundSync::Stream, volume: 0.8, loops: 0 }), ..Keyframe::blank(48) }];
}

/// Embeds the bundled font.
fn default_font(b: &mut Builder) -> AssetId {
    use crate::asset::{Asset, AssetKind, Bytes};
    use crate::text::{DEFAULT_FONT, DEFAULT_FONT_NAME};
    let font = AssetId(b.id());
    b.project.assets.push(Asset {
        id: font,
        name: format!("{DEFAULT_FONT_NAME}.ttf"),
        kind: AssetKind::Font { family: DEFAULT_FONT_NAME.into(), data: Bytes(DEFAULT_FONT.into()) },
    });
    font
}

/// The Phase 7 gate demo, "Bee Catcher": steer the bee with the arrow keys
/// and catch flowers before the time runs out; the button restarts. All
/// behaviour is in frame and symbol scripts (see `GAME_PLAY_SCRIPT`).
pub fn game_project() -> Project {
    use crate::text::{TextAlign, TextBlock};
    let mut b = Builder {
        project: Project {
            next_id: 1,
            stage: Stage { width: 960.0, height: 540.0, background: Color::rgb(0xf4, 0xef, 0xe6), fps: 30.0 },
            root: SymbolId(0),
            symbols: Vec::new(),
            assets: Vec::new(),
            publish: None,
        },
    };
    let scene = b.symbol("Bee Catcher", SymbolKind::MovieClip);
    b.project.root = scene;
    // The target is a flower head (no stem), so catching it means touching it.
    let flower = flower_symbol(&mut b);
    let blossom = b.project.symbol_mut(flower).unwrap();
    blossom.name = "Blossom".into();
    blossom.layers.retain(|l| l.name != "Stem");
    blossom.script = Some(GAME_FLOWER_SCRIPT.into());
    let bee = bee_symbol(&mut b);
    let button = button_symbol(&mut b);
    let font = default_font(&mut b);
    let text = |s: &str, size: f64, color: Color, align: TextAlign, width: Option<f64>| {
        ElementKind::Text(TextBlock {
            text: s.into(),
            font,
            size,
            fill: Paint::solid(color),
            align,
            letter_spacing: 0.0,
            line_height: 1.25,
            width,
        })
    };
    let ink = Color::rgb(0x2b, 0x3a, 0x67);

    let bg = b.layer(Parent::Symbol(scene), "Background", LayerKind::Normal);
    let sky = Paint::Linear {
        start: Point::new(0.0, -270.0),
        end: Point::new(0.0, 270.0),
        stops: stops(&[(0.0, Color::rgb(0x7f, 0xc4, 0xec)), (1.0, Color::rgb(0xe4, 0xf4, 0xfa))]),
    };
    let sky = el(&mut b, "sky", Transform::at(480.0, 270.0), painted(Geometry::Rect { width: 960.0, height: 540.0 }, Some(sky), None));
    let ground = el(&mut b, "ground", Transform::at(480.0, 515.0), shape(Geometry::Rect { width: 960.0, height: 50.0 }, Some(Color::rgb(0x8c, 0xc0, 0x6b)), None));
    b.project.layer_mut(bg).unwrap().keyframes = vec![Keyframe::with(2, vec![sky, ground])];

    let small = Transform { x: 600.0, y: 300.0, scale_x: 0.45, scale_y: 0.45, ..Default::default() };
    let target = b.layer(Parent::Symbol(scene), "Flower", LayerKind::Normal);
    let f = el(&mut b, "flower", small, ElementKind::instance(flower));
    b.project.layer_mut(target).unwrap().keyframes = vec![Keyframe::with(1, vec![f]), Keyframe::blank(1)];

    let player = b.layer(Parent::Symbol(scene), "Bee", LayerKind::Normal);
    let pb = el(&mut b, "bee", Transform::at(140.0, 300.0), ElementKind::instance(bee));
    b.project.layer_mut(player).unwrap().keyframes = vec![Keyframe::with(1, vec![pb]), Keyframe::blank(1)];

    let hud = b.layer(Parent::Symbol(scene), "HUD", LayerKind::Normal);
    let score = el(&mut b, "scoreText", Transform::at(24.0, 16.0), text("Score: 0", 30.0, ink, TextAlign::Left, None));
    let time = el(&mut b, "timeText", Transform::at(736.0, 16.0), text("Time: 15", 30.0, ink, TextAlign::Right, Some(200.0)));
    let hint = el(&mut b, "", Transform::at(280.0, 22.0), text("Arrow keys: fly · catch the flowers", 20.0, ink, TextAlign::Center, Some(400.0)));
    let restart = el(&mut b, "restartButton", Transform::at(860.0, 500.0), ElementKind::instance(button));
    let label = el(&mut b, "", Transform::at(660.0, 488.0), text("Restart", 22.0, ink, TextAlign::Right, Some(120.0)));
    b.project.layer_mut(hud).unwrap().keyframes = vec![Keyframe::with(2, vec![score, time, hint, restart, label])];

    let message = b.layer(Parent::Symbol(scene), "Message", LayerKind::Normal);
    let fin = el(&mut b, "finalText", Transform::at(130.0, 220.0), text("Time's up!", 48.0, ink, TextAlign::Center, Some(700.0)));
    b.project.layer_mut(message).unwrap().keyframes = vec![Keyframe::blank(1), Keyframe::with(1, vec![fin])];

    let actions = b.layer(Parent::Symbol(scene), "Actions", LayerKind::Normal);
    b.project.layer_mut(actions).unwrap().keyframes = vec![
        Keyframe { label: Some("play".into()), script: Some(GAME_PLAY_SCRIPT.into()), ..Keyframe::blank(1) },
        Keyframe { label: Some("over".into()), script: Some(GAME_OVER_SCRIPT.into()), ..Keyframe::blank(1) },
    ];

    debug_assert!(b.project.validate().is_ok());
    b.project
}

/// Frame 1 of the animation demo: the ▶ button pauses and resumes it.
pub const DEMO_SCRIPT: &str = r#"// The ▶ button pauses and resumes the movie.
playButton.onClick = function () {
  if (isPlaying) stop();
  else play();
};
"#;

/// Frame 1 ("play") of the game demo.
pub const GAME_PLAY_SCRIPT: &str = r#"// Bee Catcher: steer with the arrow keys, catch flowers before time runs out.
stop();

const speed = 7;
var score = 0;                  // `var`s are properties of this timeline
var timeLeft = 15 * stage.fps;  // in frames

function placeFlower() {
  flower.x = 80 + Math.random() * (stage.width - 160);
  flower.y = 140 + Math.random() * (stage.height - 240);
}

function startGame() {
  score = 0;
  timeLeft = 15 * stage.fps;
  bee.x = 140;
  bee.y = 300;
  placeFlower();
}
startGame();

this.onEnterFrame = function () {
  const dx = (Key.isDown("ArrowRight") ? 1 : 0) - (Key.isDown("ArrowLeft") ? 1 : 0);
  const dy = (Key.isDown("ArrowDown") ? 1 : 0) - (Key.isDown("ArrowUp") ? 1 : 0);
  bee.x = Math.max(30, Math.min(stage.width - 30, bee.x + dx * speed));
  bee.y = Math.max(80, Math.min(stage.height - 40, bee.y + dy * speed));
  if (bee.hitTestObject(flower)) {
    score += 1;
    placeFlower();
  }
  timeLeft -= 1;
  scoreText.text = "Score: " + score;
  timeText.text = "Time: " + Math.ceil(timeLeft / stage.fps);
  if (timeLeft <= 0) gotoAndStop("over");
};

restartButton.onClick = startGame;
"#;

/// Frame 2 ("over") of the game demo.
pub const GAME_OVER_SCRIPT: &str = r#"stop();
this.onEnterFrame = null;
timeText.text = "Time: 0";
finalText.text = "Time's up! You caught " + score + (score === 1 ? " flower." : " flowers.");
restartButton.onClick = function () {
  gotoAndStop("play");
};
"#;

/// Symbol script of the game's Blossom: every blossom spins on its own.
pub const GAME_FLOWER_SCRIPT: &str = r#"this.onEnterFrame = function () {
  this.rotation += 3;
};
"#;

/// A synthesized 2-second, 16-bit mono 22.05 kHz WAV: a little arpeggio of
/// soft bell tones (deterministic, no external audio needed).
pub fn chime_wav() -> Vec<u8> {
    const RATE: u32 = 22_050;
    let notes = [523.25, 659.25, 783.99, 1046.5, 783.99, 659.25, 523.25, 392.0]; // C E G C G E C G
    let total = RATE * 2;
    let step = total as usize / notes.len();
    let mut samples = vec![0f64; total as usize];
    for (i, f) in notes.iter().enumerate() {
        for n in 0..(step * 2).min(samples.len() - i * step) {
            let t = n as f64 / RATE as f64;
            let env = (-t * 5.0).exp() * (1.0 - (-t * 400.0).exp());
            let tone = (std::f64::consts::TAU * f * t).sin() + 0.3 * (std::f64::consts::TAU * f * 2.0 * t).sin();
            samples[i * step + n] += 0.32 * env * tone;
        }
    }
    let data_len = total * 2;
    let mut w = Vec::with_capacity(44 + data_len as usize);
    w.extend_from_slice(b"RIFF");
    w.extend_from_slice(&(36 + data_len).to_le_bytes());
    w.extend_from_slice(b"WAVEfmt ");
    w.extend_from_slice(&16u32.to_le_bytes());
    w.extend_from_slice(&1u16.to_le_bytes()); // PCM
    w.extend_from_slice(&1u16.to_le_bytes()); // mono
    w.extend_from_slice(&RATE.to_le_bytes());
    w.extend_from_slice(&(RATE * 2).to_le_bytes());
    w.extend_from_slice(&2u16.to_le_bytes());
    w.extend_from_slice(&16u16.to_le_bytes());
    w.extend_from_slice(b"data");
    w.extend_from_slice(&data_len.to_le_bytes());
    for s in samples {
        w.extend_from_slice(&((s.clamp(-1.0, 1.0) * 32767.0) as i16).to_le_bytes());
    }
    w
}
