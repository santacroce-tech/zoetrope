//! Read-only geometric queries over the scene: bounds, hit-testing, marquee
//! selection and selection geometry for the editor's handles.
//!
//! A *scope* is the symbol whose elements are currently selectable (the root
//! timeline for now; the symbol being edited in place from Phase 5), with the
//! matrix mapping that symbol's space to stage space.

use crate::asset::AssetKind;
use crate::geom::Rect;
use crate::math::{Matrix, Point};
use crate::model::*;
use crate::timeline::{evaluate_layer_shown, instance_frame};
use serde::Serialize;
use std::borrow::Cow;

#[derive(Debug, Clone, Copy)]
pub struct Scope {
    pub symbol: SymbolId,
    /// Symbol space → stage space.
    pub matrix: Matrix,
    /// The frame of `symbol`'s timeline being viewed/edited.
    pub frame: u32,
}

impl Scope {
    pub fn root(p: &Project) -> Scope {
        Scope { symbol: p.root, matrix: Matrix::IDENTITY, frame: 0 }
    }

    pub fn at(self, frame: u32) -> Scope {
        Scope { frame, ..self }
    }
}

/// Uniform scale estimate of a matrix (geometric mean of axis scales).
pub fn scale_factor(m: &Matrix) -> f64 {
    m.determinant().abs().sqrt()
}

/// One element as shown at a frame (interpolated when tweened), with its
/// layer's effective lock state.
pub struct SceneElement<'a> {
    pub element: Cow<'a, Element>,
    pub layer: LayerId,
    pub locked: bool,
    /// For instances: the frame their symbol shows (stateless timing, see
    /// `timeline::instance_frame`); 0 otherwise.
    pub child_frame: u32,
}

/// Elements of `symbol` visible at `frame`, in render order (back first).
pub fn scene_elements<'a>(p: &'a Project, symbol: SymbolId, frame: u32) -> Vec<SceneElement<'a>> {
    let Some(sym) = p.symbol(symbol) else {
        return Vec::new();
    };
    sym.content_layers()
        .into_iter()
        .filter(|(_, vis, _)| *vis)
        .flat_map(|(l, _, locked)| {
            evaluate_layer_shown(l, frame).into_iter().map(move |shown| {
                let child_frame = match shown.element.kind {
                    ElementKind::Instance { symbol, .. } => {
                        p.symbol(symbol).map_or(0, |child| instance_frame(child, &shown.element.kind, &shown, frame))
                    }
                    _ => 0,
                };
                SceneElement { element: shown.element, layer: l.id, locked, child_frame }
            })
        })
        .collect()
}

/// Bounds of `kind`'s content drawn through `m`. `None` if it draws nothing
/// (e.g. an empty symbol). `child_frame` is the frame an instance's symbol
/// shows (ignored for other kinds).
pub fn content_bounds(p: &Project, kind: &ElementKind, m: &Matrix, child_frame: u32, depth: usize) -> Option<Rect> {
    match kind {
        ElementKind::Shape(s) => {
            let b = s.geometry.to_path().bounds(m)?;
            let half = s.stroke.as_ref().map_or(0.0, |st| st.width / 2.0 * scale_factor(m));
            Some(b.inflate(half))
        }
        ElementKind::Bitmap { asset } => {
            let AssetKind::Image { width, height, .. } = p.asset(*asset)?.kind else {
                return None;
            };
            let (w, h) = (width as f64, height as f64);
            Some(Rect::new(-w / 2.0, -h / 2.0, w / 2.0, h / 2.0).transformed(m))
        }
        ElementKind::Text(t) => {
            let l = p.layout_text(t)?;
            Some(Rect::new(0.0, 0.0, l.width.max(1.0), l.height).transformed(m))
        }
        ElementKind::Instance { symbol, .. } => {
            if depth > MAX_NESTING_DEPTH {
                return None;
            }
            Rect::union_all(scene_elements(p, *symbol, child_frame).into_iter().filter_map(|se| {
                content_bounds(p, &se.element.kind, &(*m * se.element.transform.matrix()), se.child_frame, depth + 1)
            }))
        }
    }
}

/// An element's bounds in its parent symbol's space, with nested content
/// shown as at the parent's `parent_frame`.
pub fn element_bounds(p: &Project, e: &Element, parent_frame: u32) -> Option<Rect> {
    let child_frame = child_frame_of(p, e.id, parent_frame);
    content_bounds(p, &e.kind, &e.transform.matrix(), child_frame, 0)
}

/// The frame an instance's symbol shows when its parent timeline is at
/// `parent_frame` (0 if the element isn't an instance or isn't shown there).
pub fn child_frame_of(p: &Project, id: ElementId, parent_frame: u32) -> u32 {
    let Some(loc) = p.locate(id) else { return 0 };
    scene_elements(p, loc.symbol, parent_frame).into_iter().find(|se| se.element.id == id).map_or(0, |se| se.child_frame)
}

/// An element as displayed in the scope's current frame, or `None` if it
/// isn't shown there (other keyframe, hidden layer, deleted).
pub fn displayed<'a>(p: &'a Project, scope: &Scope, id: ElementId) -> Option<SceneElement<'a>> {
    let loc = p.locate(id)?;
    if loc.symbol != scope.symbol {
        return None;
    }
    scene_elements(p, scope.symbol, scope.frame).into_iter().find(|se| se.element.id == id)
}

/// Topmost selectable element of the scope under stage point `pt`.
/// `tolerance` is in stage units (callers convert from screen pixels).
pub fn hit_test(p: &Project, scope: &Scope, pt: Point, tolerance: f64) -> Option<ElementId> {
    scene_elements(p, scope.symbol, scope.frame)
        .iter()
        .rev()
        .filter(|se| !se.locked)
        .find(|se| hits(p, &se.element.kind, &(scope.matrix * se.element.transform.matrix()), pt, tolerance, se.child_frame, 0))
        .map(|se| se.element.id)
}

#[allow(clippy::too_many_arguments)]
pub fn hits(p: &Project, kind: &ElementKind, m: &Matrix, pt: Point, tol: f64, child_frame: u32, depth: usize) -> bool {
    match kind {
        ElementKind::Shape(s) => {
            let Some(inv) = m.invert() else { return false };
            let q = inv.apply(pt);
            let path = s.geometry.to_path();
            let fillable = s.geometry.is_fillable();
            if fillable && (s.fill.is_some() || s.stroke.is_none()) && path.contains_with(q, s.fill_rule) {
                return true;
            }
            let half = s.stroke.as_ref().map_or(0.0, |st| st.width / 2.0);
            path.distance_to_outline(q) <= half + tol / scale_factor(m)
        }
        ElementKind::Bitmap { asset } => {
            let (Some(a), Some(inv)) = (p.asset(*asset), m.invert()) else {
                return false;
            };
            let AssetKind::Image { width, height, .. } = a.kind else {
                return false;
            };
            let q = inv.apply(pt);
            q.x.abs() <= width as f64 / 2.0 && q.y.abs() <= height as f64 / 2.0
        }
        ElementKind::Text(t) => {
            // The whole text box is clickable (not just the ink).
            let (Some(l), Some(inv)) = (p.layout_text(t), m.invert()) else {
                return false;
            };
            let q = inv.apply(pt);
            let pad = tol / scale_factor(m);
            q.x >= -pad && q.x <= l.width.max(1.0) + pad && q.y >= -pad && q.y <= l.height + pad
        }
        ElementKind::Instance { symbol, .. } => {
            if depth > MAX_NESTING_DEPTH {
                return false;
            }
            scene_elements(p, *symbol, child_frame)
                .iter()
                .rev()
                .any(|se| hits(p, &se.element.kind, &(*m * se.element.transform.matrix()), pt, tol, se.child_frame, depth + 1))
        }
    }
}

/// The front-most *shape* drawn under `pt`, looking inside instances, and
/// whether the point is on its stroke or its fill (eyedropper). Locked
/// layers are included; hidden ones are not.
pub fn pick_shape(p: &Project, scope: &Scope, pt: Point, tolerance: f64) -> Option<(Shape, crate::interact::PaintPart)> {
    pick_in(p, scope.symbol, scope.frame, &scope.matrix, pt, tolerance, 0)
}

fn pick_in(
    p: &Project,
    symbol: SymbolId,
    frame: u32,
    m: &Matrix,
    pt: Point,
    tol: f64,
    depth: usize,
) -> Option<(Shape, crate::interact::PaintPart)> {
    use crate::interact::PaintPart;
    if depth > MAX_NESTING_DEPTH {
        return None;
    }
    for se in scene_elements(p, symbol, frame).iter().rev() {
        let e = &se.element;
        let em = *m * e.transform.matrix();
        match &e.kind {
            ElementKind::Shape(s) => {
                let Some(inv) = em.invert() else { continue };
                let q = inv.apply(pt);
                let path = s.geometry.to_path();
                if let Some(st) = &s.stroke {
                    if path.distance_to_outline(q) <= st.width / 2.0 + tol / scale_factor(&em) {
                        return Some((s.clone(), PaintPart::Stroke));
                    }
                }
                if s.fill.is_some() && s.geometry.is_fillable() && path.contains_with(q, s.fill_rule) {
                    return Some((s.clone(), PaintPart::Fill));
                }
            }
            ElementKind::Instance { symbol: child, .. } => {
                if let Some(hit) = pick_in(p, *child, se.child_frame, &em, pt, tol, depth + 1) {
                    return Some(hit);
                }
            }
            ElementKind::Bitmap { .. } | ElementKind::Text(_) => {}
        }
    }
    None
}

/// Selectable elements of the scope whose stage bounds intersect `rect`,
/// in render order.
pub fn marquee(p: &Project, scope: &Scope, rect: Rect) -> Vec<ElementId> {
    scene_elements(p, scope.symbol, scope.frame)
        .into_iter()
        .filter(|se| !se.locked)
        .filter(|se| {
            content_bounds(p, &se.element.kind, &(scope.matrix * se.element.transform.matrix()), se.child_frame, 0)
                .is_some_and(|b| b.intersects(&rect))
        })
        .map(|se| se.element.id)
        .collect()
}

/// Elements shown in the scope's frame on visible, unlocked layers, in
/// render order.
pub fn selectable<'a>(p: &'a Project, scope: &Scope) -> Vec<Cow<'a, Element>> {
    scene_elements(p, scope.symbol, scope.frame).into_iter().filter(|se| !se.locked).map(|se| se.element).collect()
}

/// What the editor needs to draw a selection and its handles.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SelectionGeometry {
    /// Handle box corners in stage space: TL, TR, BR, BL (box space order).
    pub corners: [Point; 4],
    /// Rotation/scale origin in stage space.
    pub pivot: Point,
    /// Axis-aligned stage bounds of the whole selection.
    pub bounds: Rect,
    /// Per-element axis-aligned stage bounds (for outlining each item).
    pub items: Vec<Rect>,
    /// Whether the pivot is the element's own (draggable) pivot.
    pub has_own_pivot: bool,
}

/// The handle box's frame (box space → stage) and extent in box space.
/// A single element uses its own content box (so handles rotate with it);
/// multiple elements use their combined axis-aligned stage bounds.
pub fn selection_frame(p: &Project, scope: &Scope, ids: &[ElementId]) -> Option<(Matrix, Rect)> {
    match ids {
        [] => None,
        [id] => {
            let se = displayed(p, scope, *id)?;
            let local = content_bounds(p, &se.element.kind, &Matrix::IDENTITY, se.child_frame, 0)?;
            Some((scope.matrix * se.element.transform.matrix(), local))
        }
        _ => {
            let b = Rect::union_all(ids.iter().filter_map(|id| {
                let se = displayed(p, scope, *id)?;
                content_bounds(p, &se.element.kind, &(scope.matrix * se.element.transform.matrix()), se.child_frame, 0)
            }))?;
            Some((Matrix::IDENTITY, b))
        }
    }
}

pub fn selection_geometry(p: &Project, scope: &Scope, ids: &[ElementId]) -> Option<SelectionGeometry> {
    let (frame, bx) = selection_frame(p, scope, ids)?;
    let items: Vec<Rect> = ids
        .iter()
        .filter_map(|id| {
            let se = displayed(p, scope, *id)?;
            content_bounds(p, &se.element.kind, &(scope.matrix * se.element.transform.matrix()), se.child_frame, 0)
        })
        .collect();
    let bounds = Rect::union_all(items.iter().copied())?;
    let (pivot, has_own_pivot) = match ids {
        [id] => {
            let t = displayed(p, scope, *id)?.element.transform;
            (scope.matrix.apply(Point::new(t.x, t.y)), true)
        }
        _ => (bx.center(), false),
    };
    Some(SelectionGeometry { corners: bx.corners(&frame), pivot, bounds, items, has_own_pivot })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::demo::demo_project;

    fn named(p: &Project, name: &str) -> ElementId {
        let mut found = None;
        for s in &p.symbols {
            walk_layers(&s.layers, &mut |l| {
                if let Some(e) = l.all_elements().find(|e| e.name == name) {
                    found = Some(e.id);
                }
            });
        }
        found.unwrap()
    }

    #[test]
    fn hit_test_finds_topmost_and_recurses_into_instances() {
        let p = demo_project();
        let scope = Scope::root(&p);
        // Center of flower B is on the Flowers layer, above the sky.
        assert_eq!(hit_test(&p, &scope, Point::new(480.0, 210.0), 0.0), Some(named(&p, "flower B")));
        // Empty sky area hits the sky rect.
        assert_eq!(hit_test(&p, &scope, Point::new(100.0, 40.0), 0.0), Some(named(&p, "sky")));
        // Off stage: nothing.
        assert_eq!(hit_test(&p, &scope, Point::new(-50.0, -50.0), 0.0), None);
    }

    #[test]
    fn locked_and_hidden_layers_are_not_selectable() {
        let mut p = demo_project();
        let scope = Scope::root(&p);
        let flowers = p.locate(named(&p, "flower B")).unwrap().layer;
        p.layer_mut(flowers).unwrap().locked = true;
        assert_eq!(hit_test(&p, &scope, Point::new(480.0, 210.0), 0.0), Some(named(&p, "sky")));
        p.layer_mut(flowers).unwrap().locked = false;
        p.layer_mut(flowers).unwrap().visible = false;
        assert_eq!(hit_test(&p, &scope, Point::new(480.0, 210.0), 0.0), Some(named(&p, "sky")));
    }

    #[test]
    fn marquee_selects_intersecting() {
        let p = demo_project();
        let ids = marquee(&p, &Scope::root(&p), Rect::new(780.0, 60.0, 800.0, 80.0));
        assert!(ids.contains(&named(&p, "sun")));
        assert!(ids.contains(&named(&p, "sky")));
        assert!(!ids.contains(&named(&p, "ground")));
    }

    #[test]
    fn single_selection_box_follows_rotation() {
        let p = demo_project();
        let sun = named(&p, "sun");
        let g = selection_geometry(&p, &Scope::root(&p), &[sun]).unwrap();
        assert!((g.pivot.x - 820.0).abs() < 1e-9);
        assert!((g.corners[0].x - 765.0).abs() < 1e-6 && (g.corners[0].y - 35.0).abs() < 1e-6);
    }
}
