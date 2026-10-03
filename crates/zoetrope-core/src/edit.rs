//! Primitive reversible edits.
//!
//! Every mutation of a `Project` goes through `Edit::apply`, which validates,
//! applies, and returns the exact inverse edit. Higher-level commands (see
//! `ops`) are sequences of these, grouped into an undoable transaction by
//! `history::Document`. Edits are plain data, so they can later be logged,
//! replayed or sent over a wire without change.

use crate::asset::Asset;
use crate::error::{Error, Result};
use crate::model::*;

#[derive(Debug, Clone, PartialEq)]
pub enum Edit {
    SetTransform { element: ElementId, transform: Transform },
    /// Replaces an element's whole value (same id, same symbol/layer slot).
    ReplaceElement { element: Element },
    SetStage(Stage),
    InsertElement { layer: LayerId, index: usize, element: Element },
    RemoveElement { element: ElementId },
    /// Sets the element order of a layer; `order` must be a permutation of
    /// the layer's current element ids.
    ReorderElements { layer: LayerId, order: Vec<ElementId> },
    /// Inserts a layer (with any contents) into `parent` (a folder) or the
    /// symbol's top level.
    InsertLayer { symbol: SymbolId, parent: Option<LayerId>, index: usize, layer: Layer },
    RemoveLayer { layer: LayerId },
    SetLayerProps { layer: LayerId, props: LayerProps },
    InsertAsset { index: usize, asset: Asset },
    RemoveAsset { asset: AssetId },
}

/// The editable, non-structural fields of a layer.
#[derive(Debug, Clone, PartialEq)]
pub struct LayerProps {
    pub name: String,
    pub visible: bool,
    pub locked: bool,
    pub kind: LayerKind,
}

impl LayerProps {
    pub fn of(l: &Layer) -> LayerProps {
        LayerProps { name: l.name.clone(), visible: l.visible, locked: l.locked, kind: l.kind }
    }
}

impl Edit {
    /// Applies the edit and returns its inverse. On error the project is
    /// left unchanged.
    pub fn apply(self, p: &mut Project) -> Result<Edit> {
        match self {
            Edit::SetTransform { element, transform } => {
                if !transform.is_finite() {
                    return Err(Error::Invalid("transform has non-finite values".into()));
                }
                let e = p.element_mut(element).ok_or_else(|| not_found(element))?;
                let old = std::mem::replace(&mut e.transform, transform);
                Ok(Edit::SetTransform { element, transform: old })
            }
            Edit::ReplaceElement { element } => {
                let loc = p.locate(element.id).ok_or_else(|| not_found(element.id))?;
                p.check_element_placement(loc.symbol, &element)?;
                let slot = p.element_mut(element.id).expect("located");
                let old = std::mem::replace(slot, element);
                Ok(Edit::ReplaceElement { element: old })
            }
            Edit::SetStage(stage) => {
                stage.validate()?;
                Ok(Edit::SetStage(std::mem::replace(&mut p.stage, stage)))
            }
            Edit::InsertElement { layer, index, element } => {
                if p.locate(element.id).is_some() {
                    return Err(Error::Invalid(format!("element {} already exists", element.id.0)));
                }
                if element.id.0 >= p.next_id {
                    return Err(Error::Invalid(format!("element id {} was not allocated", element.id.0)));
                }
                let symbol = p.locate_layer(layer).ok_or_else(|| layer_not_found(layer))?.symbol;
                p.check_element_placement(symbol, &element)?;
                let l = p.layer_mut(layer).expect("located");
                if l.is_folder() {
                    return Err(Error::Invalid("cannot place elements in a folder".into()));
                }
                if index > l.elements.len() {
                    return Err(Error::Invalid(format!("insert index {index} out of range")));
                }
                let id = element.id;
                l.elements.insert(index, element);
                Ok(Edit::RemoveElement { element: id })
            }
            Edit::RemoveElement { element } => {
                let loc = p.locate(element).ok_or_else(|| not_found(element))?;
                let removed = p.layer_mut(loc.layer).expect("located").elements.remove(loc.index);
                Ok(Edit::InsertElement { layer: loc.layer, index: loc.index, element: removed })
            }
            Edit::ReorderElements { layer, order } => {
                let l = p.layer_mut(layer).ok_or_else(|| layer_not_found(layer))?;
                let mut current: Vec<ElementId> = l.elements.iter().map(|e| e.id).collect();
                let mut wanted = order.clone();
                current.sort();
                wanted.sort();
                if current != wanted {
                    return Err(Error::Invalid("reorder must be a permutation of the layer's elements".into()));
                }
                let old: Vec<ElementId> = l.elements.iter().map(|e| e.id).collect();
                let mut taken = std::mem::take(&mut l.elements);
                for id in &order {
                    let i = taken.iter().position(|e| e.id == *id).expect("permutation");
                    l.elements.push(taken.swap_remove(i));
                }
                Ok(Edit::ReorderElements { layer, order: old })
            }
            Edit::InsertLayer { symbol, parent, index, layer } => {
                check_new_layer(p, symbol, &layer)?;
                let list = p.layer_list_mut(symbol, parent)?;
                if index > list.len() {
                    return Err(Error::Invalid(format!("layer index {index} out of range")));
                }
                let id = layer.id;
                list.insert(index, layer);
                Ok(Edit::RemoveLayer { layer: id })
            }
            Edit::RemoveLayer { layer } => {
                let loc = p.locate_layer(layer).ok_or_else(|| layer_not_found(layer))?;
                let removed = p.layer_list_mut(loc.symbol, loc.parent)?.remove(loc.index);
                Ok(Edit::InsertLayer { symbol: loc.symbol, parent: loc.parent, index: loc.index, layer: removed })
            }
            Edit::SetLayerProps { layer, props } => {
                let l = p.layer_mut(layer).ok_or_else(|| layer_not_found(layer))?;
                if (props.kind == LayerKind::Folder) != l.is_folder() {
                    return Err(Error::Invalid("cannot convert between folders and layers".into()));
                }
                let old = LayerProps::of(l);
                l.name = props.name;
                l.visible = props.visible;
                l.locked = props.locked;
                l.kind = props.kind;
                Ok(Edit::SetLayerProps { layer, props: old })
            }
            Edit::InsertAsset { index, asset } => {
                if asset.id.0 >= p.next_id || p.asset(asset.id).is_some() {
                    return Err(Error::Invalid(format!("asset id {} is not fresh", asset.id.0)));
                }
                if index > p.assets.len() {
                    return Err(Error::Invalid(format!("asset index {index} out of range")));
                }
                let id = asset.id;
                p.assets.insert(index, asset);
                Ok(Edit::RemoveAsset { asset: id })
            }
            Edit::RemoveAsset { asset } => {
                let i = p.assets.iter().position(|a| a.id == asset).ok_or_else(|| Error::NotFound(format!("asset {}", asset.0)))?;
                let mut used = false;
                for s in &p.symbols {
                    walk_layers(&s.layers, &mut |l| {
                        used |= l.elements.iter().any(|e| e.kind == ElementKind::Bitmap { asset });
                    });
                }
                if used {
                    return Err(Error::Invalid("asset is still used by elements".into()));
                }
                Ok(Edit::InsertAsset { index: i, asset: p.assets.remove(i) })
            }
        }
    }
}

/// Every id inside a layer subtree must be fresh, and its contents valid.
fn check_new_layer(p: &Project, symbol: SymbolId, layer: &Layer) -> Result<()> {
    let mut result = Ok(());
    walk_layers(std::slice::from_ref(layer), &mut |l| {
        if result.is_err() {
            return;
        }
        if l.id.0 >= p.next_id || p.layer(l.id).is_some() {
            result = Err(Error::Invalid(format!("layer id {} is not fresh", l.id.0)));
        } else if l.is_folder() && !l.elements.is_empty() {
            result = Err(Error::Invalid("folders cannot hold elements".into()));
        } else if !l.is_folder() && !l.children.is_empty() {
            result = Err(Error::Invalid("only folders can hold layers".into()));
        }
        for e in &l.elements {
            if result.is_ok() && (e.id.0 >= p.next_id || p.locate(e.id).is_some()) {
                result = Err(Error::Invalid(format!("element id {} is not fresh", e.id.0)));
            }
            if result.is_ok() {
                result = p.check_element_placement(symbol, e);
            }
        }
    });
    result
}

fn not_found(id: ElementId) -> Error {
    Error::NotFound(format!("element {}", id.0))
}

fn layer_not_found(id: LayerId) -> Error {
    Error::NotFound(format!("layer {}", id.0))
}
