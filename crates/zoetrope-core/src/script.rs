//! The script bridge: the operations scripts can perform on the runtime,
//! as one JSON-shaped call. The sandboxed JavaScript host (editor preview
//! and exported player, `editor/src/runtime/`) wraps these in a Flash-like
//! object API; everything with meaning (what is on stage, what a jump does,
//! what overlaps) is decided here, so scripts behave the same everywhere.
//!
//! Objects are addressed by `InstancePath` (`[[layer, track], …]`, root =
//! `[]`). Frames are 0-based here; the JS API is 1-based like Flash.
//! See docs/FORMAT.md, "Scripting".

use crate::error::{Error, Result};
use crate::math::Point;
use crate::model::*;
use crate::player::{FrameTarget, Override, Player};
use crate::render::InstancePath;
use serde::Deserialize;
use serde_json::{json, Value};

#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "op", rename_all = "camelCase")]
pub enum Call {
    /// `{ frame, length, playing }` of the root or a movie clip, else `null`.
    Timeline {
        path: InstancePath,
    },
    Play {
        path: InstancePath,
    },
    Stop {
        path: InstancePath,
    },
    Goto {
        path: InstancePath,
        frame: FrameTarget,
        play: bool,
    },
    /// The object's properties (see `properties`), or `null` if it's gone.
    Get {
        path: InstancePath,
    },
    Set {
        path: InstancePath,
        props: Override,
    },
    /// Path of the topmost child with that instance name, or `null`.
    Child {
        path: InstancePath,
        name: String,
    },
    /// `[{ name, path }]` of the named children, in render order.
    Children {
        path: InstancePath,
    },
    HitTestPoint {
        path: InstancePath,
        x: f64,
        y: f64,
        #[serde(default)]
        shape: bool,
    },
    HitTestObject {
        path: InstancePath,
        other: InstancePath,
    },
    /// Stage-space bounds `{ x, y, width, height }`, or `null`.
    Bounds {
        path: InstancePath,
    },
}

/// Performs one call.
pub fn call(p: &Project, player: &mut Player, c: Call) -> Result<Value> {
    Ok(match c {
        Call::Timeline { path } => json!(player.timeline(p, &path)),
        Call::Play { path } => {
            player.set_playing(p, &path, true)?;
            Value::Null
        }
        Call::Stop { path } => {
            player.set_playing(p, &path, false)?;
            Value::Null
        }
        Call::Goto { path, frame, play } => {
            player.goto(p, &path, &frame, play)?;
            Value::Null
        }
        Call::Get { path } => properties(p, player, &path),
        Call::Set { path, props } => {
            player.set_properties(p, &path, props)?;
            Value::Null
        }
        Call::Child { path, name } => json!(player.child_named(p, &path, &name)),
        Call::Children { path } => Value::Array(
            player.named_children(p, &path).into_iter().map(|(name, path)| json!({ "name": name, "path": path })).collect(),
        ),
        Call::HitTestPoint { path, x, y, shape } => {
            if !(x.is_finite() && y.is_finite()) {
                return Err(Error::Invalid("hitTestPoint needs finite coordinates".into()));
            }
            json!(player.hit_test_point(p, &path, Point::new(x, y), shape))
        }
        Call::HitTestObject { path, other } => json!(player.hit_test_object(p, &path, &other)),
        Call::Bounds { path } => match player.bounds(p, &path) {
            Some(b) => {
                json!({ "x": b.min.x, "y": b.min.y, "width": b.width(), "height": b.height() })
            }
            None => Value::Null,
        },
    })
}

/// `{ kind, name, x, y, rotation, scaleX, scaleY, alpha, visible, text?,
/// symbol?, timeline? }`; the root is `{ kind: "root", timeline }`.
fn properties(p: &Project, player: &Player, path: &[(u32, u32)]) -> Value {
    if path.is_empty() {
        return json!({ "kind": "root", "name": "root", "timeline": player.timeline(p, path) });
    }
    let Some(r) = player.resolve(p, path) else {
        return Value::Null;
    };
    let e = &r.element;
    let t = &e.transform;
    let mut v = json!({
        "name": e.name,
        "x": t.x,
        "y": t.y,
        "rotation": t.rotation,
        "scaleX": t.scale_x,
        "scaleY": t.scale_y,
        "alpha": e.opacity,
        "visible": r.visible,
    });
    let kind = match &e.kind {
        ElementKind::Shape(_) => "shape",
        ElementKind::Bitmap { .. } => "bitmap",
        ElementKind::Text(text) => {
            v["text"] = json!(text.text);
            "text"
        }
        ElementKind::Instance { symbol, .. } => {
            let sym = p.symbol(*symbol);
            v["symbol"] = json!(sym.map(|s| s.name.as_str()));
            v["timeline"] = json!(player.timeline(p, path));
            match sym.map(|s| s.kind) {
                Some(SymbolKind::MovieClip) => "movieClip",
                Some(SymbolKind::Button) => "button",
                _ => "graphic",
            }
        }
    };
    v["kind"] = json!(kind);
    v
}

/// Human-readable location of a frame script, for error messages:
/// `"Scene 1 › Actions › frame 3"`.
pub fn frame_script_location(p: &Project, symbol: SymbolId, layer: LayerId, frame: u32) -> String {
    let sym = p.symbol(symbol).map_or("?", |s| s.name.as_str());
    let layer = p.layer(layer).map_or("?", |l| l.name.as_str());
    format!("{sym} › {layer} › frame {}", frame + 1)
}

/// What a script on `symbol`'s timeline can refer to by name, for the
/// script editor's autocomplete: instance names (any layer, any frame) and
/// frame labels, each sorted and without duplicates.
pub fn names_in(p: &Project, symbol: SymbolId) -> (Vec<String>, Vec<String>) {
    let mut names = std::collections::BTreeSet::new();
    let mut labels = std::collections::BTreeSet::new();
    if let Some(s) = p.symbol(symbol) {
        walk_layers(&s.layers, &mut |l| {
            for k in &l.keyframes {
                names.extend(k.elements.iter().filter(|e| !e.name.is_empty()).map(|e| e.name.clone()));
                labels.extend(k.label.iter().filter(|l| !l.is_empty()).cloned());
            }
        });
    }
    (names.into_iter().collect(), labels.into_iter().collect())
}
