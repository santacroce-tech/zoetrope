//! Copy and paste.
//!
//! A copy is a self-contained snippet: the copied elements plus every symbol
//! and asset they need (transitively), so it can be pasted into the same
//! project or into another one. Pasting reuses symbols and assets the target
//! project already has with the same *content*: assets byte for byte, and
//! symbols by structure (ids don't matter; nested symbols and assets compare
//! by content too), so repeated pastes don't pile up copies. Anything else
//! is added with fresh ids, and references are rewritten. A paste is one
//! undo step. See docs/FORMAT.md, "Clipboard".

use crate::asset::Asset;
use crate::edit::Edit;
use crate::error::{Error, Result};
use crate::history::Document;
use crate::model::*;
use crate::ops;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};

pub const CLIPBOARD_FORMAT: &str = "zoetrope-clipboard";

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Clipboard {
    pub format: String,
    /// The project schema the snippet was written with.
    pub schema_version: u32,
    /// Back to front, in the coordinates of the timeline they came from.
    pub elements: Vec<Element>,
    /// Every symbol the elements use, directly or through other symbols.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub symbols: Vec<Symbol>,
    /// Every image, font and sound the elements and symbols use.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub assets: Vec<Asset>,
}

/// Symbols and assets an element refers to directly.
fn element_refs(e: &Element, symbols: &mut Vec<SymbolId>, assets: &mut Vec<AssetId>) {
    match &e.kind {
        ElementKind::Instance { symbol, .. } => symbols.push(*symbol),
        ElementKind::Bitmap { asset } => assets.push(*asset),
        ElementKind::Text(t) => assets.push(t.font),
        ElementKind::Shape(_) => {}
    }
}

/// Symbols and assets a symbol refers to directly (elements and sounds).
fn symbol_refs(s: &Symbol) -> (Vec<SymbolId>, Vec<AssetId>) {
    let (mut symbols, mut assets) = (Vec::new(), Vec::new());
    walk_layers(&s.layers, &mut |l| {
        for k in &l.keyframes {
            for e in &k.elements {
                element_refs(e, &mut symbols, &mut assets);
            }
            if let Some(sound) = &k.sound {
                assets.push(sound.asset);
            }
        }
    });
    (symbols, assets)
}

/// Copies elements (shown at `frame` or not; their stored state is copied).
/// They are ordered back to front, whatever order `ids` lists them in.
pub fn copy(p: &Project, ids: &[ElementId]) -> Result<Clipboard> {
    if ids.is_empty() {
        return Err(Error::Invalid("select something to copy".into()));
    }
    // Stacking order: layer render order, then keyframe position.
    let order: HashMap<LayerId, usize> = p
        .symbols
        .iter()
        .flat_map(|s| s.content_layers().into_iter().enumerate().map(|(i, (l, _, _))| (l.id, i)).collect::<Vec<_>>())
        .collect();
    let mut located: Vec<((usize, usize), Element)> = ids
        .iter()
        .map(|id| {
            let loc = p.locate(*id).ok_or_else(|| Error::NotFound(format!("element {}", id.0)))?;
            Ok(((order.get(&loc.layer).copied().unwrap_or(0), loc.index), p.element(*id).unwrap().clone()))
        })
        .collect::<Result<_>>()?;
    located.sort_by_key(|(k, _)| *k);
    let elements: Vec<Element> = located.into_iter().map(|(_, e)| e).collect();

    // Everything they depend on, transitively.
    let (mut pending, mut asset_ids) = (Vec::new(), Vec::new());
    for e in &elements {
        element_refs(e, &mut pending, &mut asset_ids);
    }
    let mut seen = HashSet::new();
    let mut symbols = Vec::new();
    while let Some(id) = pending.pop() {
        if !seen.insert(id) {
            continue;
        }
        let s = p.require_symbol(id)?;
        let (more_symbols, more_assets) = symbol_refs(s);
        pending.extend(more_symbols);
        asset_ids.extend(more_assets);
        symbols.push(s.clone());
    }
    // Library order, so a paste keeps the familiar order.
    symbols.sort_by_key(|s| p.symbols.iter().position(|t| t.id == s.id));
    let wanted: HashSet<AssetId> = asset_ids.into_iter().collect();
    let assets = p.assets.iter().filter(|a| wanted.contains(&a.id)).cloned().collect();
    Ok(Clipboard { format: CLIPBOARD_FORMAT.into(), schema_version: crate::format::SCHEMA_VERSION, elements, symbols, assets })
}

/// Reads clipboard text; `None` if it isn't a Zoetrope snippet at all.
pub fn parse(text: &str) -> Option<Result<Clipboard>> {
    let v: serde_json::Value = serde_json::from_str(text.trim()).ok()?;
    if v.get("format").and_then(|f| f.as_str()) != Some(CLIPBOARD_FORMAT) {
        return None;
    }
    let version = v.get("schemaVersion").and_then(|n| n.as_u64()).unwrap_or(0);
    if version != crate::format::SCHEMA_VERSION as u64 {
        return Some(Err(Error::Invalid(format!(
            "copied from a different Zoetrope version (format {version}, this one reads {}); save it there and open the file here instead",
            crate::format::SCHEMA_VERSION
        ))));
    }
    Some(serde_json::from_value(v).map_err(|e| Error::Invalid(format!("damaged clipboard data: {e}"))))
}

/// Rewrites symbol and asset references.
fn remap(e: &mut Element, symbols: &HashMap<SymbolId, SymbolId>, assets: &HashMap<AssetId, AssetId>) {
    match &mut e.kind {
        ElementKind::Instance { symbol, .. } => *symbol = symbols.get(symbol).copied().unwrap_or(*symbol),
        ElementKind::Bitmap { asset } => *asset = assets.get(asset).copied().unwrap_or(*asset),
        ElementKind::Text(t) => t.font = assets.get(&t.font).copied().unwrap_or(t.font),
        ElementKind::Shape(_) => {}
    }
}

/// Fresh layer and element ids for a pasted symbol's contents (tracks are
/// remapped per layer so tweens still pair up), with references rewritten.
fn refresh(p: &mut Project, layers: &mut [Layer], symbols: &HashMap<SymbolId, SymbolId>, assets: &HashMap<AssetId, AssetId>) {
    for l in layers {
        l.id = LayerId(p.alloc_id());
        let mut tracks = HashMap::new();
        for k in &mut l.keyframes {
            for e in &mut k.elements {
                let old = e.track();
                e.id = ElementId(p.alloc_id());
                let t = *tracks.entry(old).or_insert(e.id.0);
                e.track = (t != e.id.0).then_some(t);
                remap(e, symbols, assets);
            }
            if let Some(sound) = &mut k.sound {
                sound.asset = assets.get(&sound.asset).copied().unwrap_or(sound.asset);
            }
        }
        refresh(p, &mut l.children, symbols, assets);
    }
}

/// A library name not used yet: `name`, else `name 2`, `name 3`…
fn unique_name(taken: &HashSet<String>, name: &str) -> String {
    if !taken.contains(name) {
        return name.to_string();
    }
    (2..).map(|n| format!("{name} {n}")).find(|n| !taken.contains(n)).unwrap()
}

/// Content fingerprints of symbols in one context (the clipboard or a
/// project): the symbol with every id normalized to its order of
/// appearance, nested symbols replaced by their fingerprints and assets by
/// a hash of their content. Equal fingerprints mean equal pictures and
/// behaviour.
struct Fingerprints<'a> {
    symbols: HashMap<SymbolId, &'a Symbol>,
    assets: HashMap<AssetId, u64>,
    memo: HashMap<SymbolId, String>,
}

fn asset_hash(a: &Asset) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    serde_json::to_string(&a.kind).unwrap_or_default().hash(&mut h);
    h.finish()
}

impl<'a> Fingerprints<'a> {
    fn new(symbols: impl Iterator<Item = &'a Symbol>, assets: &'a [Asset]) -> Self {
        Fingerprints {
            symbols: symbols.map(|s| (s.id, s)).collect(),
            assets: assets.iter().map(|a| (a.id, asset_hash(a))).collect(),
            memo: HashMap::new(),
        }
    }

    fn of(&mut self, id: SymbolId, depth: usize) -> String {
        if let Some(f) = self.memo.get(&id) {
            return f.clone();
        }
        let Some(sym) = self.symbols.get(&id).copied() else { return format!("missing:{}", id.0) };
        if depth > MAX_NESTING_DEPTH {
            return "deep".into();
        }
        let mut next = 0u32;
        let mut fresh = || {
            next += 1;
            next
        };
        let mut layers = sym.layers.clone();
        let mut nested = Vec::new();
        fn norm(layers: &mut [Layer], fresh: &mut dyn FnMut() -> u32, nested: &mut Vec<(usize, SymbolId)>, slot: &mut usize) {
            for l in layers {
                l.id = LayerId(fresh());
                let mut tracks = HashMap::new();
                for k in &mut l.keyframes {
                    for e in &mut k.elements {
                        let t = e.track();
                        e.id = ElementId(fresh());
                        let nt = *tracks.entry(t).or_insert_with(&mut *fresh);
                        e.track = Some(nt);
                        if let ElementKind::Instance { symbol, .. } = &mut e.kind {
                            nested.push((*slot, *symbol));
                            *symbol = SymbolId(u32::MAX - *slot as u32);
                            *slot += 1;
                        }
                    }
                }
                norm(&mut l.children, fresh, nested, slot);
            }
        }
        let mut slot = 0;
        norm(&mut layers, &mut fresh, &mut nested, &mut slot);
        let normalized = Symbol { id: SymbolId(0), name: String::new(), layers, ..sym.clone() };
        let mut v = serde_json::to_value(&normalized).unwrap_or_default();
        // Asset references → content hashes.
        let assets = self.assets.clone();
        fn assets_by_content(v: &mut serde_json::Value, assets: &HashMap<AssetId, u64>) {
            match v {
                serde_json::Value::Object(m) => {
                    for key in ["asset", "font"] {
                        if let Some(id) = m.get(key).and_then(|x| x.as_u64()) {
                            m.insert(
                                key.into(),
                                assets.get(&AssetId(id as u32)).map_or(serde_json::Value::Null, |h| (*h).into()),
                            );
                        }
                    }
                    m.values_mut().for_each(|x| assets_by_content(x, assets));
                }
                serde_json::Value::Array(a) => a.iter_mut().for_each(|x| assets_by_content(x, assets)),
                _ => {}
            }
        }
        assets_by_content(&mut v, &assets);
        let inner: Vec<String> = nested.iter().map(|(_, child)| self.of(*child, depth + 1)).collect();
        let f = format!("{v}|{}", inner.join("|"));
        self.memo.insert(id, f.clone());
        f
    }
}

/// Pastes a snippet on top of `layer` at `frame`, moved by `(dx, dy)`.
/// Returns the new elements' ids (back to front).
pub fn paste(doc: &mut Document, layer: LayerId, frame: u32, clip: &Clipboard, dx: f64, dy: f64) -> Result<Vec<ElementId>> {
    if clip.elements.is_empty() {
        return Err(Error::Invalid("nothing to paste".into()));
    }
    ops::check_target_layer(&doc.project, layer)?;
    let mut edits = Vec::new();

    // Assets: reuse ones with identical content, add the rest under fresh ids.
    let mut asset_map: HashMap<AssetId, AssetId> = HashMap::new();
    let mut n_assets = doc.project.assets.len();
    for a in &clip.assets {
        let same =
            doc.project.asset(a.id).filter(|b| b.kind == a.kind).or_else(|| doc.project.assets.iter().find(|b| b.kind == a.kind));
        if let Some(b) = same {
            asset_map.insert(a.id, b.id);
            continue;
        }
        let id = AssetId(doc.project.alloc_id());
        asset_map.insert(a.id, id);
        edits.push(Edit::InsertAsset { index: n_assets, asset: Asset { id, ..a.clone() } });
        n_assets += 1;
    }

    // Symbols: reuse one with the same content (preferring the same id).
    let by_id: HashMap<SymbolId, &Symbol> = clip.symbols.iter().map(|s| (s.id, s)).collect();
    let mut theirs = Fingerprints::new(clip.symbols.iter(), &clip.assets);
    let mut ours = Fingerprints::new(doc.project.symbols.iter().filter(|s| s.id != doc.project.root), &doc.project.assets);
    let mut existing: HashMap<String, SymbolId> = HashMap::new();
    for s in doc.project.symbols.iter().rev().filter(|s| s.id != doc.project.root) {
        existing.insert(ours.of(s.id, 0), s.id);
    }
    let mut reusable: HashMap<SymbolId, SymbolId> = HashMap::new();
    for s in &clip.symbols {
        let f = theirs.of(s.id, 0);
        let same_id = doc.project.symbol(s.id).is_some() && s.id != doc.project.root && ours.of(s.id, 0) == f;
        if same_id {
            reusable.insert(s.id, s.id);
        } else if let Some(id) = existing.get(&f) {
            reusable.insert(s.id, *id);
        }
    }
    let mut symbol_map: HashMap<SymbolId, SymbolId> = HashMap::new();
    for s in &clip.symbols {
        let id = reusable.get(&s.id).copied().unwrap_or_else(|| SymbolId(doc.project.alloc_id()));
        symbol_map.insert(s.id, id);
    }
    // Insert new symbols dependencies first (a symbol's contents are
    // validated against the project when it's inserted).
    let mut taken: HashSet<String> = doc.project.symbols.iter().map(|s| s.name.clone()).collect();
    let mut inserted: HashSet<SymbolId> = HashSet::new();
    let mut remaining: Vec<&Symbol> = clip.symbols.iter().filter(|s| !reusable.contains_key(&s.id)).collect();
    let mut n_symbols = doc.project.symbols.len();
    while !remaining.is_empty() {
        let ready = remaining.iter().position(|s| {
            symbol_refs(s).0.iter().all(|d| reusable.contains_key(d) || inserted.contains(d) || !by_id.contains_key(d))
        });
        let Some(i) = ready else { return Err(Error::Invalid("symbols in the clipboard contain each other".into())) };
        let s = remaining.remove(i);
        let mut layers = s.layers.clone();
        refresh(&mut doc.project, &mut layers, &symbol_map, &asset_map);
        let name = unique_name(&taken, &s.name);
        taken.insert(name.clone());
        let symbol = Symbol { id: symbol_map[&s.id], name, kind: s.kind, layers, script: s.script.clone() };
        edits.push(Edit::InsertSymbol { index: n_symbols, symbol });
        n_symbols += 1;
        inserted.insert(s.id);
    }

    // The elements themselves, on top of the target layer.
    let mut new_ids = Vec::new();
    let elements: Vec<Element> = clip
        .elements
        .iter()
        .map(|e| {
            let mut c = e.clone();
            c.id = ElementId(doc.project.alloc_id());
            c.track = None;
            c.transform.x += dx;
            c.transform.y += dy;
            remap(&mut c, &symbol_map, &asset_map);
            new_ids.push(c.id);
            c
        })
        .collect();
    edits.extend(ops::place_on_top(&doc.project, layer, frame, elements)?);
    doc.execute("Paste", edits)?;
    Ok(new_ids)
}
