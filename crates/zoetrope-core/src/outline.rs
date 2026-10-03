//! Presentation-ready views for editor panels.

use crate::model::*;
use crate::query::{content_bounds, element_bounds};
use serde::Serialize;

/// One row of the layers panel. Children are listed top-first (front-most
/// first), as in Flash's timeline, i.e. reversed from storage order.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LayerNode {
    pub id: u32,
    pub name: String,
    pub kind: LayerKind,
    pub visible: bool,
    pub locked: bool,
    pub element_count: usize,
    pub children: Vec<LayerNode>,
}

pub fn layer_tree(project: &Project, symbol: SymbolId) -> Vec<LayerNode> {
    fn go(layers: &[Layer]) -> Vec<LayerNode> {
        layers
            .iter()
            .rev()
            .map(|l| LayerNode {
                id: l.id.0,
                name: l.name.clone(),
                kind: l.kind,
                visible: l.visible,
                locked: l.locked,
                element_count: l.elements.len(),
                children: go(&l.children),
            })
            .collect()
    }
    project.symbol(symbol).map_or_else(Vec::new, |s| go(&s.layers))
}

/// Everything the properties panel shows for one element.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ElementInfo<'a> {
    pub element: &'a Element,
    pub layer: u32,
    /// Content box size before scale (W/H fields are this × scale).
    pub content_width: f64,
    pub content_height: f64,
    /// Axis-aligned bounds in the parent symbol's space.
    pub bounds: Option<crate::geom::Rect>,
    /// For bitmaps / instances: the referenced asset or symbol name.
    pub source_name: Option<String>,
}

pub fn element_info(project: &Project, id: ElementId) -> Option<ElementInfo<'_>> {
    let e = project.element(id)?;
    let loc = project.locate(id)?;
    let content = content_bounds(project, &e.kind, &crate::math::Matrix::IDENTITY, 0);
    let source_name = match e.kind {
        ElementKind::Bitmap { asset } => project.asset(asset).map(|a| a.name.clone()),
        ElementKind::Instance { symbol } => project.symbol(symbol).map(|s| s.name.clone()),
        ElementKind::Shape(_) => None,
    };
    Some(ElementInfo {
        element: e,
        layer: loc.layer.0,
        content_width: content.map_or(0.0, |c| c.width()),
        content_height: content.map_or(0.0, |c| c.height()),
        bounds: element_bounds(project, e),
        source_name,
    })
}
