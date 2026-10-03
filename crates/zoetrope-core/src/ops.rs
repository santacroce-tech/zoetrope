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
use crate::interact::{shape_from_drag, Modifiers, ShapeTool};
use crate::math::Point;
use crate::model::*;
use crate::query::element_bounds;
use serde::Deserialize;
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
        let edit = Edit::InsertElement { layer: loc.layer, index: loc.index + 1, element: copy };
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
const PATCHABLE: &[&str] = &["name", "transform", "opacity", "blend", "tint", "geometry", "fill", "stroke"];

/// Merges a JSON patch into an element (`transform` is merged field by
/// field; other keys replace; `null` clears optional fields). Only
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
            } else if k == "fill" || k == "stroke" || k == "geometry" {
                if !matches!(e.kind, ElementKind::Shape(_)) {
                    return Err(Error::Invalid(format!("{k:?} only applies to shapes")));
                }
                v[k] = pv.clone();
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
pub fn set_element_size(doc: &mut Document, id: ElementId, width: f64, height: f64) -> Result<()> {
    let e = doc.project.require_element(id)?;
    let content = crate::query::content_bounds(&doc.project, &e.kind, &crate::math::Matrix::IDENTITY, 0)
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

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ShapeStyle {
    pub fill: Option<Color>,
    pub stroke: Option<Color>,
    #[serde(default = "default_stroke_width")]
    pub stroke_width: f64,
}

fn default_stroke_width() -> f64 {
    1.0
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
pub fn create_shape(
    doc: &mut Document,
    layer: LayerId,
    tool: ShapeTool,
    p0: Point,
    p1: Point,
    mods: Modifiers,
    style: &ShapeStyle,
) -> Result<Option<ElementId>> {
    check_target_layer(&doc.project, layer)?;
    let Some(drag) = shape_from_drag(tool, p0, p1, mods) else { return Ok(None) };
    let stroke_color = match (tool, style.stroke) {
        (ShapeTool::Line, None) => Some(Color::BLACK),
        (_, s) => s,
    };
    let shape = Shape {
        geometry: drag.geometry,
        fill: if tool == ShapeTool::Line { None } else { style.fill.map(|color| Fill::Solid { color }) },
        stroke: stroke_color.map(|color| Stroke { width: style.stroke_width.max(0.0), color }),
    };
    let mut el = Element::new(ElementId(doc.project.alloc_id()), ElementKind::Shape(shape));
    el.transform = Transform::at(drag.center.x, drag.center.y);
    let id = el.id;
    let index = doc.project.require_layer(layer)?.elements.len();
    let label = match tool {
        ShapeTool::Rect => "Rectangle",
        ShapeTool::Ellipse => "Ellipse",
        ShapeTool::Line => "Line",
    };
    doc.execute(label, vec![Edit::InsertElement { layer, index, element: el }])?;
    Ok(Some(id))
}

/// Embeds an image and places it centered at `at` on top of `layer`.
pub fn import_image(doc: &mut Document, layer: LayerId, name: &str, data: &[u8], at: Point) -> Result<ElementId> {
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
    let index = doc.project.require_layer(layer)?.elements.len();
    doc.execute(
        "Import Image",
        vec![
            Edit::InsertAsset { index: doc.project.assets.len(), asset },
            Edit::InsertElement { layer, index, element: el },
        ],
    )?;
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

fn bounds_of(p: &Project, ids: &[ElementId]) -> Result<Vec<(ElementId, Rect)>> {
    ids.iter()
        .map(|id| {
            let e = p.require_element(*id)?;
            Ok((*id, element_bounds(p, e).ok_or_else(|| Error::Invalid("element has no bounds".into()))?))
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
pub fn align(doc: &mut Document, ids: &[ElementId], mode: Align, to_stage: bool) -> Result<()> {
    let items = bounds_of(&doc.project, ids)?;
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
pub fn distribute(doc: &mut Document, ids: &[ElementId], mode: Distribute, to_stage: bool) -> Result<()> {
    let mut items = bounds_of(&doc.project, ids)?;
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
    let mut layers: Vec<LayerId> = Vec::new();
    for id in ids {
        let loc = doc.project.locate(*id).ok_or_else(|| not_found(*id))?;
        if !layers.contains(&loc.layer) {
            layers.push(loc.layer);
        }
    }
    let mut edits = Vec::new();
    for layer in layers {
        let old: Vec<ElementId> = doc.project.require_layer(layer)?.elements.iter().map(|e| e.id).collect();
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
            edits.push(Edit::ReorderElements { layer, order });
        }
    }
    doc.execute("Arrange", edits)
}

/// Moves elements to the top of another layer in the same symbol.
pub fn move_to_layer(doc: &mut Document, ids: &[ElementId], layer: LayerId) -> Result<()> {
    check_target_layer(&doc.project, layer)?;
    let target_symbol = doc.project.locate_layer(layer).unwrap().symbol;
    let mut edits = Vec::new();
    let mut index = doc.project.require_layer(layer)?.elements.len();
    for id in ids {
        let loc = doc.project.locate(*id).ok_or_else(|| not_found(*id))?;
        if loc.symbol != target_symbol {
            return Err(Error::Invalid("elements can only move between layers of the same symbol".into()));
        }
        if loc.layer == layer {
            continue;
        }
        let element = doc.project.element(*id).unwrap().clone();
        edits.push(Edit::RemoveElement { element: *id });
        edits.push(Edit::InsertElement { layer, index, element });
        index += 1;
    }
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
