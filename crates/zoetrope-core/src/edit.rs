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
    /// Inserts into keyframe `keyframe` (index) of `layer`.
    InsertElement { layer: LayerId, keyframe: usize, index: usize, element: Element },
    RemoveElement { element: ElementId },
    /// Sets the element order of one keyframe; `order` must be a
    /// permutation of its current element ids.
    ReorderElements { layer: LayerId, keyframe: usize, order: Vec<ElementId> },
    /// Replaces a content layer's whole keyframe list (timeline structure
    /// and tween edits). Elements may be kept (same id), dropped, or new.
    SetKeyframes { layer: LayerId, keyframes: Vec<Keyframe> },
    /// Inserts a layer (with any contents) into `parent` (a folder) or the
    /// symbol's top level.
    InsertLayer { symbol: SymbolId, parent: Option<LayerId>, index: usize, layer: Layer },
    RemoveLayer { layer: LayerId },
    SetLayerProps { layer: LayerId, props: LayerProps },
    InsertAsset { index: usize, asset: Asset },
    RemoveAsset { asset: AssetId },
    /// Adds a symbol definition (with all its contents) to the library.
    InsertSymbol { index: usize, symbol: Symbol },
    /// Removes an unused, non-root symbol.
    RemoveSymbol { symbol: SymbolId },
    SetSymbolProps { symbol: SymbolId, name: String, kind: SymbolKind },
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
            Edit::InsertElement { layer, keyframe, index, element } => {
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
                let kf = l.keyframes.get_mut(keyframe).ok_or_else(|| Error::NotFound(format!("keyframe {keyframe}")))?;
                if index > kf.elements.len() {
                    return Err(Error::Invalid(format!("insert index {index} out of range")));
                }
                let id = element.id;
                kf.elements.insert(index, element);
                Ok(Edit::RemoveElement { element: id })
            }
            Edit::RemoveElement { element } => {
                let loc = p.locate(element).ok_or_else(|| not_found(element))?;
                let removed = p.layer_mut(loc.layer).expect("located").keyframes[loc.keyframe].elements.remove(loc.index);
                Ok(Edit::InsertElement { layer: loc.layer, keyframe: loc.keyframe, index: loc.index, element: removed })
            }
            Edit::ReorderElements { layer, keyframe, order } => {
                let l = p.layer_mut(layer).ok_or_else(|| layer_not_found(layer))?;
                let kf = l.keyframes.get_mut(keyframe).ok_or_else(|| Error::NotFound(format!("keyframe {keyframe}")))?;
                let mut current: Vec<ElementId> = kf.elements.iter().map(|e| e.id).collect();
                let mut wanted = order.clone();
                current.sort();
                wanted.sort();
                if current != wanted {
                    return Err(Error::Invalid("reorder must be a permutation of the layer's elements".into()));
                }
                let old: Vec<ElementId> = kf.elements.iter().map(|e| e.id).collect();
                let mut taken = std::mem::take(&mut kf.elements);
                for id in &order {
                    let i = taken.iter().position(|e| e.id == *id).expect("permutation");
                    kf.elements.push(taken.swap_remove(i));
                }
                Ok(Edit::ReorderElements { layer, keyframe, order: old })
            }
            Edit::SetKeyframes { layer, keyframes } => {
                let symbol = p.locate_layer(layer).ok_or_else(|| layer_not_found(layer))?.symbol;
                let old = p.require_layer(layer)?;
                if old.is_folder() {
                    return Err(Error::Invalid("folders have no keyframes".into()));
                }
                let probe = Layer { keyframes: keyframes.clone(), ..Layer::new(layer, "", old.kind) };
                check_keyframes(&probe)?;
                for k in &keyframes {
                    if let Some(s) = &k.sound {
                        p.check_sound(s)?;
                    }
                }
                let mut seen = std::collections::HashSet::new();
                let kept: std::collections::HashSet<ElementId> = old.all_elements().map(|e| e.id).collect();
                for e in probe.all_elements() {
                    if !seen.insert(e.id) {
                        return Err(Error::Invalid(format!("element {} appears twice", e.id.0)));
                    }
                    if !kept.contains(&e.id) && (e.id.0 >= p.next_id || p.locate(e.id).is_some()) {
                        return Err(Error::Invalid(format!("element id {} is not fresh", e.id.0)));
                    }
                    p.check_element_placement(symbol, e)?;
                }
                let l = p.layer_mut(layer).expect("located");
                Ok(Edit::SetKeyframes { layer, keyframes: std::mem::replace(&mut l.keyframes, keyframes) })
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
                        used |= l.all_elements().any(|e| match &e.kind {
                            ElementKind::Bitmap { asset: a } => *a == asset,
                            ElementKind::Text(t) => t.font == asset,
                            _ => false,
                        });
                        used |= l.keyframes.iter().any(|k| k.sound.as_ref().is_some_and(|s| s.asset == asset));
                    });
                }
                if used {
                    return Err(Error::Invalid("asset is still used by elements".into()));
                }
                Ok(Edit::InsertAsset { index: i, asset: p.assets.remove(i) })
            }
            Edit::InsertSymbol { index, symbol } => {
                if symbol.id.0 >= p.next_id || p.symbol(symbol.id).is_some() {
                    return Err(Error::Invalid(format!("symbol id {} is not fresh", symbol.id.0)));
                }
                if index > p.symbols.len() {
                    return Err(Error::Invalid(format!("symbol index {index} out of range")));
                }
                if symbol.name.trim().is_empty() {
                    return Err(Error::Invalid("symbol name cannot be empty".into()));
                }
                // Contents are checked against the project with the symbol in
                // place (elements may reference the symbol's own layers' ids).
                let id = symbol.id;
                p.symbols.insert(index, Symbol { layers: Vec::new(), ..symbol.clone() });
                let mut result = Ok(());
                for l in &symbol.layers {
                    if result.is_ok() {
                        result = check_new_layer(p, id, l);
                    }
                }
                if let Err(e) = result {
                    p.symbols.remove(index);
                    return Err(e);
                }
                p.symbols[index].layers = symbol.layers;
                Ok(Edit::RemoveSymbol { symbol: id })
            }
            Edit::RemoveSymbol { symbol } => {
                if symbol == p.root {
                    return Err(Error::Invalid("the main timeline can't be removed".into()));
                }
                let i = p.symbols.iter().position(|s| s.id == symbol).ok_or_else(|| Error::NotFound(format!("symbol {}", symbol.0)))?;
                if p.symbols.iter().any(|s| s.id != symbol && s.instanced_symbols().contains(&symbol)) {
                    return Err(Error::Invalid("symbol is still used by instances".into()));
                }
                Ok(Edit::InsertSymbol { index: i, symbol: p.symbols.remove(i) })
            }
            Edit::SetSymbolProps { symbol, name, kind } => {
                if name.trim().is_empty() {
                    return Err(Error::Invalid("symbol name cannot be empty".into()));
                }
                let s = p.symbol_mut(symbol).ok_or_else(|| Error::NotFound(format!("symbol {}", symbol.0)))?;
                let old = Edit::SetSymbolProps { symbol, name: std::mem::replace(&mut s.name, name), kind: std::mem::replace(&mut s.kind, kind) };
                Ok(old)
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
        } else if l.is_folder() && !l.keyframes.is_empty() {
            result = Err(Error::Invalid("folders cannot hold keyframes".into()));
        } else if !l.is_folder() && !l.children.is_empty() {
            result = Err(Error::Invalid("only folders can hold layers".into()));
        } else if let Err(e) = check_keyframes(l) {
            result = Err(e);
        }
        for e in l.all_elements() {
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
