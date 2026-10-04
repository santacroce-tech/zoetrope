//! The document model.
//!
//! A `Project` owns a flat list of symbol *definitions* and embedded assets.
//! Each symbol has a tree of layers (folders contain layers); each content
//! layer holds elements, and an element is a leaf (`Shape`, `Bitmap`) or an
//! `Instance` that references another symbol. Rendering starts at the
//! project's root symbol and expands instances recursively, so the evaluated
//! scene graph is a TREE of instances (one symbol definition may appear many
//! times in that tree, each with its own transform — and, from Phase 5, its
//! own clock). Instance references must be acyclic; `Project::validate` and
//! every insert edit enforce this.
//!
//! Ordering conventions (stored data):
//! - Layer lists (`Symbol::layers`, `Layer::children`) are bottom-to-top:
//!   index 0 is drawn first (furthest back).
//! - `Keyframe::elements` is back-to-front: index 0 is drawn first.
//!
//! Content layers hold their elements in keyframes (see `timeline`).

use crate::asset::{Asset, AssetKind};
use crate::color::{Color, ColorTransform};
use crate::error::{Error, Result};
use crate::geom::Path;
use crate::math::{Matrix, Point};
pub use crate::paint::{FillRule, GradientStop, LineCap, LineJoin, Paint, PaintStyle, Stroke};
pub use crate::text::{TextAlign, TextBlock};
pub use crate::timeline::{EasePreset, Easing, Keyframe, SoundRef, SoundSync, Tween, TweenKind};
pub use crate::vector::{HandleSide, Node, NodeKind, NodeRef, SubPath, VectorPath};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;

macro_rules! id_type {
    ($name:ident) => {
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
        #[serde(transparent)]
        pub struct $name(pub u32);
    };
}

id_type!(SymbolId);
id_type!(LayerId);
id_type!(ElementId);
id_type!(AssetId);

/// Maximum instance nesting depth evaluated at render time. A guard against
/// pathological projects; valid projects are acyclic and far shallower.
pub const MAX_NESTING_DEPTH: usize = 64;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Project {
    /// Next unallocated id. Shared by symbols, layers, elements and assets,
    /// so every id in a project is unique. Monotonic and NOT part of undo.
    pub next_id: u32,
    pub stage: Stage,
    /// The main timeline. Rendering starts here.
    pub root: SymbolId,
    pub symbols: Vec<Symbol>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub assets: Vec<Asset>,
    /// Export ("publish") settings; `None` = defaults.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub publish: Option<Publish>,
}

/// How the exported player fits the stage into its window.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ScaleMode {
    /// The whole stage, as large as fits; bars fill the rest (Flash "show all").
    #[default]
    Letterbox,
    /// Covers the window, keeping proportions; the overflow is cropped (Flash "no border").
    Fill,
    /// Stage pixels 1:1, centered.
    Fixed,
}

/// Single self-contained HTML, or HTML + player script + asset pack.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ExportMode {
    #[default]
    SingleFile,
    Folder,
}

fn default_page_color() -> Color {
    Color::rgb(0x11, 0x11, 0x11)
}

/// Export settings, saved with the project.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Publish {
    /// Page title; empty = the file name.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub title: String,
    #[serde(default)]
    pub scale: ScaleMode,
    #[serde(default)]
    pub mode: ExportMode,
    /// Page color around the stage (letterbox bars, fixed mode margins).
    #[serde(default = "default_page_color")]
    pub page_color: Color,
    /// Wait for a click before playing (so sound can start with the movie).
    #[serde(default)]
    pub start_on_click: bool,
}

impl Default for Publish {
    fn default() -> Self {
        Publish {
            title: String::new(),
            scale: ScaleMode::default(),
            mode: ExportMode::default(),
            page_color: default_page_color(),
            start_on_click: false,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Stage {
    pub width: f64,
    pub height: f64,
    pub background: Color,
    pub fps: f64,
}

impl Stage {
    pub fn validate(&self) -> Result<()> {
        let ok = |v: f64, max: f64| v.is_finite() && v > 0.0 && v <= max;
        if ok(self.width, 16384.0) && ok(self.height, 16384.0) && ok(self.fps, 240.0) {
            Ok(())
        } else {
            Err(Error::Invalid("stage width/height must be in (0, 16384] and fps in (0, 240]".into()))
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Symbol {
    pub id: SymbolId,
    pub name: String,
    /// How instances of this symbol get their frame (see `timeline`).
    #[serde(default)]
    pub kind: SymbolKind,
    pub layers: Vec<Layer>,
    /// Symbol script (JavaScript), run once for every instance when it
    /// appears on stage, with `this` = the instance.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub script: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum SymbolKind {
    /// Timeline synced to the parent's (per-instance first frame + loop mode).
    #[default]
    Graphic,
    /// Independent clock per instance, starting when the instance appears.
    MovieClip,
    /// Frames are Up, Over, Down, Hit; the pointer picks the state.
    Button,
}

/// How a graphic instance maps its parent's frames onto its own timeline.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum LoopMode {
    #[default]
    Loop,
    PlayOnce,
    SingleFrame,
}

fn is_default<T: Default + PartialEq>(v: &T) -> bool {
    *v == T::default()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum LayerKind {
    /// Holds elements; rendered everywhere.
    #[default]
    Normal,
    /// Holds elements; visible while authoring but never in the player/export.
    Guide,
    /// Holds child layers (no elements). Hiding/locking cascades to children.
    Folder,
    /// Holds elements *and* child layers: the children are drawn only where
    /// the mask's filled areas are (the mask itself isn't drawn). Children
    /// must be normal or guide layers. Hiding/locking cascades like a folder.
    /// In the editor a mask only clips while it is locked (as in Flash), so
    /// its shapes stay visible and editable otherwise.
    Mask,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Layer {
    pub id: LayerId,
    pub name: String,
    #[serde(default)]
    pub kind: LayerKind,
    #[serde(default = "yes")]
    pub visible: bool,
    #[serde(default)]
    pub locked: bool,
    /// Content layers only: contiguous keyframe spans from frame 0.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub keyframes: Vec<Keyframe>,
    /// Folders and masks only. Bottom-to-top.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub children: Vec<Layer>,
}

fn yes() -> bool {
    true
}

fn one() -> f64 {
    1.0
}

fn is_one(v: &f64) -> bool {
    *v == 1.0
}

impl Layer {
    pub fn new(id: LayerId, name: impl Into<String>, kind: LayerKind) -> Layer {
        let keyframes = if kind == LayerKind::Folder { Vec::new() } else { vec![Keyframe::blank(1)] };
        Layer { id, name: name.into(), kind, visible: true, locked: false, keyframes, children: Vec::new() }
    }

    pub fn is_folder(&self) -> bool {
        self.kind == LayerKind::Folder
    }

    /// Folders and masks hold child layers.
    pub fn holds_layers(&self) -> bool {
        matches!(self.kind, LayerKind::Folder | LayerKind::Mask)
    }

    pub fn is_mask(&self) -> bool {
        self.kind == LayerKind::Mask
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum BlendMode {
    #[default]
    Normal,
    /// Composites the element as an isolated group (so opacity applies to
    /// the group as a whole rather than to each primitive).
    Layer,
    Multiply,
    Screen,
    Overlay,
    Darken,
    Lighten,
    HardLight,
    Difference,
    Add,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Tint {
    pub color: Color,
    /// 0 = no tint, 1 = solid color.
    pub amount: f64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Element {
    pub id: ElementId,
    /// Identity across keyframes for tweening: copies made by "insert
    /// keyframe" share their source's track. `None` means "my own id".
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub track: Option<u32>,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub transform: Transform,
    /// 0..=1, multiplies alpha.
    #[serde(default = "one", skip_serializing_if = "is_one")]
    pub opacity: f64,
    #[serde(default, skip_serializing_if = "is_normal")]
    pub blend: BlendMode,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tint: Option<Tint>,
    #[serde(flatten)]
    pub kind: ElementKind,
}

fn is_normal(b: &BlendMode) -> bool {
    *b == BlendMode::Normal
}

// Shapes dominate scenes, so boxing them would cost an allocation per element
// for little gain (elements already live in a Vec).
#[allow(clippy::large_enum_variant)]
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum ElementKind {
    Shape(Shape),
    #[serde(rename_all = "camelCase")]
    Instance {
        symbol: SymbolId,
        /// Graphic symbols: frame shown when the parent keyframe starts.
        #[serde(default, skip_serializing_if = "is_default")]
        first_frame: u32,
        /// Graphic symbols: loop / play once / single frame.
        #[serde(default, skip_serializing_if = "is_default")]
        loop_mode: LoopMode,
    },
    /// An embedded image, drawn at its pixel size centered on the origin.
    Bitmap {
        asset: AssetId,
    },
    /// Static text (laid out by the core from an embedded font).
    Text(TextBlock),
}

impl ElementKind {
    pub fn instance(symbol: SymbolId) -> ElementKind {
        ElementKind::Instance { symbol, first_frame: 0, loop_mode: LoopMode::Loop }
    }
}

impl Element {
    pub fn new(id: ElementId, kind: ElementKind) -> Element {
        Element {
            id,
            track: None,
            name: String::new(),
            transform: Transform::default(),
            opacity: 1.0,
            blend: BlendMode::Normal,
            tint: None,
            kind,
        }
    }

    pub fn track(&self) -> u32 {
        self.track.unwrap_or(self.id.0)
    }

    /// Tint then opacity (they touch disjoint channels, so order is moot).
    pub fn color_transform(&self, include_alpha: bool) -> ColorTransform {
        let tint = self.tint.map_or(ColorTransform::IDENTITY, |t| ColorTransform::tint(t.color, t.amount));
        if include_alpha {
            tint.then(&ColorTransform::alpha(self.opacity))
        } else {
            tint
        }
    }

    pub fn validate(&self) -> Result<()> {
        let bad = |m: &str| Err(Error::Invalid(format!("element {}: {m}", self.id.0)));
        if !self.transform.is_finite() {
            return bad("transform has non-finite values");
        }
        if !(0.0..=1.0).contains(&self.opacity) {
            return bad("opacity must be within 0..1");
        }
        if let Some(t) = self.tint {
            if !(0.0..=1.0).contains(&t.amount) {
                return bad("tint amount must be within 0..1");
            }
        }
        if let ElementKind::Shape(s) = &self.kind {
            s.validate().or_else(|e| bad(&e.to_string()))?;
        }
        if let ElementKind::Text(t) = &self.kind {
            t.validate().or_else(|e| bad(&e.to_string()))?;
        }
        Ok(())
    }
}

/// Element transform. The pivot (anchor point, in the element's content
/// coordinates) is placed at `(x, y)`; rotation, skew and scale act around it:
///
/// `matrix = translate(x, y) · rotate(rotation) · skew(skewX, skewY) · scale(scaleX, scaleY) · translate(-pivotX, -pivotY)`
///
/// Angles in degrees; y points down, so positive rotation is clockwise.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Transform {
    pub x: f64,
    pub y: f64,
    pub scale_x: f64,
    pub scale_y: f64,
    pub rotation: f64,
    pub skew_x: f64,
    pub skew_y: f64,
    pub pivot_x: f64,
    pub pivot_y: f64,
}

impl Default for Transform {
    fn default() -> Self {
        Transform {
            x: 0.0,
            y: 0.0,
            scale_x: 1.0,
            scale_y: 1.0,
            rotation: 0.0,
            skew_x: 0.0,
            skew_y: 0.0,
            pivot_x: 0.0,
            pivot_y: 0.0,
        }
    }
}

impl Transform {
    pub fn at(x: f64, y: f64) -> Self {
        Transform { x, y, ..Default::default() }
    }

    /// rotate · skew · scale (no translation, no pivot).
    pub fn linear(&self) -> Matrix {
        Matrix::rotate(self.rotation) * Matrix::skew(self.skew_x, self.skew_y) * Matrix::scale(self.scale_x, self.scale_y)
    }

    pub fn matrix(&self) -> Matrix {
        Matrix::translate(self.x, self.y) * self.linear() * Matrix::translate(-self.pivot_x, -self.pivot_y)
    }

    pub fn pivot(&self) -> Point {
        Point::new(self.pivot_x, self.pivot_y)
    }

    pub fn is_finite(&self) -> bool {
        [self.x, self.y, self.scale_x, self.scale_y, self.rotation, self.skew_x, self.skew_y, self.pivot_x, self.pivot_y]
            .iter()
            .all(|v| v.is_finite())
    }

    /// Moves the pivot to `pivot` (content coordinates) without moving the
    /// element on screen.
    pub fn with_pivot(&self, pivot: Point) -> Transform {
        let m = self.matrix();
        let at = m.apply(pivot);
        Transform { x: at.x, y: at.y, pivot_x: pivot.x, pivot_y: pivot.y, ..*self }
    }

    /// The transform whose matrix is `g · self.matrix()`, keeping the pivot.
    /// Rotation/skew/scale are re-derived from the new linear part, keeping
    /// this transform's parameterization where possible (see `decompose`).
    pub fn transformed_by(&self, g: &Matrix) -> Transform {
        self.with_matrix(&(*g * self.matrix()), self.rotation + g.b.atan2(g.a).to_degrees())
    }

    /// The transform (same pivot) whose matrix is `m`, preferring
    /// `rotation_hint` as its rotation.
    pub fn with_matrix(&self, m: &Matrix, rotation_hint: f64) -> Transform {
        let at = m.apply(self.pivot());
        let d = decompose(&m.linear(), rotation_hint);
        Transform {
            x: clean(at.x),
            y: clean(at.y),
            rotation: d.rotation,
            skew_x: d.skew_x,
            skew_y: d.skew_y,
            scale_x: d.scale_x,
            scale_y: d.scale_y,
            ..*self
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Decomposed {
    pub rotation: f64,
    pub skew_x: f64,
    pub skew_y: f64,
    pub scale_x: f64,
    pub scale_y: f64,
}

/// Splits a linear matrix into `rotate · skew · scale`. The model has one
/// more parameter than a 2×2 matrix has degrees of freedom, so we prefer to
/// keep `rotation_hint` as the rotation and absorb the rest into skew/scale.
/// Falls back to a skewY-free decomposition when that would be degenerate.
pub fn decompose(lin: &Matrix, rotation_hint: f64) -> Decomposed {
    const EPS: f64 = 1e-9;
    let hint = normalize_degrees(rotation_hint);
    // K·S = [[sx, tx·sy], [ty·sx, sy]]
    let m = Matrix::rotate(-hint) * *lin;
    if m.a.abs() > EPS && m.d.abs() > EPS {
        let (sx, sy) = (m.a, m.d);
        let (tx, ty) = (m.c / sy, m.b / sx);
        if tx.abs() < 1e3 && ty.abs() < 1e3 {
            return Decomposed {
                rotation: hint,
                skew_x: clean(tx.atan().to_degrees()),
                skew_y: clean(ty.atan().to_degrees()),
                scale_x: clean(sx),
                scale_y: clean(sy),
            };
        }
    }
    let sx = lin.a.hypot(lin.b);
    let rotation = if sx > EPS { lin.b.atan2(lin.a).to_degrees() } else { 0.0 };
    let m = Matrix::rotate(-rotation) * *lin;
    let sy = m.d;
    let tx = if sy.abs() > EPS { m.c / sy } else { 0.0 };
    Decomposed {
        rotation: clean(normalize_degrees(rotation)),
        skew_x: clean(tx.atan().to_degrees()),
        skew_y: 0.0,
        scale_x: clean(sx).max(EPS),
        scale_y: if sy.abs() > EPS { clean(sy) } else { EPS },
    }
}

/// Rounds away float noise (to 1e-9) so decomposed values stay readable in
/// saved files, e.g. `12` rather than `12.000000000000002`.
pub fn clean(v: f64) -> f64 {
    let r = (v * 1e9).round() / 1e9;
    if r == 0.0 {
        0.0
    } else {
        r
    }
}

/// Maps an angle to (-180, 180].
pub fn normalize_degrees(a: f64) -> f64 {
    let r = a.rem_euclid(360.0);
    let r = if r > 180.0 { r - 360.0 } else { r };
    if r.abs() < 1e-9 {
        0.0
    } else {
        r
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Shape {
    pub geometry: Geometry,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fill: Option<Paint>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stroke: Option<Stroke>,
    #[serde(default, skip_serializing_if = "is_nonzero")]
    pub fill_rule: FillRule,
}

fn is_nonzero(r: &FillRule) -> bool {
    *r == FillRule::NonZero
}

impl Shape {
    pub fn new(geometry: Geometry, fill: Option<Paint>, stroke: Option<Stroke>) -> Shape {
        Shape { geometry, fill, stroke, fill_rule: FillRule::NonZero }
    }

    pub fn validate(&self) -> Result<()> {
        self.geometry.validate()?;
        if let Some(f) = &self.fill {
            f.validate()?;
        }
        if let Some(s) = &self.stroke {
            s.validate()?;
        }
        Ok(())
    }
}

/// Shape geometry. Primitives are centered on the content origin; paths are
/// free-form in content coordinates.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum Geometry {
    Rect {
        width: f64,
        height: f64,
    },
    Ellipse {
        width: f64,
        height: f64,
    },
    /// From `(-dx/2, -dy/2)` to `(dx/2, dy/2)`.
    Line {
        dx: f64,
        dy: f64,
    },
    Path(VectorPath),
}

impl Geometry {
    pub fn to_path(&self) -> Path {
        match self {
            Geometry::Rect { width, height } => Path::rect(-width / 2.0, -height / 2.0, *width, *height),
            Geometry::Ellipse { width, height } => Path::ellipse(0.0, 0.0, width / 2.0, height / 2.0),
            Geometry::Line { dx, dy } => Path::line(-dx / 2.0, -dy / 2.0, dx / 2.0, dy / 2.0),
            Geometry::Path(v) => v.to_path(),
        }
    }

    /// The editable anchor form of this geometry (converting primitives).
    pub fn to_vector_path(&self) -> VectorPath {
        match self {
            Geometry::Rect { width, height } => VectorPath::rect(*width, *height),
            Geometry::Ellipse { width, height } => VectorPath::ellipse(*width, *height),
            Geometry::Line { dx, dy } => {
                VectorPath::polyline(&[Point::new(-dx / 2.0, -dy / 2.0), Point::new(dx / 2.0, dy / 2.0)], false)
            }
            Geometry::Path(v) => v.clone(),
        }
    }

    /// Whether a fill applies (lines are never filled).
    pub fn is_fillable(&self) -> bool {
        !matches!(self, Geometry::Line { .. })
    }

    fn validate(&self) -> Result<()> {
        let ok = match self {
            Geometry::Rect { width, height } | Geometry::Ellipse { width, height } => {
                width.is_finite() && height.is_finite() && *width >= 0.0 && *height >= 0.0
            }
            Geometry::Line { dx, dy } => dx.is_finite() && dy.is_finite(),
            Geometry::Path(v) => return v.validate(),
        };
        if ok {
            Ok(())
        } else {
            Err(Error::Invalid("geometry sizes must be finite and non-negative".into()))
        }
    }
}

/// Where an element lives inside the project.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ElementLocation {
    pub symbol: SymbolId,
    pub layer: LayerId,
    pub keyframe: usize,
    pub index: usize,
}

/// Where a layer lives inside its symbol's layer tree.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LayerLocation {
    pub symbol: SymbolId,
    /// Containing folder, or `None` for the symbol's top level.
    pub parent: Option<LayerId>,
    pub index: usize,
}

/// Structural checks on a layer's keyframes: content layers need at least
/// one keyframe, every span is ≥ 1 frame, and tween easings are valid.
pub fn check_keyframes(l: &Layer) -> Result<()> {
    if !l.is_folder() && l.keyframes.is_empty() {
        return Err(Error::Invalid(format!("layer {} has no keyframes", l.id.0)));
    }
    for k in &l.keyframes {
        if k.duration == 0 {
            return Err(Error::Invalid(format!("layer {} has a zero-length keyframe", l.id.0)));
        }
        if let Some(t) = &k.tween {
            t.easing.validate()?;
        }
    }
    Ok(())
}

/// Calls `f` for every layer in the tree (depth-first, parents before children).
pub fn walk_layers<'a>(layers: &'a [Layer], f: &mut impl FnMut(&'a Layer)) {
    for l in layers {
        f(l);
        walk_layers(&l.children, f);
    }
}

pub fn find_layer(layers: &[Layer], id: LayerId) -> Option<&Layer> {
    for l in layers {
        if l.id == id {
            return Some(l);
        }
        if let Some(found) = find_layer(&l.children, id) {
            return Some(found);
        }
    }
    None
}

pub fn find_layer_mut(layers: &mut [Layer], id: LayerId) -> Option<&mut Layer> {
    for l in layers {
        if l.id == id {
            return Some(l);
        }
        if let Some(found) = find_layer_mut(&mut l.children, id) {
            return Some(found);
        }
    }
    None
}

/// `(parent, index)` of layer `id` in the tree rooted at `layers`.
fn find_layer_slot(layers: &[Layer], parent: Option<LayerId>, id: LayerId) -> Option<(Option<LayerId>, usize)> {
    for (i, l) in layers.iter().enumerate() {
        if l.id == id {
            return Some((parent, i));
        }
        if let Some(found) = find_layer_slot(&l.children, Some(l.id), id) {
            return Some(found);
        }
    }
    None
}

impl Project {
    pub fn alloc_id(&mut self) -> u32 {
        let id = self.next_id;
        self.next_id += 1;
        id
    }

    pub fn symbol(&self, id: SymbolId) -> Option<&Symbol> {
        self.symbols.iter().find(|s| s.id == id)
    }

    pub fn symbol_mut(&mut self, id: SymbolId) -> Option<&mut Symbol> {
        self.symbols.iter_mut().find(|s| s.id == id)
    }

    pub fn require_symbol(&self, id: SymbolId) -> Result<&Symbol> {
        self.symbol(id).ok_or_else(|| Error::NotFound(format!("symbol {}", id.0)))
    }

    pub fn asset(&self, id: AssetId) -> Option<&Asset> {
        self.assets.iter().find(|a| a.id == id)
    }

    pub fn locate(&self, id: ElementId) -> Option<ElementLocation> {
        for s in &self.symbols {
            let mut found = None;
            walk_layers(&s.layers, &mut |l| {
                if found.is_none() {
                    for (k, kf) in l.keyframes.iter().enumerate() {
                        if let Some(i) = kf.elements.iter().position(|e| e.id == id) {
                            found = Some(ElementLocation { symbol: s.id, layer: l.id, keyframe: k, index: i });
                        }
                    }
                }
            });
            if found.is_some() {
                return found;
            }
        }
        None
    }

    pub fn locate_layer(&self, id: LayerId) -> Option<LayerLocation> {
        self.symbols.iter().find_map(|s| {
            find_layer_slot(&s.layers, None, id).map(|(parent, index)| LayerLocation { symbol: s.id, parent, index })
        })
    }

    pub fn layer(&self, id: LayerId) -> Option<&Layer> {
        self.symbols.iter().find_map(|s| find_layer(&s.layers, id))
    }

    pub fn layer_mut(&mut self, id: LayerId) -> Option<&mut Layer> {
        self.symbols.iter_mut().find_map(|s| find_layer_mut(&mut s.layers, id))
    }

    pub fn require_layer(&self, id: LayerId) -> Result<&Layer> {
        self.layer(id).ok_or_else(|| Error::NotFound(format!("layer {}", id.0)))
    }

    /// The list a layer lives in: a symbol's top level or a folder's (or mask's) children.
    pub fn layer_list(&self, symbol: SymbolId, parent: Option<LayerId>) -> Result<&Vec<Layer>> {
        let sym = self.symbol(symbol).ok_or_else(|| Error::NotFound(format!("symbol {}", symbol.0)))?;
        match parent {
            None => Ok(&sym.layers),
            Some(pid) => find_layer(&sym.layers, pid)
                .filter(|l| l.holds_layers())
                .map(|l| &l.children)
                .ok_or_else(|| Error::NotFound(format!("layer {}", pid.0))),
        }
    }

    /// The list a layer lives in: a symbol's top level or a folder's children.
    pub fn layer_list_mut(&mut self, symbol: SymbolId, parent: Option<LayerId>) -> Result<&mut Vec<Layer>> {
        let sym = self.symbol_mut(symbol).ok_or_else(|| Error::NotFound(format!("symbol {}", symbol.0)))?;
        match parent {
            None => Ok(&mut sym.layers),
            Some(pid) => {
                let folder = find_layer_mut(&mut sym.layers, pid).ok_or_else(|| Error::NotFound(format!("layer {}", pid.0)))?;
                if !folder.holds_layers() {
                    return Err(Error::Invalid(format!("layer {} is not a folder or mask", pid.0)));
                }
                Ok(&mut folder.children)
            }
        }
    }

    /// True if `ancestor` is `layer` itself or one of its containing folders.
    pub fn layer_is_within(&self, layer: LayerId, ancestor: LayerId) -> bool {
        let mut cur = Some(layer);
        while let Some(id) = cur {
            if id == ancestor {
                return true;
            }
            cur = self.locate_layer(id).and_then(|l| l.parent);
        }
        false
    }

    /// A layer's effective visibility/lock (folders cascade).
    pub fn layer_state(&self, id: LayerId) -> Option<(bool, bool)> {
        let mut visible = true;
        let mut locked = false;
        let mut cur = Some(id);
        while let Some(lid) = cur {
            let l = self.layer(lid)?;
            visible &= l.visible;
            locked |= l.locked;
            cur = self.locate_layer(lid).and_then(|loc| loc.parent);
        }
        Some((visible, locked))
    }

    pub fn element(&self, id: ElementId) -> Option<&Element> {
        let loc = self.locate(id)?;
        Some(&self.layer(loc.layer)?.keyframes[loc.keyframe].elements[loc.index])
    }

    pub fn element_mut(&mut self, id: ElementId) -> Option<&mut Element> {
        let loc = self.locate(id)?;
        Some(&mut self.layer_mut(loc.layer)?.keyframes[loc.keyframe].elements[loc.index])
    }

    pub fn require_element(&self, id: ElementId) -> Result<&Element> {
        self.element(id).ok_or_else(|| Error::NotFound(format!("element {}", id.0)))
    }

    /// True if rendering `from` would (transitively) render `target`.
    pub fn symbol_reaches(&self, from: SymbolId, target: SymbolId) -> bool {
        let mut stack = vec![from];
        let mut seen = HashSet::new();
        while let Some(s) = stack.pop() {
            if s == target {
                return true;
            }
            if !seen.insert(s) {
                continue;
            }
            if let Some(sym) = self.symbol(s) {
                stack.extend(sym.instanced_symbols());
            }
        }
        false
    }

    /// Checks that an element may be placed in `symbol` (references resolve,
    /// no cycles, values in range).
    pub fn check_element_placement(&self, symbol: SymbolId, e: &Element) -> Result<()> {
        e.validate()?;
        match e.kind {
            ElementKind::Instance { symbol: child, .. } => {
                self.require_symbol(child)?;
                if self.symbol_reaches(child, symbol) {
                    return Err(Error::Invalid("a symbol cannot contain an instance of itself".into()));
                }
            }
            ElementKind::Bitmap { asset } => {
                if !matches!(self.asset(asset).map(|a| &a.kind), Some(AssetKind::Image { .. })) {
                    return Err(Error::NotFound(format!("image asset {}", asset.0)));
                }
            }
            ElementKind::Text(ref t) => {
                if self.font_data(t.font).is_none() {
                    return Err(Error::NotFound(format!("font asset {}", t.font.0)));
                }
            }
            ElementKind::Shape(_) => {}
        }
        Ok(())
    }

    /// A keyframe sound must point at an audio asset, with volume in 0..=1.
    pub fn check_sound(&self, s: &SoundRef) -> Result<()> {
        if !matches!(self.asset(s.asset).map(|a| &a.kind), Some(AssetKind::Audio { .. })) {
            return Err(Error::NotFound(format!("audio asset {}", s.asset.0)));
        }
        if !(0.0..=1.0).contains(&s.volume) {
            return Err(Error::Invalid("sound volume must be within 0..1".into()));
        }
        Ok(())
    }

    /// The bytes of a font asset.
    pub fn font_data(&self, id: AssetId) -> Option<&[u8]> {
        match &self.asset(id)?.kind {
            AssetKind::Font { data, .. } => Some(&data.0),
            _ => None,
        }
    }

    /// Lays out a text block with its font (see `text::layout`).
    /// Lays out a text element (cached: see `text::layout_cached`).
    pub fn layout_text(&self, t: &TextBlock) -> Option<std::rc::Rc<crate::text::TextLayout>> {
        crate::text::layout_cached(t, self.font_data(t.font)?)
    }

    /// Structural integrity check, run on every load.
    pub fn validate(&self) -> Result<()> {
        let invalid = |m: String| Err(Error::Invalid(m));
        if self.symbol(self.root).is_none() {
            return invalid(format!("root symbol {} does not exist", self.root.0));
        }
        self.stage.validate()?;
        let mut ids = HashSet::new();
        let mut claim = |id: u32| -> Result<()> {
            if id >= self.next_id {
                return invalid(format!("id {id} is not below nextId {}", self.next_id));
            }
            if !ids.insert(id) {
                return invalid(format!("duplicate id {id}"));
            }
            Ok(())
        };
        for a in &self.assets {
            claim(a.id.0)?;
            match &a.kind {
                AssetKind::Image { width, height, .. } if *width == 0 || *height == 0 => {
                    return invalid(format!("asset {} has zero size", a.id.0))
                }
                AssetKind::Audio { duration, .. } if !(duration.is_finite() && *duration > 0.0) => {
                    return invalid(format!("audio asset {} has no duration", a.id.0))
                }
                _ => {}
            }
        }
        for s in &self.symbols {
            claim(s.id.0)?;
            let mut result = Ok(());
            walk_layers(&s.layers, &mut |l| {
                if result.is_err() {
                    return;
                }
                result = (|| {
                    claim(l.id.0)?;
                    if l.is_folder() && !l.keyframes.is_empty() {
                        return invalid(format!("folder {} contains keyframes", l.id.0));
                    }
                    if !l.holds_layers() && !l.children.is_empty() {
                        return invalid(format!("layer {} has child layers but is not a folder or mask", l.id.0));
                    }
                    if l.is_mask() && l.children.iter().any(|c| c.holds_layers()) {
                        return invalid(format!("mask {} may only mask normal and guide layers", l.id.0));
                    }
                    check_keyframes(l)?;
                    for s in l.keyframes.iter().filter_map(|k| k.sound.as_ref()) {
                        self.check_sound(s)?;
                    }
                    for e in l.all_elements() {
                        claim(e.id.0)?;
                        e.validate()?;
                        match e.kind {
                            ElementKind::Instance { symbol, .. } if self.symbol(symbol).is_none() => {
                                return invalid(format!("element {} instances missing symbol {}", e.id.0, symbol.0))
                            }
                            ElementKind::Bitmap { asset }
                                if !matches!(self.asset(asset).map(|a| &a.kind), Some(AssetKind::Image { .. })) =>
                            {
                                return invalid(format!("element {} references missing image {}", e.id.0, asset.0))
                            }
                            ElementKind::Text(ref t) if self.font_data(t.font).is_none() => {
                                return invalid(format!("text {} references missing font {}", e.id.0, t.font.0))
                            }
                            _ => {}
                        }
                    }
                    Ok(())
                })();
            });
            result?;
        }
        for s in &self.symbols {
            for child in s.instanced_symbols() {
                if self.symbol_reaches(child, s.id) {
                    return invalid(format!("symbol {} ({}) contains itself", s.id.0, s.name));
                }
            }
        }
        Ok(())
    }
}

impl Symbol {
    /// Symbols directly instanced by this symbol's elements.
    pub fn instanced_symbols(&self) -> Vec<SymbolId> {
        let mut out = Vec::new();
        walk_layers(&self.layers, &mut |l| {
            out.extend(l.all_elements().filter_map(|e| match e.kind {
                ElementKind::Instance { symbol, .. } => Some(symbol),
                _ => None,
            }))
        });
        out
    }

    /// Content layers in render order (bottom first) with their effective
    /// visibility and lock state.
    pub fn content_layers(&self) -> Vec<(&Layer, bool, bool)> {
        fn go<'a>(layers: &'a [Layer], vis: bool, lock: bool, out: &mut Vec<(&'a Layer, bool, bool)>) {
            for l in layers {
                let (v, k) = (vis && l.visible, lock || l.locked);
                if l.is_folder() {
                    go(&l.children, v, k, out);
                } else {
                    // A mask's masked layers come before (under) the mask.
                    if l.is_mask() {
                        go(&l.children, v, k, out);
                    }
                    out.push((l, v, k));
                }
            }
        }
        let mut out = Vec::new();
        go(&self.layers, true, false, &mut out);
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn approx(a: &Matrix, b: &Matrix) -> bool {
        a.approx_eq(b, 1e-9)
    }

    #[test]
    fn pivot_is_placed_at_xy() {
        let t = Transform { x: 100.0, y: 50.0, rotation: 90.0, pivot_x: 10.0, pivot_y: 0.0, ..Default::default() };
        let p = t.matrix().apply(Point::new(10.0, 0.0));
        assert!((p.x - 100.0).abs() < 1e-9 && (p.y - 50.0).abs() < 1e-9);
    }

    #[test]
    fn moving_the_pivot_keeps_the_matrix() {
        let t = Transform { x: 3.0, y: 4.0, rotation: 30.0, skew_x: 10.0, scale_x: 2.0, scale_y: 0.5, ..Default::default() };
        let moved = t.with_pivot(Point::new(7.0, -2.0));
        assert!(approx(&t.matrix(), &moved.matrix()));
        assert_eq!((moved.pivot_x, moved.pivot_y), (7.0, -2.0));
    }

    #[test]
    fn transformed_by_matches_matrix_product_and_keeps_rotation() {
        let t = Transform {
            x: 10.0,
            y: 20.0,
            rotation: 35.0,
            skew_x: 12.0,
            skew_y: -7.0,
            scale_x: 1.5,
            scale_y: 0.75,
            pivot_x: 4.0,
            pivot_y: -3.0,
        };
        for g in [
            Matrix::translate(5.0, -2.0),
            Matrix::scale(2.0, 3.0),
            Matrix::rotate(40.0),
            Matrix::skew(20.0, 0.0),
            Matrix::scale(-1.0, 1.0),
            Matrix::translate(9.0, 9.0) * Matrix::rotate(-120.0) * Matrix::scale(0.5, 2.0),
        ] {
            let t2 = t.transformed_by(&g);
            assert!(t2.matrix().approx_eq(&(g * t.matrix()), 1e-7), "g = {g:?}");
            assert_eq!((t2.pivot_x, t2.pivot_y), (t.pivot_x, t.pivot_y));
        }
        // Pure scale / translate keep the authored rotation and skews.
        let s = t.transformed_by(&Matrix::translate(1.0, 1.0));
        assert_eq!((s.rotation, s.skew_x, s.skew_y), (t.rotation, t.skew_x, t.skew_y));
        let r = t.transformed_by(&Matrix::rotate(40.0));
        assert!((r.rotation - 75.0).abs() < 1e-9);
        assert!((r.skew_x - 12.0).abs() < 1e-9 && (r.skew_y + 7.0).abs() < 1e-9);
    }

    #[test]
    fn decompose_handles_degenerate_hint() {
        // Hint 90° off from the true rotation makes K·S degenerate; fallback must still be exact.
        let lin = Matrix::rotate(0.0) * Matrix::scale(2.0, 3.0);
        let d = decompose(&lin, 90.0);
        let back = Matrix::rotate(d.rotation) * Matrix::skew(d.skew_x, d.skew_y) * Matrix::scale(d.scale_x, d.scale_y);
        assert!(approx(&back, &lin));
    }
}
