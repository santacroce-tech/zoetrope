//! High-level editing commands. Each builds primitive `Edit`s from the current
//! project and executes them as one undoable transaction. The UI calls these
//! (through the WASM API) and never constructs edits itself.
//!
//! Coordinates for element operations are in the element's parent symbol
//! space (stage space for the root timeline).

use crate::asset::{sniff_image, Asset, AssetKind, Bytes};
use crate::color::Color;
use crate::edit::{Edit, LayerProps};
use crate::error::{Error, Result};
use crate::geom::Rect;
use crate::history::Document;
use crate::interact::{shape_from_drag, Modifiers, PaintPart, ShapeOptions, ShapeTool};
use crate::math::Matrix;
use crate::math::Point;
use crate::model::*;
use crate::query::element_bounds;
use crate::vector::freehand;
use serde::{Deserialize, Serialize};
use serde_json::Value;

fn not_found(id: ElementId) -> Error {
    Error::NotFound(format!("element {}", id.0))
}

// ---------------------------------------------------------------- elements

pub fn translate_elements(doc: &mut Document, ids: &[ElementId], dx: f64, dy: f64) -> Result<()> {
    let edits = ids
        .iter()
        .map(|id| {
            let mut t = doc.project.require_element(*id)?.transform;
            t.x += dx;
            t.y += dy;
            Ok(Edit::SetTransform { element: *id, transform: t })
        })
        .collect::<Result<Vec<_>>>()?;
    doc.execute("Move", edits)
}

pub fn translate_element(doc: &mut Document, id: ElementId, dx: f64, dy: f64) -> Result<()> {
    translate_elements(doc, &[id], dx, dy)
}

pub fn rotate_element(doc: &mut Document, id: ElementId, degrees: f64) -> Result<()> {
    let mut t = doc.project.require_element(id)?.transform;
    t.rotation = normalize_degrees(t.rotation + degrees);
    doc.execute("Rotate", vec![Edit::SetTransform { element: id, transform: t }])
}

pub fn set_transform(doc: &mut Document, id: ElementId, transform: Transform) -> Result<()> {
    doc.project.require_element(id)?;
    doc.execute("Transform", vec![Edit::SetTransform { element: id, transform }])
}

pub fn delete_elements(doc: &mut Document, ids: &[ElementId]) -> Result<()> {
    for id in ids {
        doc.project.require_element(*id)?;
    }
    doc.execute("Delete", ids.iter().map(|id| Edit::RemoveElement { element: *id }).collect())
}

pub fn delete_element(doc: &mut Document, id: ElementId) -> Result<()> {
    delete_elements(doc, &[id])
}

/// Duplicates each element directly above its original, offset by `(dx, dy)`.
/// Returns the new ids in the same order.
pub fn duplicate_elements(doc: &mut Document, ids: &[ElementId], dx: f64, dy: f64) -> Result<Vec<ElementId>> {
    // Indices are resolved against a scratch copy so each copy lands right
    // above its original even when several share a layer.
    let mut scratch = doc.project.clone();
    let mut edits = Vec::new();
    let mut new_ids = Vec::new();
    for id in ids {
        let loc = scratch.locate(*id).ok_or_else(|| not_found(*id))?;
        let mut copy = scratch.element(*id).unwrap().clone();
        copy.id = ElementId(doc.project.alloc_id());
        scratch.next_id = doc.project.next_id;
        copy.transform.x += dx;
        copy.transform.y += dy;
        new_ids.push(copy.id);
        copy.track = None;
        let edit = Edit::InsertElement { layer: loc.layer, keyframe: loc.keyframe, index: loc.index + 1, element: copy };
        edit.clone().apply(&mut scratch)?;
        edits.push(edit);
    }
    doc.execute("Duplicate", edits)?;
    Ok(new_ids)
}

pub fn duplicate_element(doc: &mut Document, id: ElementId, dx: f64, dy: f64) -> Result<ElementId> {
    Ok(duplicate_elements(doc, &[id], dx, dy)?[0])
}

/// Keys of an element's JSON form that `patch_element` may change.
const PATCHABLE: &[&str] = &[
    "name",
    "transform",
    "opacity",
    "blend",
    "tint",
    "geometry",
    "fill",
    "stroke",
    "fillRule",
    "firstFrame",
    "loopMode",
    "text",
    "font",
    "size",
    "align",
    "letterSpacing",
    "lineHeight",
    "width",
];

/// Keys that only apply to text elements.
const TEXT_KEYS: &[&str] = &["text", "font", "size", "align", "letterSpacing", "lineHeight", "width"];

/// Merges a JSON patch into an element (`transform`, and `stroke` when the
/// shape already has one, merge field by field; other keys replace; `null`
/// clears optional fields). Only
/// `PATCHABLE` keys are accepted; the element's id and type cannot change.
pub fn patch_element(doc: &mut Document, id: ElementId, patch: &Value) -> Result<()> {
    patch_elements(doc, &[id], patch)
}

/// Applies the same patch to several elements as one undo step.
pub fn patch_elements(doc: &mut Document, ids: &[ElementId], patch: &Value) -> Result<()> {
    let obj = patch.as_object().ok_or_else(|| Error::Invalid("patch must be an object".into()))?;
    if let Some(bad) = obj.keys().find(|k| !PATCHABLE.contains(&k.as_str())) {
        return Err(Error::Invalid(format!("property {bad:?} cannot be changed")));
    }
    let mut edits = Vec::new();
    for id in ids {
        let e = doc.project.require_element(*id)?;
        let mut v = serde_json::to_value(e).expect("element serializes");
        for (k, pv) in obj {
            if k == "transform" {
                let t = v["transform"].as_object_mut().expect("transform object");
                let pt = pv.as_object().ok_or_else(|| Error::Invalid("transform patch must be an object".into()))?;
                for (tk, tv) in pt {
                    t.insert(tk.clone(), tv.clone());
                }
            } else if TEXT_KEYS.contains(&k.as_str()) {
                if !matches!(e.kind, ElementKind::Text(_)) {
                    return Err(Error::Invalid(format!("{k:?} only applies to text")));
                }
                v[k] = pv.clone();
            } else if k == "fill" && matches!(e.kind, ElementKind::Text(_)) {
                v[k] = pv.clone();
            } else if k == "firstFrame" || k == "loopMode" {
                if !matches!(e.kind, ElementKind::Instance { .. }) {
                    return Err(Error::Invalid(format!("{k:?} only applies to symbol instances")));
                }
                v[k] = pv.clone();
            } else if k == "fill" || k == "stroke" || k == "geometry" || k == "fillRule" {
                if !matches!(e.kind, ElementKind::Shape(_)) {
                    return Err(Error::Invalid(format!("{k:?} only applies to shapes")));
                }
                // A partial stroke patch merges into each shape's own stroke,
                // so one patch can restyle several shapes without copying paint.
                match (k.as_str(), pv, v.get_mut("stroke")) {
                    ("stroke", Value::Object(patch), Some(Value::Object(stroke))) => {
                        for (sk, sv) in patch {
                            stroke.insert(sk.clone(), sv.clone());
                        }
                    }
                    _ => v[k] = pv.clone(),
                }
            } else {
                v[k] = pv.clone();
            }
        }
        let new: Element = serde_json::from_value(v).map_err(|err| Error::Invalid(err.to_string()))?;
        if new != *e {
            edits.push(Edit::ReplaceElement { element: new });
        }
    }
    doc.execute("Properties", edits)
}

/// Sets an element's scale so its content box measures `width`×`height`
/// (before rotation/skew). Zero-sized content axes are left unchanged.
pub fn set_element_size(doc: &mut Document, id: ElementId, width: f64, height: f64, frame: u32) -> Result<()> {
    let e = doc.project.require_element(id)?;
    let content = crate::query::content_bounds(&doc.project, &e.kind, &crate::math::Matrix::IDENTITY, frame, 0)
        .ok_or_else(|| Error::Invalid("element has no size".into()))?;
    let mut t = e.transform;
    if content.width() > 1e-9 {
        t.scale_x = width / content.width() * t.scale_x.signum();
    }
    if content.height() > 1e-9 {
        t.scale_y = height / content.height() * t.scale_y.signum();
    }
    if t.scale_x == 0.0 || t.scale_y == 0.0 {
        return Err(Error::Invalid("size must be non-zero".into()));
    }
    doc.execute("Resize", vec![Edit::SetTransform { element: id, transform: t }])
}

/// Stroke settings held by the drawing tools.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct StrokeStyle {
    pub color: Color,
    pub width: f64,
    pub cap: LineCap,
    pub join: LineJoin,
    pub dash: Vec<f64>,
}

impl Default for StrokeStyle {
    fn default() -> Self {
        StrokeStyle { color: Color::BLACK, width: 1.0, cap: LineCap::Round, join: LineJoin::Round, dash: Vec::new() }
    }
}

impl StrokeStyle {
    pub fn to_stroke(&self) -> Stroke {
        Stroke { cap: self.cap, join: self.join, dash: self.dash.clone(), ..Stroke::solid(self.width.max(0.0), self.color) }
    }

    pub fn of(s: &Stroke) -> StrokeStyle {
        StrokeStyle { color: s.paint.primary_color(), width: s.width, cap: s.cap, join: s.join, dash: s.dash.clone() }
    }
}

/// Fill and stroke for newly drawn shapes. Gradient fills are fitted to
/// each new shape's bounds.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ShapeStyle {
    pub fill: Option<PaintStyle>,
    pub stroke: Option<StrokeStyle>,
}

impl ShapeStyle {
    /// A shape with this style. Lines (and open paths) are never filled;
    /// they get a default stroke if the style has none.
    fn make_shape(&self, geometry: Geometry) -> Shape {
        let closed = match &geometry {
            Geometry::Line { .. } => false,
            Geometry::Path(v) => v.subpaths.iter().any(|sp| sp.closed),
            _ => true,
        };
        let bounds = geometry.to_path().bounds(&Matrix::IDENTITY);
        let fill = match (&self.fill, closed, bounds) {
            (Some(style), true, Some(b)) => Some(style.fit(b)),
            _ => None,
        };
        let stroke = match &self.stroke {
            Some(s) => Some(s.to_stroke()),
            None if !closed => Some(StrokeStyle::default().to_stroke()),
            None => None,
        };
        Shape::new(geometry, fill, stroke)
    }
}

pub(crate) fn check_target_layer(p: &Project, layer: LayerId) -> Result<()> {
    let l = p.require_layer(layer)?;
    if l.is_folder() {
        return Err(Error::Invalid("select a layer (not a folder) to draw on".into()));
    }
    match p.layer_state(layer) {
        Some((_, true)) => Err(Error::Invalid(format!("layer \"{}\" is locked", l.name))),
        Some((false, _)) => Err(Error::Invalid(format!("layer \"{}\" is hidden", l.name))),
        _ => Ok(()),
    }
}

/// Creates a shape from a shape-tool drag (stage coords) on top of `layer`.
/// Returns `None` (and records nothing) if the drag was too small.
#[allow(clippy::too_many_arguments)]
pub fn create_shape(
    doc: &mut Document,
    layer: LayerId,
    tool: ShapeTool,
    p0: Point,
    p1: Point,
    mods: Modifiers,
    opts: &ShapeOptions,
    style: &ShapeStyle,
    frame: u32,
) -> Result<Option<ElementId>> {
    check_target_layer(&doc.project, layer)?;
    let Some(drag) = shape_from_drag(tool, p0, p1, mods, opts) else {
        return Ok(None);
    };
    let mut el = Element::new(ElementId(doc.project.alloc_id()), ElementKind::Shape(style.make_shape(drag.geometry)));
    el.transform = Transform::at(drag.center.x, drag.center.y);
    let id = el.id;
    let label = match tool {
        ShapeTool::Rect => "Rectangle",
        ShapeTool::Ellipse => "Ellipse",
        ShapeTool::Line => "Line",
        ShapeTool::Polygon => "Polygon",
    };
    let edits = place_on_top(&doc.project, layer, frame, vec![el])?;
    doc.execute(label, edits)?;
    Ok(Some(id))
}

/// Edits that put `elements` on top of `layer`'s keyframe at `frame`. If the
/// layer ends before `frame`, it is first extended with a blank keyframe
/// there (the last span stretches to fill the gap), as one transaction.
pub(crate) fn place_on_top(p: &Project, layer: LayerId, frame: u32, elements: Vec<Element>) -> Result<Vec<Edit>> {
    let l = p.require_layer(layer)?;
    let mut edits = Vec::new();
    let keyframe = match l.keyframe_at(frame) {
        Some((i, _)) => i,
        None => {
            let mut kfs = l.keyframes.clone();
            let gap = frame - l.length();
            if let Some(last) = kfs.last_mut() {
                last.duration += gap;
            }
            kfs.push(Keyframe::blank(1));
            let i = kfs.len() - 1;
            edits.push(Edit::SetKeyframes { layer, keyframes: kfs });
            i
        }
    };
    let base = l.keyframes.get(keyframe).map_or(0, |k| k.elements.len());
    for (n, element) in elements.into_iter().enumerate() {
        edits.push(Edit::InsertElement { layer, keyframe, index: base + n, element });
    }
    Ok(edits)
}

/// Creates a path element from a path in stage coordinates (pen/pencil).
/// The element is positioned at the path's bounds center, which becomes its
/// pivot.
pub fn create_path(
    doc: &mut Document,
    layer: LayerId,
    mut path: VectorPath,
    style: &ShapeStyle,
    label: &str,
    frame: u32,
) -> Result<ElementId> {
    check_target_layer(&doc.project, layer)?;
    path.validate()?;
    let c = path.to_path().bounds(&Matrix::IDENTITY).ok_or_else(|| Error::Invalid("empty path".into()))?.center();
    path.transform(&Matrix::translate(-c.x, -c.y));
    let mut el = Element::new(ElementId(doc.project.alloc_id()), ElementKind::Shape(style.make_shape(Geometry::Path(path))));
    el.transform = Transform::at(c.x, c.y);
    let id = el.id;
    let edits = place_on_top(&doc.project, layer, frame, vec![el])?;
    doc.execute(label, edits)?;
    Ok(id)
}

/// Creates a path from freehand pointer samples (stage coordinates).
/// `tolerance` is the simplification distance in stage units. Returns
/// `None` if the stroke was too short.
pub fn create_freehand(
    doc: &mut Document,
    layer: LayerId,
    points: &[Point],
    smooth: bool,
    tolerance: f64,
    style: &ShapeStyle,
    frame: u32,
) -> Result<Option<ElementId>> {
    check_target_layer(&doc.project, layer)?;
    let Some(path) = freehand(points, tolerance, smooth, tolerance * 4.0) else {
        return Ok(None);
    };
    create_path(doc, layer, path, style, "Pencil", frame).map(Some)
}

fn shape_mut(e: &mut Element) -> Result<&mut Shape> {
    match &mut e.kind {
        ElementKind::Shape(s) => Ok(s),
        _ => Err(Error::Invalid("only shapes can be edited this way".into())),
    }
}

/// Converts rectangles/ellipses/lines to editable paths (same appearance).
pub fn convert_to_path(doc: &mut Document, ids: &[ElementId]) -> Result<()> {
    let mut edits = Vec::new();
    for id in ids {
        let mut e = doc.project.require_element(*id)?.clone();
        let Ok(s) = shape_mut(&mut e) else { continue };
        if matches!(s.geometry, Geometry::Path(_)) {
            continue;
        }
        s.geometry = Geometry::Path(s.geometry.to_vector_path());
        edits.push(Edit::ReplaceElement { element: e });
    }
    doc.execute("Convert to Path", edits)
}

/// Applies `f` to a shape's (converted) path as one undo step. Deletes the
/// element if the path ends up empty.
fn modify_path(doc: &mut Document, id: ElementId, label: &str, f: impl FnOnce(&mut VectorPath) -> Result<()>) -> Result<()> {
    let mut e = doc.project.require_element(id)?.clone();
    let s = shape_mut(&mut e)?;
    let mut path = s.geometry.to_vector_path();
    f(&mut path)?;
    if path.subpaths.is_empty() {
        return doc.execute(label, vec![Edit::RemoveElement { element: id }]);
    }
    s.geometry = Geometry::Path(path);
    doc.execute(label, vec![Edit::ReplaceElement { element: e }])
}

pub fn insert_anchor(doc: &mut Document, id: ElementId, subpath: usize, segment: usize, t: f64) -> Result<NodeRef> {
    let mut out = None;
    modify_path(doc, id, "Add Anchor", |p| {
        out = Some(p.split(subpath, segment, t)?);
        Ok(())
    })?;
    Ok(out.unwrap())
}

pub fn delete_anchors(doc: &mut Document, id: ElementId, nodes: &[NodeRef]) -> Result<()> {
    modify_path(doc, id, "Delete Anchor", |p| {
        p.delete_nodes(nodes);
        Ok(())
    })
}

pub fn convert_anchor(doc: &mut Document, id: ElementId, node: NodeRef) -> Result<()> {
    modify_path(doc, id, "Convert Anchor", |p| p.convert_node(node))
}

/// What the eyedropper picked: the clicked part plus the shape's full style.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PickedStyle {
    pub part: PaintPart,
    pub fill: Option<PaintStyle>,
    pub stroke: Option<StrokeStyle>,
}

pub fn pick_style(p: &Project, scope: &crate::query::Scope, pt: Point, tolerance: f64) -> Option<PickedStyle> {
    let (shape, part) = crate::query::pick_shape(p, scope, pt, tolerance)?;
    Some(PickedStyle { part, fill: shape.fill.as_ref().map(Paint::style), stroke: shape.stroke.as_ref().map(StrokeStyle::of) })
}

/// Sets the fill or stroke paint of shapes from a geometry-free style
/// (gradients are fitted to each shape's bounds). `None` removes it. Setting
/// a stroke paint on a shape without a stroke adds a 1px stroke.
pub fn set_paint_style(doc: &mut Document, ids: &[ElementId], part: PaintPart, style: Option<&PaintStyle>) -> Result<()> {
    let mut edits = Vec::new();
    for id in ids {
        let mut e = doc.project.require_element(*id)?.clone();
        if let ElementKind::Text(t) = &mut e.kind {
            // Text has a fill only; gradients fit its box.
            let (PaintPart::Fill, Some(style)) = (part, style) else {
                return Err(Error::Invalid("text has a fill but no stroke, and the fill can't be removed".into()));
            };
            let l = doc.project.layout_text(t).ok_or_else(|| Error::Invalid("text font is missing".into()))?;
            t.fill = style.fit(Rect::new(0.0, 0.0, l.width.max(1.0), l.height));
            if *doc.project.element(*id).unwrap() != e {
                edits.push(Edit::ReplaceElement { element: e });
            }
            continue;
        }
        let s = shape_mut(&mut e)?;
        let bounds = s.geometry.to_path().bounds(&Matrix::IDENTITY).unwrap_or(Rect::new(0.0, 0.0, 1.0, 1.0));
        let paint = style.map(|st| st.fit(bounds));
        match part {
            PaintPart::Fill => {
                if paint.is_some() && !s.geometry.is_fillable() {
                    return Err(Error::Invalid("lines can't be filled".into()));
                }
                s.fill = paint;
            }
            PaintPart::Stroke => {
                s.stroke = match (paint, s.stroke.take()) {
                    (None, _) => None,
                    (Some(p), Some(st)) => Some(Stroke { paint: p, ..st }),
                    (Some(p), None) => Some(Stroke { paint: p, ..Stroke::solid(1.0, Color::BLACK) }),
                };
            }
        }
        if *doc.project.element(*id).unwrap() != e {
            edits.push(Edit::ReplaceElement { element: e });
        }
    }
    let label = if part == PaintPart::Fill { "Fill" } else { "Stroke" };
    doc.execute(label, edits)
}

/// Embeds an image and places it centered at `at` on top of `layer`.
pub fn import_image(doc: &mut Document, layer: LayerId, name: &str, data: &[u8], at: Point, frame: u32) -> Result<ElementId> {
    check_target_layer(&doc.project, layer)?;
    let info = sniff_image(data)?;
    let asset_id = AssetId(doc.project.alloc_id());
    let asset = Asset {
        id: asset_id,
        name: name.to_string(),
        kind: AssetKind::Image { mime: info.mime.to_string(), width: info.width, height: info.height, data: Bytes(data.into()) },
    };
    let mut el = Element::new(ElementId(doc.project.alloc_id()), ElementKind::Bitmap { asset: asset_id });
    el.name = name.to_string();
    el.transform = Transform::at(at.x, at.y);
    let id = el.id;
    let mut edits = vec![Edit::InsertAsset { index: doc.project.assets.len(), asset }];
    edits.extend(place_on_top(&doc.project, layer, frame, vec![el])?);
    doc.execute("Import Image", edits)?;
    Ok(id)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Align {
    Left,
    CenterX,
    Right,
    Top,
    CenterY,
    Bottom,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Distribute {
    /// Equal spacing between horizontal centers.
    CentersX,
    CentersY,
    /// Equal gaps between neighbors' edges.
    SpaceX,
    SpaceY,
}

fn bounds_of(p: &Project, ids: &[ElementId], frame: u32) -> Result<Vec<(ElementId, Rect)>> {
    ids.iter()
        .map(|id| {
            let e = p.require_element(*id)?;
            Ok((*id, element_bounds(p, e, frame).ok_or_else(|| Error::Invalid("element has no bounds".into()))?))
        })
        .collect()
}

fn translate_edits(p: &Project, moves: impl IntoIterator<Item = (ElementId, f64, f64)>) -> Vec<Edit> {
    moves
        .into_iter()
        .filter(|(_, dx, dy)| dx.abs() > 1e-12 || dy.abs() > 1e-12)
        .map(|(id, dx, dy)| {
            let mut t = p.element(id).unwrap().transform;
            t.x += dx;
            t.y += dy;
            Edit::SetTransform { element: id, transform: t }
        })
        .collect()
}

/// Aligns elements to their combined bounds, or to the stage when
/// `to_stage` is set (always the case for a single element).
pub fn align(doc: &mut Document, ids: &[ElementId], mode: Align, to_stage: bool, frame: u32) -> Result<()> {
    let items = bounds_of(&doc.project, ids, frame)?;
    let reference = if to_stage || items.len() == 1 {
        Rect::new(0.0, 0.0, doc.project.stage.width, doc.project.stage.height)
    } else {
        Rect::union_all(items.iter().map(|(_, b)| *b)).ok_or_else(|| Error::Invalid("nothing to align".into()))?
    };
    let c = reference.center();
    let moves = items.iter().map(|(id, b)| {
        let bc = b.center();
        match mode {
            Align::Left => (*id, reference.min.x - b.min.x, 0.0),
            Align::CenterX => (*id, c.x - bc.x, 0.0),
            Align::Right => (*id, reference.max.x - b.max.x, 0.0),
            Align::Top => (*id, 0.0, reference.min.y - b.min.y),
            Align::CenterY => (*id, 0.0, c.y - bc.y),
            Align::Bottom => (*id, 0.0, reference.max.y - b.max.y),
        }
    });
    let edits = translate_edits(&doc.project, moves.collect::<Vec<_>>());
    doc.execute("Align", edits)
}

/// Distributes elements between the outermost ones (or across the stage
/// when `to_stage`). Needs at least 2 elements (3 unless `to_stage`).
pub fn distribute(doc: &mut Document, ids: &[ElementId], mode: Distribute, to_stage: bool, frame: u32) -> Result<()> {
    let mut items = bounds_of(&doc.project, ids, frame)?;
    if items.len() < if to_stage { 2 } else { 3 } {
        return Err(Error::Invalid("select at least three objects to distribute".into()));
    }
    let horizontal = matches!(mode, Distribute::CentersX | Distribute::SpaceX);
    let lo = |r: &Rect| if horizontal { r.min.x } else { r.min.y };
    let hi = |r: &Rect| if horizontal { r.max.x } else { r.max.y };
    let mid = |r: &Rect| (lo(r) + hi(r)) / 2.0;
    items.sort_by(|a, b| mid(&a.1).total_cmp(&mid(&b.1)).then(a.0.cmp(&b.0)));
    let n = items.len();
    let stage_extent = if horizontal { doc.project.stage.width } else { doc.project.stage.height };
    let mut targets: Vec<f64> = Vec::with_capacity(n); // new `lo` for each item
    match mode {
        Distribute::CentersX | Distribute::CentersY => {
            let (first, last) = if to_stage {
                let size0 = hi(&items[0].1) - lo(&items[0].1);
                let size1 = hi(&items[n - 1].1) - lo(&items[n - 1].1);
                (size0 / 2.0, stage_extent - size1 / 2.0)
            } else {
                (mid(&items[0].1), mid(&items[n - 1].1))
            };
            for (i, (_, b)) in items.iter().enumerate() {
                let c = first + (last - first) * i as f64 / (n - 1) as f64;
                targets.push(c - (hi(b) - lo(b)) / 2.0);
            }
        }
        Distribute::SpaceX | Distribute::SpaceY => {
            let total: f64 = items.iter().map(|(_, b)| hi(b) - lo(b)).sum();
            let (start, end) = if to_stage { (0.0, stage_extent) } else { (lo(&items[0].1), hi(&items[n - 1].1)) };
            let gap = (end - start - total) / (n - 1) as f64;
            let mut cursor = start;
            for (_, b) in &items {
                targets.push(cursor);
                cursor += hi(b) - lo(b) + gap;
            }
        }
    }
    let moves: Vec<_> = items
        .iter()
        .zip(targets)
        .map(|((id, b), t)| if horizontal { (*id, t - lo(b), 0.0) } else { (*id, 0.0, t - lo(b)) })
        .collect();
    let edits = translate_edits(&doc.project, moves);
    doc.execute("Distribute", edits)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Arrange {
    Front,
    Forward,
    Backward,
    Back,
}

/// Changes z-order within each element's layer.
pub fn arrange(doc: &mut Document, ids: &[ElementId], op: Arrange) -> Result<()> {
    let mut slots: Vec<(LayerId, usize)> = Vec::new();
    for id in ids {
        let loc = doc.project.locate(*id).ok_or_else(|| not_found(*id))?;
        if !slots.contains(&(loc.layer, loc.keyframe)) {
            slots.push((loc.layer, loc.keyframe));
        }
    }
    let mut edits = Vec::new();
    for (layer, keyframe) in slots {
        let old: Vec<ElementId> = doc.project.require_layer(layer)?.keyframes[keyframe].elements.iter().map(|e| e.id).collect();
        let sel = |id: &ElementId| ids.contains(id);
        let mut order = old.clone();
        match op {
            Arrange::Front => order = old.iter().filter(|i| !sel(i)).chain(old.iter().filter(|i| sel(i))).copied().collect(),
            Arrange::Back => order = old.iter().filter(|i| sel(i)).chain(old.iter().filter(|i| !sel(i))).copied().collect(),
            Arrange::Forward => {
                for i in (0..order.len().saturating_sub(1)).rev() {
                    if sel(&order[i]) && !sel(&order[i + 1]) {
                        order.swap(i, i + 1);
                    }
                }
            }
            Arrange::Backward => {
                for i in 1..order.len() {
                    if sel(&order[i]) && !sel(&order[i - 1]) {
                        order.swap(i, i - 1);
                    }
                }
            }
        }
        if order != old {
            edits.push(Edit::ReorderElements { layer, keyframe, order });
        }
    }
    doc.execute("Arrange", edits)
}

/// Moves elements to the top of `layer`'s keyframe at `frame` (same symbol).
pub fn move_to_layer(doc: &mut Document, ids: &[ElementId], layer: LayerId, frame: u32) -> Result<()> {
    check_target_layer(&doc.project, layer)?;
    let target_symbol = doc.project.locate_layer(layer).unwrap().symbol;
    let mut removals = Vec::new();
    let mut moved = Vec::new();
    for id in ids {
        let loc = doc.project.locate(*id).ok_or_else(|| not_found(*id))?;
        if loc.symbol != target_symbol {
            return Err(Error::Invalid("elements can only move between layers of the same symbol".into()));
        }
        if loc.layer == layer {
            continue;
        }
        removals.push(Edit::RemoveElement { element: *id });
        moved.push(doc.project.element(*id).unwrap().clone());
    }
    let mut edits = removals;
    edits.extend(place_on_top(&doc.project, layer, frame, moved)?);
    doc.execute("Move to Layer", edits)
}

pub fn set_stage(doc: &mut Document, stage: Stage) -> Result<()> {
    doc.execute("Stage Settings", vec![Edit::SetStage(stage)])
}

pub fn set_stage_background(doc: &mut Document, color: Color) -> Result<()> {
    let stage = Stage { background: color, ..doc.project.stage.clone() };
    set_stage(doc, stage)
}

// ---------------------------------------------------------------- layers

fn layer_count(layers: &[Layer], kind: LayerKind) -> usize {
    let mut n = 0;
    walk_layers(layers, &mut |l| n += (l.kind == kind || (kind == LayerKind::Normal && l.kind == LayerKind::Guide)) as usize);
    n
}

/// Adds a layer directly above `above` (in the same folder), or at the top
/// of the symbol's layer list. Returns the new layer's id.
pub fn add_layer(doc: &mut Document, symbol: SymbolId, above: Option<LayerId>, kind: LayerKind) -> Result<LayerId> {
    let sym = doc.project.require_symbol(symbol)?;
    let name = match kind {
        LayerKind::Folder => format!("Folder {}", layer_count(&sym.layers, LayerKind::Folder) + 1),
        _ => format!("Layer {}", layer_count(&sym.layers, LayerKind::Normal) + 1),
    };
    let (parent, index) = match above {
        Some(a) => {
            let loc = doc.project.locate_layer(a).ok_or_else(|| Error::NotFound(format!("layer {}", a.0)))?;
            if loc.symbol != symbol {
                return Err(Error::Invalid("layer is in another symbol".into()));
            }
            (loc.parent, loc.index + 1)
        }
        None => (None, sym.layers.len()),
    };
    let id = LayerId(doc.project.alloc_id());
    let label = if kind == LayerKind::Folder { "New Folder" } else { "New Layer" };
    doc.execute(label, vec![Edit::InsertLayer { symbol, parent, index, layer: Layer::new(id, name, kind) }])?;
    Ok(id)
}

/// Deletes a layer (a folder with all its contents). A symbol always keeps
/// at least one content layer.
pub fn delete_layer(doc: &mut Document, id: LayerId) -> Result<()> {
    let loc = doc.project.locate_layer(id).ok_or_else(|| Error::NotFound(format!("layer {}", id.0)))?;
    let sym = doc.project.require_symbol(loc.symbol)?;
    let remaining = sym.content_layers().iter().filter(|(l, _, _)| !doc.project.layer_is_within(l.id, id)).count();
    if remaining == 0 {
        return Err(Error::Invalid("a symbol needs at least one layer".into()));
    }
    doc.execute("Delete Layer", vec![Edit::RemoveLayer { layer: id }])
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LayerPatch {
    pub name: Option<String>,
    pub visible: Option<bool>,
    pub locked: Option<bool>,
    /// `normal`, `guide` or `mask` (not to or from `folder`).
    pub kind: Option<LayerKind>,
}

pub fn set_layer_props(doc: &mut Document, id: LayerId, patch: &LayerPatch) -> Result<()> {
    let l = doc.project.require_layer(id)?;
    let mut props = LayerProps::of(l);
    if let Some(n) = &patch.name {
        let n = n.trim();
        if n.is_empty() {
            return Err(Error::Invalid("layer name cannot be empty".into()));
        }
        props.name = n.to_string();
    }
    if let Some(v) = patch.visible {
        props.visible = v;
    }
    if let Some(v) = patch.locked {
        props.locked = v;
    }
    if let Some(k) = patch.kind {
        props.kind = k;
    }
    if props == LayerProps::of(l) {
        return Ok(());
    }
    let label = match patch {
        LayerPatch { name: Some(_), .. } => "Rename Layer",
        LayerPatch { visible: Some(_), .. } => "Show/Hide Layer",
        LayerPatch { locked: Some(_), .. } => "Lock/Unlock Layer",
        LayerPatch { kind: Some(LayerKind::Mask), .. } => "Mask",
        _ => "Layer Type",
    };
    let was_mask = l.is_mask();
    let becomes_mask = props.kind == LayerKind::Mask;
    let loc = doc.project.locate_layer(id).ok_or_else(|| Error::NotFound(format!("layer {}", id.0)))?;
    let mut edits = Vec::new();
    if was_mask && !becomes_mask {
        // Release the masked layers in place: just below where the mask is.
        let children = l.children.clone();
        for c in children.iter().rev() {
            edits.push(Edit::RemoveLayer { layer: c.id });
        }
        edits.push(Edit::SetLayerProps { layer: id, props });
        for (i, c) in children.into_iter().enumerate() {
            edits.push(Edit::InsertLayer { symbol: loc.symbol, parent: loc.parent, index: loc.index + i, layer: c });
        }
    } else if becomes_mask && !was_mask {
        // As in Flash, the layer directly below becomes masked.
        edits.push(Edit::SetLayerProps { layer: id, props });
        let siblings = doc.project.layer_list(loc.symbol, loc.parent)?;
        if let Some(below) = loc.index.checked_sub(1).map(|i| siblings[i].clone()) {
            if matches!(below.kind, LayerKind::Normal | LayerKind::Guide) {
                edits.push(Edit::RemoveLayer { layer: below.id });
                edits.push(Edit::InsertLayer { symbol: loc.symbol, parent: Some(id), index: 0, layer: below });
            }
        }
    } else {
        edits.push(Edit::SetLayerProps { layer: id, props });
    }
    doc.execute(label, edits)
}

/// Moves a layer into `parent` (a folder, or the top level when `None`) at
/// `index` — an index into the destination list *after* the layer has been
/// removed from its current position.
pub fn move_layer(doc: &mut Document, id: LayerId, parent: Option<LayerId>, index: usize) -> Result<()> {
    let loc = doc.project.locate_layer(id).ok_or_else(|| Error::NotFound(format!("layer {}", id.0)))?;
    if let Some(pid) = parent {
        if doc.project.layer_is_within(pid, id) {
            return Err(Error::Invalid("cannot move a folder into itself".into()));
        }
        let ploc = doc.project.locate_layer(pid).ok_or_else(|| Error::NotFound(format!("layer {}", pid.0)))?;
        if ploc.symbol != loc.symbol {
            return Err(Error::Invalid("cannot move layers between symbols".into()));
        }
    }
    if parent == loc.parent && index == loc.index {
        return Ok(());
    }
    let layer = doc.project.require_layer(id)?.clone();
    doc.execute(
        "Move Layer",
        vec![Edit::RemoveLayer { layer: id }, Edit::InsertLayer { symbol: loc.symbol, parent, index, layer }],
    )
}

// ---------------------------------------------------------------- timeline

/// Applies `f` to each content layer's keyframe list (folders are skipped)
/// and records the changed ones as one undo step.
fn edit_timeline(
    doc: &mut Document,
    layers: &[LayerId],
    label: &str,
    mut f: impl FnMut(&mut Project, &Layer) -> Result<Option<Vec<Keyframe>>>,
) -> Result<()> {
    let mut edits = Vec::new();
    for id in layers {
        let layer = doc.project.require_layer(*id)?.clone();
        if layer.is_folder() {
            continue;
        }
        if let Some(kfs) = f(&mut doc.project, &layer)? {
            if kfs != layer.keyframes {
                edits.push(Edit::SetKeyframes { layer: *id, keyframes: kfs });
            }
        }
    }
    doc.execute(label, edits)
}

/// Lengthens the span at `frame` by `count` (F5). Past a layer's end, the
/// last span is stretched to reach `frame` instead.
pub fn insert_frames(doc: &mut Document, layers: &[LayerId], frame: u32, count: u32) -> Result<()> {
    edit_timeline(doc, layers, "Insert Frame", |_, l| {
        let mut kfs = l.keyframes.clone();
        match l.keyframe_at(frame) {
            Some((i, _)) => kfs[i].duration += count.max(1),
            None => kfs.last_mut().expect("content layers have keyframes").duration += frame + 1 - l.length(),
        }
        Ok(Some(kfs))
    })
}

/// Shortens the span at `frame` by one (⇧F5); a span reduced to nothing is
/// removed. A layer always keeps at least one (blank) keyframe.
pub fn remove_frames(doc: &mut Document, layers: &[LayerId], frame: u32, count: u32) -> Result<()> {
    edit_timeline(doc, layers, "Remove Frame", |_, l| {
        let mut kfs = l.keyframes.clone();
        for _ in 0..count.max(1) {
            let probe = Layer { keyframes: kfs.clone(), ..l.clone() };
            let Some((i, _)) = probe.keyframe_at(frame) else {
                break;
            };
            kfs[i].duration -= 1;
            if kfs[i].duration == 0 {
                kfs.remove(i);
            }
        }
        if kfs.is_empty() {
            kfs.push(Keyframe::blank(1));
        }
        Ok(Some(kfs))
    })
}

/// Makes `frame` a keyframe (F6), copying what the layer shows there —
/// including the interpolated state inside a tween — or empty (`blank`, F7).
/// Copies get fresh ids but keep their tracks, so tweens pair them up.
pub fn insert_keyframe(doc: &mut Document, layers: &[LayerId], frame: u32, blank: bool) -> Result<()> {
    let label = if blank { "Insert Blank Keyframe" } else { "Insert Keyframe" };
    edit_timeline(doc, layers, label, |p, l| {
        let mut kfs = l.keyframes.clone();
        let copies = |p: &mut Project, at: u32| -> Vec<Element> {
            if blank {
                return Vec::new();
            }
            crate::timeline::evaluate_layer(l, at)
                .into_iter()
                .map(|e| {
                    let mut c = e.into_owned();
                    c.track = Some(c.track());
                    c.id = ElementId(p.alloc_id());
                    c
                })
                .collect()
        };
        match l.keyframe_at(frame) {
            Some((_, start)) if start == frame => {
                if !blank {
                    return Ok(None); // already a keyframe
                }
                // F7 on a keyframe: insert the blank keyframe right after it.
                return insert_blank_after(l, frame);
            }
            Some((i, start)) => {
                let elements = copies(p, frame);
                let end = start + kfs[i].duration;
                kfs[i].duration = frame - start;
                let tween = kfs[i].tween.clone();
                kfs.insert(i + 1, Keyframe { tween, ..Keyframe::with(end - frame, elements) });
            }
            None => {
                let len = l.length();
                let elements = copies(p, len.saturating_sub(1));
                kfs.last_mut().expect("content layers have keyframes").duration += frame - len;
                kfs.push(Keyframe::with(1, elements));
            }
        }
        Ok(Some(kfs))
    })
}

fn insert_blank_after(l: &Layer, frame: u32) -> Result<Option<Vec<Keyframe>>> {
    let mut kfs = l.keyframes.clone();
    let (i, _) = l.keyframe_at(frame).expect("keyframe exists");
    if kfs[i].duration > 1 {
        let rest = kfs[i].duration - 1;
        kfs[i].duration = 1;
        kfs.insert(i + 1, Keyframe::blank(rest));
    } else {
        kfs.insert(i + 1, Keyframe::blank(1));
    }
    Ok(Some(kfs))
}

/// Removes the keyframe starting at `frame` (⇧F6): its span joins the
/// previous keyframe and its contents are dropped.
pub fn clear_keyframe(doc: &mut Document, layers: &[LayerId], frame: u32) -> Result<()> {
    edit_timeline(doc, layers, "Clear Keyframe", |_, l| {
        let Some((i, start)) = l.keyframe_at(frame) else {
            return Ok(None);
        };
        if start != frame {
            return Err(Error::Invalid("that frame is not a keyframe".into()));
        }
        if i == 0 {
            return Err(Error::Invalid("the first keyframe can't be cleared".into()));
        }
        let mut kfs = l.keyframes.clone();
        let removed = kfs.remove(i);
        kfs[i - 1].duration += removed.duration;
        Ok(Some(kfs))
    })
}

/// Sets (or with `None`, removes) the tween of the keyframe spanning
/// `frame`. A tween needs a following keyframe to tween toward.
pub fn set_tween(doc: &mut Document, layers: &[LayerId], frame: u32, tween: Option<Tween>) -> Result<()> {
    if let Some(t) = &tween {
        t.easing.validate()?;
    }
    let label = match &tween {
        None => "Remove Tween",
        Some(Tween { kind: TweenKind::Motion, .. }) => "Motion Tween",
        Some(Tween { kind: TweenKind::Shape, .. }) => "Shape Tween",
    };
    edit_timeline(doc, layers, label, |_, l| {
        let Some((i, _)) = l.keyframe_at(frame) else {
            return Ok(None);
        };
        if tween.is_some() && i + 1 >= l.keyframes.len() {
            return Err(Error::Invalid("insert a keyframe later on this layer to tween toward (F6)".into()));
        }
        let mut kfs = l.keyframes.clone();
        kfs[i].tween = tween.clone();
        Ok(Some(kfs))
    })
}

// ---------------------------------------------------------------- symbols

/// Moves `ids` (elements of one symbol, shown at `frame`) into a new symbol
/// whose registration point is the selection's bounds center, and puts one
/// instance in their place (on the top-most source layer, where the
/// front-most element was). Returns the new instance's id and the symbol.
pub fn convert_to_symbol(
    doc: &mut Document,
    ids: &[ElementId],
    name: &str,
    kind: SymbolKind,
    frame: u32,
) -> Result<(ElementId, SymbolId)> {
    if ids.is_empty() {
        return Err(Error::Invalid("select something to convert".into()));
    }
    let p = &doc.project;
    let mut items = Vec::new();
    for id in ids {
        let loc = p.locate(*id).ok_or_else(|| not_found(*id))?;
        let layer = p.require_layer(loc.layer)?;
        if crate::timeline::is_tweened_frame(layer, frame) {
            return Err(Error::Invalid("can't convert on a tweened frame: insert a keyframe here (F6)".into()));
        }
        items.push(loc);
    }
    let parent = items[0].symbol;
    if items.iter().any(|l| l.symbol != parent) {
        return Err(Error::Invalid("elements must belong to the same symbol".into()));
    }
    // Render order: layer order bottom→top, then element order.
    let sym = p.require_symbol(parent)?;
    let order: Vec<LayerId> = sym.content_layers().iter().map(|(l, _, _)| l.id).collect();
    items.sort_by_key(|l| (order.iter().position(|id| *id == l.layer), l.keyframe, l.index));
    let bounds = Rect::union_all(items.iter().filter_map(|l| {
        let e = &p.layer(l.layer)?.keyframes[l.keyframe].elements[l.index];
        element_bounds(p, e, frame)
    }))
    .ok_or_else(|| Error::Invalid("selection has no bounds".into()))?;
    let c = bounds.center();
    let top = *items.last().unwrap();

    let symbol_id = SymbolId(doc.project.alloc_id());
    let layer_id = LayerId(doc.project.alloc_id());
    let mut moved = Vec::new();
    for l in &items {
        let mut e = doc.project.layer(l.layer).unwrap().keyframes[l.keyframe].elements[l.index].clone();
        e.transform.x -= c.x;
        e.transform.y -= c.y;
        moved.push(e);
    }
    // Buttons get all four states (Up, Over, Down, Hit) showing the same art.
    let duration = if kind == SymbolKind::Button { 4 } else { 1 };
    let mut layer = Layer::new(layer_id, "Layer 1", LayerKind::Normal);
    layer.keyframes = vec![Keyframe::blank(duration)];
    let symbol = Symbol { id: symbol_id, name: name.trim().to_string(), kind, layers: vec![layer], script: None };

    let mut instance = Element::new(ElementId(doc.project.alloc_id()), ElementKind::instance(symbol_id));
    instance.transform = Transform::at(c.x, c.y);
    let instance_id = instance.id;
    let mut edits = vec![Edit::InsertSymbol { index: doc.project.symbols.len(), symbol }];
    // Remove back-to-front from the end so indices stay valid, then insert.
    for l in items.iter().rev() {
        edits.push(Edit::RemoveElement {
            element: doc.project.layer(l.layer).unwrap().keyframes[l.keyframe].elements[l.index].id,
        });
    }
    for (i, e) in moved.into_iter().enumerate() {
        edits.push(Edit::InsertElement { layer: layer_id, keyframe: 0, index: i, element: e });
    }
    let removed_before_top =
        items.iter().filter(|l| l.layer == top.layer && l.keyframe == top.keyframe && l.index < top.index).count();
    edits.push(Edit::InsertElement {
        layer: top.layer,
        keyframe: top.keyframe,
        index: top.index - removed_before_top,
        element: instance,
    });
    doc.execute("Convert to Symbol", edits)?;
    Ok((instance_id, symbol_id))
}

pub fn set_symbol_props(doc: &mut Document, symbol: SymbolId, name: Option<&str>, kind: Option<SymbolKind>) -> Result<()> {
    let s = doc.project.require_symbol(symbol)?;
    let (name, kind) = (name.map_or_else(|| s.name.clone(), |n| n.trim().to_string()), kind.unwrap_or(s.kind));
    if name == s.name && kind == s.kind {
        return Ok(());
    }
    doc.execute("Symbol Properties", vec![Edit::SetSymbolProps { symbol, name, kind }])
}

/// Copies a symbol (fresh ids throughout; nested instances still point at
/// the same symbols). Returns the copy.
pub fn duplicate_symbol(doc: &mut Document, symbol: SymbolId) -> Result<SymbolId> {
    let src = doc.project.require_symbol(symbol)?.clone();
    if symbol == doc.project.root {
        return Err(Error::Invalid("the main timeline can't be duplicated".into()));
    }
    let id = SymbolId(doc.project.alloc_id());
    /// Fresh layer/element ids; tracks are remapped per layer so tweens
    /// still pair the copies up.
    fn refresh(p: &mut Project, layers: &mut [Layer]) {
        for l in layers {
            l.id = LayerId(p.alloc_id());
            let mut tracks = std::collections::HashMap::new();
            for k in &mut l.keyframes {
                for e in &mut k.elements {
                    let old = e.track();
                    e.id = ElementId(p.alloc_id());
                    let t = *tracks.entry(old).or_insert(e.id.0);
                    e.track = (t != e.id.0).then_some(t);
                }
            }
            refresh(p, &mut l.children);
        }
    }
    let mut layers = src.layers.clone();
    refresh(&mut doc.project, &mut layers);
    let copy = Symbol { id, name: format!("{} copy", src.name), kind: src.kind, layers, script: src.script.clone() };
    doc.execute("Duplicate Symbol", vec![Edit::InsertSymbol { index: doc.project.symbols.len(), symbol: copy }])?;
    Ok(id)
}

pub fn delete_symbol(doc: &mut Document, symbol: SymbolId) -> Result<()> {
    doc.project.require_symbol(symbol)?;
    doc.execute("Delete Symbol", vec![Edit::RemoveSymbol { symbol }])
}

/// Points instances at another symbol, keeping their transforms.
pub fn swap_symbol(doc: &mut Document, ids: &[ElementId], symbol: SymbolId) -> Result<()> {
    doc.project.require_symbol(symbol)?;
    let mut edits = Vec::new();
    for id in ids {
        let mut e = doc.project.require_element(*id)?.clone();
        let ElementKind::Instance { symbol: s, .. } = &mut e.kind else {
            return Err(Error::Invalid("only symbol instances can be swapped".into()));
        };
        *s = symbol;
        edits.push(Edit::ReplaceElement { element: e });
    }
    doc.execute("Swap Symbol", edits)
}

/// Places a new instance of `symbol` at `at` (symbol-space of the layer's
/// symbol) on top of `layer` at `frame`.
pub fn place_instance(doc: &mut Document, layer: LayerId, symbol: SymbolId, at: Point, frame: u32) -> Result<ElementId> {
    check_target_layer(&doc.project, layer)?;
    doc.project.require_symbol(symbol)?;
    let mut e = Element::new(ElementId(doc.project.alloc_id()), ElementKind::instance(symbol));
    e.transform = Transform::at(at.x, at.y);
    let id = e.id;
    let edits = place_on_top(&doc.project, layer, frame, vec![e])?;
    doc.execute("Place Instance", edits)?;
    Ok(id)
}

// ---------------------------------------------------------------- text & audio

/// The project's copy of the bundled default font, and the edit that adds
/// it if the project doesn't have it yet (so text always embeds its font).
fn default_font(doc: &mut Document) -> (AssetId, Option<Edit>) {
    let existing = doc.project.assets.iter().find(|a| {
        matches!(&a.kind, AssetKind::Font { family, data } if family == crate::text::DEFAULT_FONT_NAME && data.0.len() == crate::text::DEFAULT_FONT.len())
    });
    if let Some(a) = existing {
        return (a.id, None);
    }
    let id = AssetId(doc.project.alloc_id());
    let asset = Asset {
        id,
        name: format!("{}.ttf", crate::text::DEFAULT_FONT_NAME),
        kind: AssetKind::Font { family: crate::text::DEFAULT_FONT_NAME.into(), data: Bytes(crate::text::DEFAULT_FONT.into()) },
    };
    (id, Some(Edit::InsertAsset { index: doc.project.assets.len(), asset }))
}

/// Style for new text (from the text tool).
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct TextStyle {
    /// A font asset; `None` uses (and embeds) the bundled default font.
    pub font: Option<AssetId>,
    pub size: f64,
    pub color: Color,
    pub align: crate::text::TextAlign,
    pub letter_spacing: f64,
    pub line_height: f64,
}

impl Default for TextStyle {
    fn default() -> Self {
        TextStyle {
            font: None,
            size: 32.0,
            color: Color::BLACK,
            align: Default::default(),
            letter_spacing: 0.0,
            line_height: 1.25,
        }
    }
}

/// Creates a text element whose box's top-left is at `at` (symbol space).
pub fn create_text(
    doc: &mut Document,
    layer: LayerId,
    at: Point,
    text: &str,
    style: &TextStyle,
    width: Option<f64>,
    frame: u32,
) -> Result<ElementId> {
    check_target_layer(&doc.project, layer)?;
    let mut edits = Vec::new();
    let font = match style.font {
        Some(f) => {
            if doc.project.font_data(f).is_none() {
                return Err(Error::NotFound(format!("font asset {}", f.0)));
            }
            f
        }
        None => {
            let (id, add) = default_font(doc);
            edits.extend(add);
            id
        }
    };
    let block = crate::text::TextBlock {
        text: text.to_string(),
        font,
        size: style.size,
        fill: Paint::solid(style.color),
        align: style.align,
        letter_spacing: style.letter_spacing,
        line_height: style.line_height,
        width,
    };
    let mut el = Element::new(ElementId(doc.project.alloc_id()), ElementKind::Text(block));
    el.transform = Transform::at(at.x, at.y);
    let id = el.id;
    // The font asset (if new) must exist before the element is placed.
    let mut scratch = doc.project.clone();
    for e in &edits {
        e.clone().apply(&mut scratch)?;
    }
    edits.extend(place_on_top(&scratch, layer, frame, vec![el])?);
    doc.execute("Text", edits)?;
    Ok(id)
}

/// Embeds a TrueType/OpenType font. Returns the asset.
pub fn import_font(doc: &mut Document, name: &str, data: &[u8]) -> Result<AssetId> {
    let family = crate::text::font_family(data).ok_or_else(|| Error::Invalid("not a TrueType/OpenType font".into()))?;
    let id = AssetId(doc.project.alloc_id());
    let asset = Asset { id, name: name.to_string(), kind: AssetKind::Font { family, data: Bytes(data.into()) } };
    doc.execute("Import Font", vec![Edit::InsertAsset { index: doc.project.assets.len(), asset }])?;
    Ok(id)
}

/// Embeds an audio clip. `duration` (seconds) comes from the platform's
/// decoder, which also proved the file playable.
pub fn import_audio(doc: &mut Document, name: &str, data: &[u8], duration: f64) -> Result<AssetId> {
    let mime = crate::asset::sniff_audio(data)
        .ok_or_else(|| Error::Invalid("unsupported audio file (use MP3, WAV, M4A/AAC, Ogg or FLAC)".into()))?;
    if !(duration.is_finite() && duration > 0.0) {
        return Err(Error::Invalid("audio duration must be positive".into()));
    }
    let id = AssetId(doc.project.alloc_id());
    let asset =
        Asset { id, name: name.to_string(), kind: AssetKind::Audio { mime: mime.into(), duration, data: Bytes(data.into()) } };
    doc.execute("Import Audio", vec![Edit::InsertAsset { index: doc.project.assets.len(), asset }])?;
    Ok(id)
}

/// Blank or whitespace-only text means "none".
fn non_blank(s: Option<&str>) -> Option<String> {
    s.filter(|s| !s.trim().is_empty()).map(str::to_string)
}

/// Sets (or with `None` / blank text, removes) the frame script on the
/// keyframe spanning `frame` of each layer.
pub fn set_frame_script(doc: &mut Document, layers: &[LayerId], frame: u32, script: Option<&str>) -> Result<()> {
    let script = non_blank(script);
    edit_timeline(doc, layers, "Frame Script", |_, l| {
        let Some((i, _)) = l.keyframe_at(frame) else {
            return Err(Error::Invalid("no frame there: insert one first (F5)".into()));
        };
        let mut kfs = l.keyframes.clone();
        kfs[i].script = script.clone();
        Ok(Some(kfs))
    })
}

/// Sets (or with `None` / blank text, removes) the label of the keyframe spanning `frame`.
pub fn set_frame_label(doc: &mut Document, layers: &[LayerId], frame: u32, label: Option<&str>) -> Result<()> {
    let label = non_blank(label.map(str::trim));
    edit_timeline(doc, layers, "Frame Label", |_, l| {
        let Some((i, _)) = l.keyframe_at(frame) else {
            return Err(Error::Invalid("no frame there: insert one first (F5)".into()));
        };
        let mut kfs = l.keyframes.clone();
        kfs[i].label = label.clone();
        Ok(Some(kfs))
    })
}

/// Changes the export settings (one undo step).
pub fn set_publish(doc: &mut Document, publish: Publish) -> Result<()> {
    let publish = (publish != Publish::default()).then_some(publish);
    if doc.project.publish == publish {
        return Ok(());
    }
    doc.execute("Export Settings", vec![Edit::SetPublish { publish }])
}

/// Sets (or with `None` / blank text, removes) a symbol's script.
pub fn set_symbol_script(doc: &mut Document, symbol: SymbolId, script: Option<&str>) -> Result<()> {
    let script = non_blank(script);
    if doc.project.require_symbol(symbol)?.script == script {
        return Ok(());
    }
    doc.execute("Symbol Script", vec![Edit::SetSymbolScript { symbol, script }])
}

/// Attaches (or with `None`, removes) a sound on the keyframe spanning `frame`.
pub fn set_sound(doc: &mut Document, layers: &[LayerId], frame: u32, sound: Option<SoundRef>) -> Result<()> {
    if let Some(s) = &sound {
        doc.project.check_sound(s)?;
    }
    let label = if sound.is_some() { "Sound" } else { "Remove Sound" };
    edit_timeline(doc, layers, label, |_, l| {
        let Some((i, _)) = l.keyframe_at(frame) else {
            return Err(Error::Invalid("no frame there: insert one first (F5)".into()));
        };
        let mut kfs = l.keyframes.clone();
        kfs[i].sound = sound.clone();
        Ok(Some(kfs))
    })
}
