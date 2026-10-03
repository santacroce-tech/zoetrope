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
use crate::vector::freehand;
use crate::math::Point;
use crate::model::*;
use crate::query::element_bounds;
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
const PATCHABLE: &[&str] = &["name", "transform", "opacity", "blend", "tint", "geometry", "fill", "stroke", "fillRule"];

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

fn check_target_layer(p: &Project, layer: LayerId) -> Result<()> {
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
    let Some(drag) = shape_from_drag(tool, p0, p1, mods, opts) else { return Ok(None) };
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
fn place_on_top(p: &Project, layer: LayerId, frame: u32, elements: Vec<Element>) -> Result<Vec<Edit>> {
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
pub fn create_path(doc: &mut Document, layer: LayerId, mut path: VectorPath, style: &ShapeStyle, label: &str, frame: u32) -> Result<ElementId> {
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
    let Some(path) = freehand(points, tolerance, smooth, tolerance * 4.0) else { return Ok(None) };
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
    Some(PickedStyle {
        part,
        fill: shape.fill.as_ref().map(Paint::style),
        stroke: shape.stroke.as_ref().map(StrokeStyle::of),
    })
}

/// Sets the fill or stroke paint of shapes from a geometry-free style
/// (gradients are fitted to each shape's bounds). `None` removes it. Setting
/// a stroke paint on a shape without a stroke adds a 1px stroke.
pub fn set_paint_style(doc: &mut Document, ids: &[ElementId], part: PaintPart, style: Option<&PaintStyle>) -> Result<()> {
    let mut edits = Vec::new();
    for id in ids {
        let mut e = doc.project.require_element(*id)?.clone();
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
            let (start, end) =
                if to_stage { (0.0, stage_extent) } else { (lo(&items[0].1), hi(&items[n - 1].1)) };
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
    /// Only `normal` ⇄ `guide`.
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
        _ => "Layer Type",
    };
    doc.execute(label, vec![Edit::SetLayerProps { layer: id, props }])
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
            let Some((i, _)) = probe.keyframe_at(frame) else { break };
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
                kfs.insert(i + 1, Keyframe { duration: end - frame, elements, tween });
            }
            None => {
                let len = l.length();
                let elements = copies(p, len.saturating_sub(1));
                kfs.last_mut().expect("content layers have keyframes").duration += frame - len;
                kfs.push(Keyframe { duration: 1, elements, tween: None });
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
        let Some((i, start)) = l.keyframe_at(frame) else { return Ok(None) };
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
        let Some((i, _)) = l.keyframe_at(frame) else { return Ok(None) };
        if tween.is_some() && i + 1 >= l.keyframes.len() {
            return Err(Error::Invalid("insert a keyframe later on this layer to tween toward (F6)".into()));
        }
        let mut kfs = l.keyframes.clone();
        kfs[i].tween = tween.clone();
        Ok(Some(kfs))
    })
}
