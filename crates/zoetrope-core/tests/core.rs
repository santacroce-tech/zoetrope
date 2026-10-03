use serde_json::{json, Value};
use zoetrope_core::demo::demo_project;
use zoetrope_core::edit::Edit;
use zoetrope_core::format::{self, load_from_str, migrate, save_to_string, Migration, SCHEMA_VERSION};
use zoetrope_core::interact::{DragMode, Handle, Modifiers, PaintPart, ShapeOptions, ShapeTool, SnapConfig, TransformSession};
use zoetrope_core::ops::{self, Align, Arrange, Distribute, LayerPatch, ShapeStyle, StrokeStyle};
use zoetrope_core::query::{element_bounds, Scope};
use zoetrope_core::render::{render_frame, DrawOp, RecordingRenderer, RenderOptions};
use zoetrope_core::*;

fn find(p: &Project, name: &str) -> ElementId {
    let mut found = None;
    for s in &p.symbols {
        walk_layers(&s.layers, &mut |l| {
            if let Some(e) = l.elements.iter().find(|e| e.name == name) {
                found = Some(e.id);
            }
        });
    }
    found.unwrap_or_else(|| panic!("no element named {name}"))
}

fn layer_named(p: &Project, name: &str) -> LayerId {
    let mut found = None;
    for s in &p.symbols {
        walk_layers(&s.layers, &mut |l| {
            if l.name == name {
                found = Some(l.id);
            }
        });
    }
    found.unwrap_or_else(|| panic!("no layer named {name}"))
}

fn style(fill: Option<Color>, stroke: Option<(Color, f64)>) -> ShapeStyle {
    ShapeStyle {
        fill: fill.map(|color| PaintStyle::Solid { color }),
        stroke: stroke.map(|(color, width)| StrokeStyle { color, width, ..Default::default() }),
    }
}

const OPTS: ShapeOptions = ShapeOptions { sides: 5, star: None };

fn render_with(p: &Project, show_guides: bool) -> Vec<DrawOp> {
    let mut r = RecordingRenderer::default();
    render_frame(p, 0, RenderOptions { view: Matrix::IDENTITY, clip_to_stage: true, show_guides }, &mut r);
    r.ops
}

fn record(p: &Project) -> Vec<DrawOp> {
    render_with(p, true)
}

fn count(ops: &[DrawOp], pred: impl Fn(&DrawOp) -> bool) -> usize {
    ops.iter().filter(|o| pred(o)).count()
}

/// A tiny valid 2×3 PNG header (enough for sniffing; pixels are never decoded in tests).
fn png(w: u32, h: u32) -> Vec<u8> {
    let mut v = b"\x89PNG\r\n\x1a\n\0\0\0\x0dIHDR".to_vec();
    v.extend_from_slice(&w.to_be_bytes());
    v.extend_from_slice(&h.to_be_bytes());
    v.extend_from_slice(&[8, 6, 0, 0, 0]);
    v
}

// ---------- scene tree & rendering ----------

#[test]
fn demo_is_nested_and_valid() {
    let p = demo_project();
    p.validate().unwrap();
    let ops = record(&p);
    // sky, ground, sun, cloud + 3 flowers × (stem + 6 petals + center)
    assert_eq!(count(&ops, |o| matches!(o, DrawOp::Fill { .. })), 4 + 3 * (1 + 6 + 1));
    // petals+center strokes ×3, plus the guide line and the cloud outline
    assert_eq!(count(&ops, |o| matches!(o, DrawOp::Stroke { .. })), 3 * (6 + 1) + 2);
    assert!(matches!(ops.first(), Some(DrawOp::Begin(_))));
    assert_eq!(ops.last(), Some(&DrawOp::End));
}

#[test]
fn guide_layers_are_editor_only() {
    let p = demo_project();
    let strokes = |ops: &[DrawOp]| count(ops, |o| matches!(o, DrawOp::Stroke { .. }));
    assert_eq!(strokes(&render_with(&p, true)), strokes(&render_with(&p, false)) + 1);
}

#[test]
fn hidden_folders_hide_their_children() {
    let mut doc = Document::new(demo_project());
    let scenery = layer_named(&doc.project, "Scenery");
    let fills_before = count(&record(&doc.project), |o| matches!(o, DrawOp::Fill { .. }));
    ops::set_layer_props(&mut doc, scenery, &LayerPatch { visible: Some(false), ..Default::default() }).unwrap();
    let fills_after = count(&record(&doc.project), |o| matches!(o, DrawOp::Fill { .. }));
    assert_eq!(fills_before - fills_after, 4, "sky, ground, sun and cloud hidden");
}

#[test]
fn nested_transforms_compose_down_the_tree() {
    let p = demo_project();
    let ops = record(&p);
    let flower_b = p.require_element(find(&p, "flower B")).unwrap().transform.matrix();
    let expected = flower_b * Matrix::IDENTITY * Transform::at(0.0, -52.0).matrix();
    assert!(ops.iter().any(|o| matches!(o, DrawOp::Fill { transform, .. } if transform.approx_eq(&expected, 1e-9))));
}

#[test]
fn rendering_is_deterministic() {
    let p = demo_project();
    assert_eq!(record(&p), record(&p));
    assert_eq!(record(&demo_project()), record(&p));
}

#[test]
fn opacity_and_tint_compose_down_the_tree() {
    let mut doc = Document::new(demo_project());
    let b = find(&doc.project, "flower B");
    let center = find(&doc.project, "center");
    ops::patch_element(&mut doc, b, &json!({"opacity": 0.5})).unwrap();
    ops::patch_element(&mut doc, center, &json!({"tint": {"color": "#ff0000", "amount": 1.0}})).unwrap();
    let flower_b = doc.project.element(b).unwrap().transform.matrix();
    let ops = record(&doc.project);
    let center_fill_b = ops.iter().find_map(|o| match o {
        DrawOp::Fill { transform, paint: Paint::Solid { color }, .. } if transform.approx_eq(&flower_b, 1e-9) => Some(*color),
        _ => None,
    });
    // Flower B's center: fully red-tinted, at half alpha from the instance.
    assert_eq!(center_fill_b, Some(Color::rgba(255, 0, 0, 128)));
}

#[test]
fn blend_modes_render_as_groups() {
    let mut doc = Document::new(demo_project());
    let sun = find(&doc.project, "sun");
    ops::patch_element(&mut doc, sun, &json!({"blend": "multiply", "opacity": 0.25})).unwrap();
    let ops = record(&doc.project);
    let i = ops.iter().position(|o| matches!(o, DrawOp::BeginGroup { .. })).unwrap();
    assert_eq!(ops[i], DrawOp::BeginGroup { blend: BlendMode::Multiply, alpha: 0.25 });
    // Inside the group, the fill is drawn at full alpha (the group applies opacity).
    assert!(matches!(&ops[i + 1], DrawOp::Fill { paint, .. } if paint.stops().iter().all(|s| s.color.a == 255)));
    assert_eq!(ops[i + 2], DrawOp::EndGroup);
}

#[test]
fn editing_a_symbol_definition_affects_every_instance() {
    let mut doc = Document::new(demo_project());
    let before = record(&doc.project);
    let center = find(&doc.project, "center");
    ops::translate_element(&mut doc, center, 5.0, 0.0).unwrap();
    let after = record(&doc.project);
    let changed = before.iter().zip(&after).filter(|(a, b)| a != b).count();
    assert_eq!(changed, 3 * 2, "center fill+stroke changes in all three flowers");
}

#[test]
fn cycles_are_rejected() {
    let mut doc = Document::new(demo_project());
    let p = &doc.project;
    let petal = p.symbols.iter().find(|s| s.name == "Petal").unwrap();
    let flower = p.symbols.iter().find(|s| s.name == "Flower").unwrap().id;
    let layer = petal.layers[0].id;
    let id = ElementId(doc.project.alloc_id());
    let el = Element::new(id, ElementKind::Instance { symbol: flower });
    let err = doc.execute("bad", vec![Edit::InsertElement { layer, index: 0, element: el }]);
    assert!(matches!(err, Err(Error::Invalid(_))));
    assert!(!doc.is_dirty());

    let mut v: Value = serde_json::from_str(&save_to_string(&demo_project())).unwrap();
    let petal_json = v["project"]["symbols"].as_array_mut().unwrap().iter_mut().find(|s| s["name"] == "Petal").unwrap();
    petal_json["layers"][0]["elements"]
        .as_array_mut()
        .unwrap()
        .push(json!({"id": 9999, "type": "instance", "symbol": flower.0}));
    v["project"]["nextId"] = json!(10000);
    assert!(matches!(load_from_str(&v.to_string()), Err(Error::Invalid(m)) if m.contains("contains itself")));
}

// ---------- undo / redo ----------

#[test]
fn undo_redo_trivial_edit() {
    let original = demo_project();
    let mut doc = Document::new(original.clone());
    let sun = find(&doc.project, "sun");
    ops::translate_element(&mut doc, sun, 10.0, -4.0).unwrap();
    let moved = doc.project.clone();
    assert_eq!(moved.require_element(sun).unwrap().transform.x, 830.0);
    assert!(doc.is_dirty());
    assert_eq!(doc.undo_label(), Some("Move"));

    assert!(doc.undo().unwrap());
    assert_eq!(doc.project, original);
    assert!(!doc.is_dirty());
    assert!(!doc.undo().unwrap(), "nothing more to undo");

    assert!(doc.redo().unwrap());
    assert_eq!(doc.project, moved);
    assert!(!doc.redo().unwrap());
}

#[test]
fn structural_edits_undo_to_the_same_position() {
    let original = demo_project();
    let mut doc = Document::new(original.clone());
    let b = find(&doc.project, "flower B");
    let loc = doc.project.locate(b).unwrap();
    ops::delete_element(&mut doc, b).unwrap();
    assert!(doc.project.element(b).is_none());
    let a = find(&doc.project, "flower A");
    let copy = ops::duplicate_element(&mut doc, a, 30.0, 0.0).unwrap();
    assert_eq!(doc.project.locate(copy).unwrap().index, doc.project.locate(a).unwrap().index + 1);

    doc.undo().unwrap();
    doc.undo().unwrap();
    assert_eq!(doc.project.locate(b), Some(loc));
    assert_eq!(Project { next_id: original.next_id, ..doc.project.clone() }, original);

    doc.redo().unwrap();
    doc.redo().unwrap();
    assert!(doc.project.element(b).is_none());
    assert!(doc.project.element(copy).is_some());
}

#[test]
fn new_edit_clears_redo() {
    let mut doc = Document::new(demo_project());
    let sun = find(&doc.project, "sun");
    ops::rotate_element(&mut doc, sun, 15.0).unwrap();
    doc.undo().unwrap();
    assert_eq!(doc.redo_label(), Some("Rotate"));
    ops::translate_element(&mut doc, sun, 1.0, 1.0).unwrap();
    assert_eq!(doc.redo_label(), None);
}

#[test]
fn failed_transaction_rolls_back_atomically() {
    let original = demo_project();
    let mut doc = Document::new(original.clone());
    let sun = find(&doc.project, "sun");
    let edits = vec![
        Edit::SetTransform { element: sun, transform: Transform::at(1.0, 2.0) },
        Edit::RemoveElement { element: ElementId(424242) },
    ];
    assert!(matches!(doc.execute("Bad", edits), Err(Error::NotFound(_))));
    assert_eq!(doc.project, original);
    assert_eq!(doc.undo_label(), None);
}

#[test]
fn saving_clears_dirty_and_undo_past_save_is_dirty() {
    let mut doc = Document::new(demo_project());
    let sun = find(&doc.project, "sun");
    ops::translate_element(&mut doc, sun, 1.0, 0.0).unwrap();
    doc.mark_saved();
    assert!(!doc.is_dirty());
    doc.undo().unwrap();
    assert!(doc.is_dirty());
    doc.redo().unwrap();
    assert!(!doc.is_dirty());
}

// ---------- Phase 2 ops ----------

#[test]
fn patch_element_validates_and_undoes() {
    let mut doc = Document::new(demo_project());
    let sun = find(&doc.project, "sun");
    let before = doc.project.clone();
    ops::patch_element(&mut doc, sun, &json!({"transform": {"rotation": 45, "skewX": 10}, "name": "Sol"})).unwrap();
    let e = doc.project.element(sun).unwrap();
    assert_eq!((e.transform.rotation, e.transform.skew_x, e.transform.x, e.name.as_str()), (45.0, 10.0, 820.0, "Sol"));
    assert!(ops::patch_element(&mut doc, sun, &json!({"id": 5})).is_err());
    assert!(ops::patch_element(&mut doc, sun, &json!({"opacity": 2})).is_err());
    let a = find(&doc.project, "flower A");
    assert!(ops::patch_element(&mut doc, a, &json!({"fill": null})).is_err());
    doc.undo().unwrap();
    assert_eq!(doc.project, before);
}

#[test]
fn create_shapes_on_active_layer_respecting_locks() {
    let mut doc = Document::new(demo_project());
    let layer = layer_named(&doc.project, "Flowers");
    let style = style(Some(Color::rgb(1, 2, 3)), Some((Color::BLACK, 4.0)));
    let id = ops::create_shape(&mut doc, layer, ShapeTool::Rect, Point::new(10.0, 10.0), Point::new(110.0, 60.0), Modifiers::default(), &OPTS, &style)
        .unwrap()
        .unwrap();
    let e = doc.project.element(id).unwrap();
    assert_eq!((e.transform.x, e.transform.y), (60.0, 35.0));
    let b = element_bounds(&doc.project, e).unwrap();
    assert_eq!((b.width(), b.height()), (104.0, 54.0), "stroke half-width included");
    assert!(ops::create_shape(&mut doc, layer, ShapeTool::Rect, Point::new(0.0, 0.0), Point::new(0.0, 0.0), Modifiers::default(), &OPTS, &style)
        .unwrap()
        .is_none());

    ops::set_layer_props(&mut doc, layer, &LayerPatch { locked: Some(true), ..Default::default() }).unwrap();
    let err = ops::create_shape(&mut doc, layer, ShapeTool::Line, Point::new(0.0, 0.0), Point::new(50.0, 0.0), Modifiers::default(), &OPTS, &style);
    assert!(matches!(err, Err(Error::Invalid(m)) if m.contains("locked")));
}

#[test]
fn align_and_distribute() {
    let mut doc = Document::new(demo_project());
    let layer = layer_named(&doc.project, "Flowers");
    let style = style(Some(Color::BLACK), None);
    let mk = |doc: &mut Document, x0: f64, y0: f64, w: f64| {
        ops::create_shape(doc, layer, ShapeTool::Rect, Point::new(x0, y0), Point::new(x0 + w, y0 + 20.0), Modifiers::default(), &OPTS, &style)
            .unwrap()
            .unwrap()
    };
    let a = mk(&mut doc, 0.0, 0.0, 10.0);
    let b = mk(&mut doc, 50.0, 30.0, 30.0);
    let c = mk(&mut doc, 200.0, 70.0, 20.0);
    let bounds = |doc: &Document, id| element_bounds(&doc.project, doc.project.element(id).unwrap()).unwrap();

    ops::align(&mut doc, &[a, b, c], Align::Top, false).unwrap();
    assert!([a, b, c].iter().all(|id| bounds(&doc, *id).min.y == 0.0));

    ops::distribute(&mut doc, &[c, a, b], Distribute::SpaceX, false).unwrap();
    let (ba, bb, bc) = (bounds(&doc, a), bounds(&doc, b), bounds(&doc, c));
    assert!((bb.min.x - ba.max.x - (bc.min.x - bb.max.x)).abs() < 1e-9, "equal gaps");
    assert_eq!((ba.min.x, bc.max.x), (0.0, 220.0), "outer items stay put");

    ops::align(&mut doc, &[b], Align::CenterX, false).unwrap();
    assert_eq!(bounds(&doc, b).center().x, 480.0, "single element aligns to stage");
    assert_eq!(doc.undo_label(), Some("Align"));
}

#[test]
fn arrange_z_order() {
    let mut doc = Document::new(demo_project());
    let (a, b, c) = (find(&doc.project, "flower A"), find(&doc.project, "flower B"), find(&doc.project, "flower C"));
    let layer = layer_named(&doc.project, "Flowers");
    let order = |doc: &Document| doc.project.layer(layer).unwrap().elements.iter().map(|e| e.id).collect::<Vec<_>>();
    ops::arrange(&mut doc, &[a], Arrange::Front).unwrap();
    assert_eq!(order(&doc), vec![b, c, a]);
    ops::arrange(&mut doc, &[a], Arrange::Backward).unwrap();
    assert_eq!(order(&doc), vec![b, a, c]);
    ops::arrange(&mut doc, &[c, a], Arrange::Back).unwrap();
    assert_eq!(order(&doc), vec![a, c, b]);
    ops::arrange(&mut doc, &[a], Arrange::Forward).unwrap();
    assert_eq!(order(&doc), vec![c, a, b]);
    for _ in 0..4 {
        doc.undo().unwrap();
    }
    assert_eq!(order(&doc), vec![a, b, c]);
}

#[test]
fn layer_tree_ops() {
    let mut doc = Document::new(demo_project());
    let original = doc.project.clone();
    let root = doc.project.root;
    let flowers = layer_named(&doc.project, "Flowers");
    let scenery = layer_named(&doc.project, "Scenery");

    let new = ops::add_layer(&mut doc, root, Some(flowers), LayerKind::Normal).unwrap();
    let loc = doc.project.locate_layer(new).unwrap();
    assert_eq!(loc.index, doc.project.locate_layer(flowers).unwrap().index + 1, "inserted above");
    let folder = ops::add_layer(&mut doc, root, None, LayerKind::Folder).unwrap();
    ops::move_layer(&mut doc, new, Some(folder), 0).unwrap();
    assert_eq!(doc.project.locate_layer(new).unwrap().parent, Some(folder));
    assert!(ops::move_layer(&mut doc, folder, Some(folder), 0).is_err(), "no folder into itself");
    ops::move_layer(&mut doc, flowers, Some(scenery), 0).unwrap();
    assert!(doc.project.layer_is_within(flowers, scenery));
    ops::set_layer_props(&mut doc, new, &LayerPatch { name: Some("Props".into()), kind: Some(LayerKind::Guide), ..Default::default() }).unwrap();
    assert!(ops::set_layer_props(&mut doc, folder, &LayerPatch { kind: Some(LayerKind::Normal), ..Default::default() }).is_err());
    ops::delete_layer(&mut doc, scenery).unwrap();
    assert!(doc.project.element(find(&original, "sun")).is_none(), "folder deletion takes contents");

    let snapshot = save_to_string(&doc.project);
    assert_eq!(save_to_string(&load_from_str(&snapshot).unwrap()), snapshot);

    while doc.undo().unwrap() {}
    assert_eq!(Project { next_id: original.next_id, ..doc.project.clone() }, original);
}

#[test]
fn cannot_delete_the_last_layer() {
    let mut doc = Document::new(demo_project());
    let petal = doc.project.symbols.iter().find(|s| s.name == "Petal").unwrap().layers[0].id;
    assert!(ops::delete_layer(&mut doc, petal).is_err());
}

#[test]
fn image_import_embeds_asset_and_round_trips() {
    let mut doc = Document::new(demo_project());
    let layer = layer_named(&doc.project, "Flowers");
    let id = ops::import_image(&mut doc, layer, "photo.png", &png(64, 32), Point::new(100.0, 100.0)).unwrap();
    let ops_ = record(&doc.project);
    assert!(ops_.iter().any(|o| matches!(o, DrawOp::Image { width, height, .. } if *width == 64.0 && *height == 32.0)));
    let b = element_bounds(&doc.project, doc.project.element(id).unwrap()).unwrap();
    assert_eq!((b.min.x, b.min.y, b.width()), (68.0, 84.0, 64.0));

    let json = save_to_string(&doc.project);
    assert!(json.contains("\"data\": \"iVBORw0KGgo"), "bytes stored as base64");
    let loaded = load_from_str(&json).unwrap();
    assert_eq!(loaded, doc.project);

    doc.undo().unwrap();
    assert!(doc.project.assets.is_empty() && doc.project.element(id).is_none());
    assert!(ops::import_image(&mut doc, layer, "x.txt", b"not an image at all", Point::default()).is_err());
}

// ---------- the Phase 2 gate, end to end ----------

#[test]
fn gate_static_multilayer_scene_round_trips_and_undoes() {
    let mut doc = Document::new(demo_project());
    let original = doc.project.clone();
    let root = doc.project.root;
    let scope = Scope::root(&doc.project);
    let style = style(Some(Color::rgb(200, 40, 40)), Some((Color::BLACK, 2.0)));

    let top = ops::add_layer(&mut doc, root, None, LayerKind::Normal).unwrap();
    let r = ops::create_shape(&mut doc, top, ShapeTool::Rect, Point::new(100.0, 100.0), Point::new(200.0, 160.0), Modifiers::default(), &OPTS, &style).unwrap().unwrap();
    let e = ops::create_shape(&mut doc, top, ShapeTool::Ellipse, Point::new(300.0, 100.0), Point::new(380.0, 200.0), Modifiers::default(), &OPTS, &style).unwrap().unwrap();
    let l = ops::create_shape(&mut doc, top, ShapeTool::Line, Point::new(50.0, 300.0), Point::new(250.0, 330.0), Modifiers { shift: true, alt: false }, &OPTS, &style).unwrap().unwrap();

    // Drag-scale the rect, rotate the ellipse, skew the line.
    let s = TransformSession::begin(&doc.project, scope, &[r], DragMode::Scale { handle: Handle::Se }, Point::new(200.0, 160.0)).unwrap();
    s.update(&mut doc.project, Point::new(250.0, 190.0), Modifiers::default(), &SnapConfig::default());
    s.commit(&mut doc).unwrap();
    let s = TransformSession::begin(&doc.project, scope, &[e], DragMode::Rotate, Point::new(400.0, 150.0)).unwrap();
    s.update(&mut doc.project, Point::new(340.0, 210.0), Modifiers { shift: true, alt: false }, &SnapConfig::default());
    s.commit(&mut doc).unwrap();
    let s = TransformSession::begin(&doc.project, scope, &[l], DragMode::Skew { handle: Handle::E }, Point::new(250.0, 300.0)).unwrap();
    s.update(&mut doc.project, Point::new(250.0, 320.0), Modifiers::default(), &SnapConfig::default());
    s.commit(&mut doc).unwrap();
    ops::patch_elements(&mut doc, &[r, e], &json!({"opacity": 0.6, "blend": "multiply", "tint": {"color": "#0000ff", "amount": 0.3}})).unwrap();
    ops::patch_element(&mut doc, r, &json!({"transform": {"pivotX": -50, "pivotY": -30}})).unwrap();
    ops::set_stage(&mut doc, Stage { width: 800.0, height: 600.0, background: Color::rgb(10, 10, 10), fps: 30.0 }).unwrap();

    let rect = doc.project.element(r).unwrap();
    // The handle box includes the 2px stroke (102×62), anchored at its NW corner.
    assert!((rect.transform.scale_x - 151.0 / 101.0).abs() < 1e-9 && (rect.transform.scale_y - 91.0 / 61.0).abs() < 1e-9);
    assert_eq!(doc.project.element(e).unwrap().transform.rotation % 15.0, 0.0);

    // Round-trip: identical model, identical render, byte-stable JSON.
    let json = save_to_string(&doc.project);
    let loaded = load_from_str(&json).unwrap();
    assert_eq!(loaded, doc.project);
    assert_eq!(record(&loaded), record(&doc.project));
    assert_eq!(save_to_string(&loaded), json);

    // Everything undoes back to the start, and redoes to the same end.
    let end = doc.project.clone();
    let mut steps = 0;
    while doc.undo().unwrap() {
        steps += 1;
    }
    assert_eq!(steps, 10, "layer, 3 shapes, 3 drags, 2 property patches, stage");
    assert_eq!(Project { next_id: original.next_id, ..doc.project.clone() }, original);
    while doc.redo().unwrap() {}
    assert_eq!(doc.project, end);
}

// ---------- serialization ----------

#[test]
fn project_round_trips_through_json() {
    let mut doc = Document::new(demo_project());
    ops::set_stage_background(&mut doc, Color::rgba(10, 20, 30, 128)).unwrap();
    let c = find(&doc.project, "flower C");
    ops::rotate_element(&mut doc, c, 33.5).unwrap();
    let json = save_to_string(&doc.project);
    let loaded = load_from_str(&json).unwrap();
    assert_eq!(loaded, doc.project);
    assert_eq!(save_to_string(&loaded), json, "serialization is stable");
    assert_eq!(record(&loaded), record(&doc.project));
}

#[test]
fn envelope_is_versioned() {
    let v: Value = serde_json::from_str(&save_to_string(&demo_project())).unwrap();
    assert_eq!(v["format"], "zoetrope-project");
    assert_eq!(v["schemaVersion"], SCHEMA_VERSION);
}

#[test]
fn bad_files_are_rejected() {
    let good: Value = serde_json::from_str(&save_to_string(&demo_project())).unwrap();
    let mut newer = good.clone();
    newer["schemaVersion"] = json!(SCHEMA_VERSION + 1);
    assert!(matches!(load_from_str(&newer.to_string()), Err(Error::UnsupportedVersion { .. })));

    let mut wrong = good.clone();
    wrong["format"] = json!("something-else");
    assert!(matches!(load_from_str(&wrong.to_string()), Err(Error::Format(_))));

    let mut missing_root = good.clone();
    missing_root["project"]["root"] = json!(777);
    assert!(matches!(load_from_str(&missing_root.to_string()), Err(Error::Invalid(_))));

    let mut folder_with_elements = good.clone();
    let scene = &mut folder_with_elements["project"]["symbols"][0];
    let folder = scene["layers"].as_array_mut().unwrap().iter_mut().find(|l| l["kind"] == "folder").unwrap();
    folder["elements"] = json!([{ "id": 9000, "type": "shape", "geometry": { "kind": "rect", "width": 1, "height": 1 } }]);
    folder_with_elements["project"]["nextId"] = json!(9001);
    assert!(matches!(load_from_str(&folder_with_elements.to_string()), Err(Error::Invalid(_))));

    assert!(matches!(load_from_str("not json"), Err(Error::Format(_))));
}

#[test]
fn phase1_files_still_load() {
    // Exactly what Phase 1 wrote: no kind/opacity/pivot/assets fields.
    let v = json!({
        "format": "zoetrope-project", "schemaVersion": 1,
        "project": { "nextId": 4, "root": 1,
            "stage": { "width": 100.0, "height": 50.0, "background": "#ffffff", "fps": 12.0 },
            "symbols": [{ "id": 1, "name": "S", "layers": [{ "id": 2, "name": "L", "visible": true, "locked": false, "elements": [
                { "id": 3, "name": "", "transform": { "x": 1.0, "y": 2.0, "scaleX": 1.0, "scaleY": 1.0, "rotation": 0.0, "skewX": 0.0, "skewY": 0.0 },
                  "type": "shape", "geometry": { "kind": "rect", "width": 10.0, "height": 10.0 },
                  "fill": { "type": "solid", "color": "#ff0000" }, "stroke": { "width": 2.0, "color": "#000000" } }
            ]}]}]
        }
    });
    let p = load_from_str(&v.to_string()).unwrap();
    let e = p.element(ElementId(3)).unwrap();
    assert_eq!((e.transform.x, e.transform.pivot_x, e.opacity, e.blend), (1.0, 0.0, 1.0, BlendMode::Normal));
    assert_eq!(p.symbols[0].layers[0].kind, LayerKind::Normal);
    // v1 → v2 migration: strokes gain a paint and keep v1's butt/miter look.
    let ElementKind::Shape(shape) = &e.kind else { panic!() };
    let st = shape.stroke.as_ref().unwrap();
    assert_eq!(st.paint, Paint::solid(Color::BLACK));
    assert_eq!((st.cap, st.join, st.miter_limit), (LineCap::Butt, LineJoin::Miter, 10.0));
    assert_eq!(shape.fill, Some(Paint::solid(Color::rgb(255, 0, 0))));
    // And it now saves as the current schema.
    assert!(save_to_string(&p).contains("\"schemaVersion\": 2"));
}

#[test]
fn migration_chain_runs_in_order() {
    fn v1_to_v2(mut v: Value) -> Result<Value> {
        v["project"]["log"] = json!(["1->2"]);
        Ok(v)
    }
    fn v2_to_v3(mut v: Value) -> Result<Value> {
        assert_eq!(v["schemaVersion"], 2, "runner bumps version between steps");
        v["project"]["log"].as_array_mut().unwrap().push(json!("2->3"));
        Ok(v)
    }
    let steps: &[Migration] = &[v1_to_v2, v2_to_v3];
    let v1 = json!({"format": format::FORMAT_ID, "schemaVersion": 1, "project": {}});
    let out = migrate(v1, steps, 3).unwrap();
    assert_eq!(out["schemaVersion"], 3);
    assert_eq!(out["project"]["log"], json!(["1->2", "2->3"]));

    let v2 = json!({"format": format::FORMAT_ID, "schemaVersion": 2, "project": {"log": []}});
    assert_eq!(migrate(v2, steps, 3).unwrap()["project"]["log"], json!(["2->3"]));

    let v0 = json!({"format": format::FORMAT_ID, "schemaVersion": 0, "project": {}});
    assert!(matches!(migrate(v0, steps, 3), Err(Error::UnsupportedVersion { .. })));
}

// ---------- Phase 3: vector drawing ----------

#[test]
fn paint_styles_fit_gradients_to_shapes() {
    let mut doc = Document::new(demo_project());
    let ground = find(&doc.project, "ground"); // 960×160 rect
    let stops = vec![GradientStop { offset: 0.0, color: Color::BLACK }, GradientStop { offset: 1.0, color: Color::WHITE }];
    ops::set_paint_style(&mut doc, &[ground], PaintPart::Fill, Some(&PaintStyle::Linear { stops: stops.clone() })).unwrap();
    let ElementKind::Shape(s) = &doc.project.element(ground).unwrap().kind else { panic!() };
    assert_eq!(s.fill, Some(Paint::Linear { start: Point::new(-480.0, 0.0), end: Point::new(480.0, 0.0), stops: stops.clone() }));
    ops::set_paint_style(&mut doc, &[ground], PaintPart::Stroke, Some(&PaintStyle::Radial { stops })).unwrap();
    let ElementKind::Shape(s) = &doc.project.element(ground).unwrap().kind else { panic!() };
    assert!(matches!(s.stroke.as_ref().unwrap().paint, Paint::Radial { radius, .. } if radius == 480.0));
    let horizon = find(&doc.project, "horizon");
    assert!(ops::set_paint_style(&mut doc, &[horizon], PaintPart::Fill, Some(&PaintStyle::Solid { color: Color::BLACK })).is_err());
    doc.undo().unwrap();
    doc.undo().unwrap();
    assert_eq!(Project { next_id: demo_project().next_id, ..doc.project.clone() }, demo_project());
}

#[test]
fn anchor_ops_and_conversion() {
    let mut doc = Document::new(demo_project());
    let sun = find(&doc.project, "sun");
    let before = record(&doc.project);
    ops::convert_to_path(&mut doc, &[sun]).unwrap();
    let ElementKind::Shape(s) = &doc.project.element(sun).unwrap().kind else { panic!() };
    assert!(matches!(s.geometry, Geometry::Path(_)));
    // Converting doesn't change the rendered outline beyond float noise.
    let after = record(&doc.project);
    assert_eq!(before.len(), after.len());

    let r = ops::insert_anchor(&mut doc, sun, 0, 1, 0.5).unwrap();
    assert_eq!(r, NodeRef { subpath: 0, node: 2 });
    ops::convert_anchor(&mut doc, sun, r).unwrap();
    ops::delete_anchors(&mut doc, sun, &[NodeRef { subpath: 0, node: 0 }, NodeRef { subpath: 0, node: 1 }]).unwrap();
    let ElementKind::Shape(s) = &doc.project.element(sun).unwrap().kind else { panic!() };
    let Geometry::Path(v) = &s.geometry else { panic!() };
    assert_eq!(v.subpaths[0].nodes.len(), 3);
    // Deleting every anchor deletes the element.
    let all: Vec<NodeRef> = (0..3).map(|node| NodeRef { subpath: 0, node }).collect();
    ops::delete_anchors(&mut doc, sun, &all).unwrap();
    assert!(doc.project.element(sun).is_none());
    for _ in 0..5 {
        doc.undo().unwrap();
    }
    assert_eq!(record(&doc.project), before);
}

#[test]
fn eyedropper_picks_nested_shapes_and_parts() {
    let p = demo_project();
    let scope = Scope::root(&p);
    // Center of flower B → the "center" shape inside the Flower symbol.
    let picked = ops::pick_style(&p, &scope, Point::new(480.0, 210.0), 1.0).unwrap();
    assert_eq!(picked.part, PaintPart::Fill);
    assert_eq!(picked.fill, Some(PaintStyle::Solid { color: Color::rgb(0xf5, 0xc5, 0x18) }));
    assert_eq!(picked.stroke.as_ref().map(|s| s.width), Some(3.0));
    // On its stroke ring (radius 28) → stroke.
    let edge = ops::pick_style(&p, &scope, Point::new(508.0, 210.0), 1.0).unwrap();
    assert_eq!(edge.part, PaintPart::Stroke);
    // The sun's radial gradient comes back as a style.
    let sun = ops::pick_style(&p, &scope, Point::new(820.0, 90.0), 1.0).unwrap();
    assert!(matches!(sun.fill, Some(PaintStyle::Radial { ref stops }) if stops.len() == 3));
}

#[test]
fn gate_vector_paths_with_gradients_round_trip_and_undo() {
    use zoetrope_core::interact::{EditSession, EditTarget, GradientHandle, PenSession};
    let mut doc = Document::new(demo_project());
    let original = doc.project.clone();
    let scope = Scope::root(&doc.project);
    let layer = layer_named(&doc.project, "Flowers");
    let m = Modifiers::default();
    let grad = ShapeStyle {
        fill: Some(PaintStyle::Radial {
            stops: vec![GradientStop { offset: 0.0, color: Color::WHITE }, GradientStop { offset: 1.0, color: Color::rgb(30, 60, 200) }],
        }),
        stroke: Some(StrokeStyle { color: Color::BLACK, width: 3.0, cap: LineCap::Square, join: LineJoin::Bevel, dash: vec![8.0, 4.0] }),
    };

    // 1. Pen: a closed shape with one smooth anchor.
    let mut pen = PenSession::default();
    pen.pointer_down(Point::new(100.0, 100.0), m, 6.0);
    pen.pointer_up();
    pen.pointer_down(Point::new(200.0, 100.0), m, 6.0);
    pen.pointer_drag(Point::new(240.0, 140.0), m);
    pen.pointer_up();
    pen.pointer_down(Point::new(150.0, 200.0), m, 6.0);
    pen.pointer_up();
    assert!(pen.pointer_down(Point::new(101.0, 101.0), m, 6.0));
    let pen_id = ops::create_path(&mut doc, layer, pen.finish().unwrap(), &grad, "Pen").unwrap();

    // 2. Pencil: an open freehand wave (no fill on open paths).
    let wave: Vec<Point> = (0..120).map(|i| Point::new(300.0 + i as f64 * 2.0, 400.0 + 30.0 * (i as f64 / 10.0).sin())).collect();
    let pencil_id = ops::create_freehand(&mut doc, layer, &wave, true, 1.5, &grad).unwrap().unwrap();

    // 3. Star with the polygon tool.
    let star_id = ops::create_shape(&mut doc, layer, ShapeTool::Polygon, Point::new(700.0, 400.0), Point::new(700.0, 340.0), m,
        &ShapeOptions { sides: 5, star: Some(0.45) }, &grad).unwrap().unwrap();

    // 4. Subselection: drag an anchor; drag a handle with Alt (break tangent).
    let s = EditSession::begin(&doc.project, scope, pen_id, EditTarget::Anchors { nodes: vec![NodeRef { subpath: 0, node: 0 }] }, Point::new(100.0, 100.0)).unwrap();
    s.update(&mut doc.project, Point::new(80.0, 90.0), m).unwrap();
    s.commit(&mut doc).unwrap();
    let s = EditSession::begin(&doc.project, scope, pen_id, EditTarget::Handle { node: NodeRef { subpath: 0, node: 1 }, side: HandleSide::Out }, Point::new(240.0, 140.0)).unwrap();
    s.update(&mut doc.project, Point::new(260.0, 100.0), Modifiers { shift: false, alt: true }).unwrap();
    s.commit(&mut doc).unwrap();

    // 5. Gradient tool: move the star's radial focal point.
    let s = EditSession::begin(&doc.project, scope, star_id, EditTarget::Gradient { part: PaintPart::Fill, handle: GradientHandle::Focal }, Point::new(700.0, 400.0)).unwrap();
    s.update(&mut doc.project, Point::new(690.0, 385.0), m).unwrap();
    s.commit(&mut doc).unwrap();

    // 6. Even-odd fill rule on the star via a property patch.
    ops::patch_element(&mut doc, star_id, &json!({"fillRule": "evenOdd"})).unwrap();

    let shape = |id| match &doc.project.element(id).unwrap().kind { ElementKind::Shape(s) => s.clone(), _ => panic!() };
    let pen_shape = shape(pen_id);
    assert!(matches!(pen_shape.fill, Some(Paint::Radial { .. })));
    let Geometry::Path(v) = &pen_shape.geometry else { panic!() };
    assert_eq!(v.subpaths[0].nodes[1].kind, NodeKind::Corner, "Alt broke the tangent");
    assert!(shape(pencil_id).fill.is_none(), "open paths are not filled");
    assert_eq!(shape(star_id).stroke.unwrap().dash, vec![8.0, 4.0]);

    // Rendering: gradients and stroke styles reach the backend.
    let ops_ = record(&doc.project);
    assert!(ops_.iter().any(|o| matches!(o, DrawOp::Fill { paint: Paint::Radial { focal: Some(_), .. }, rule: FillRule::EvenOdd, .. })));
    assert!(ops_.iter().any(|o| matches!(o, DrawOp::Stroke { stroke, .. } if stroke.cap == LineCap::Square && stroke.join == LineJoin::Bevel)));

    // Round-trip and undo.
    let json = save_to_string(&doc.project);
    let loaded = load_from_str(&json).unwrap();
    assert_eq!(loaded, doc.project);
    assert_eq!(record(&loaded), ops_);
    assert_eq!(save_to_string(&loaded), json);
    let end = doc.project.clone();
    let mut steps = 0;
    while doc.undo().unwrap() {
        steps += 1;
    }
    assert_eq!(steps, 7);
    assert_eq!(Project { next_id: original.next_id, ..doc.project.clone() }, original);
    while doc.redo().unwrap() {}
    assert_eq!(doc.project, end);
}

#[test]
fn stroke_patches_merge_per_shape() {
    let mut doc = Document::new(demo_project());
    let (center, cloud) = (find(&doc.project, "center"), find(&doc.project, "cloud"));
    ops::patch_elements(&mut doc, &[center, cloud], &json!({"stroke": {"width": 5, "cap": "square"}})).unwrap();
    let stroke = |id| match &doc.project.element(id).unwrap().kind { ElementKind::Shape(s) => s.stroke.clone().unwrap(), _ => panic!() };
    assert_eq!((stroke(center).width, stroke(center).cap), (5.0, LineCap::Square));
    assert_eq!(stroke(center).paint, Paint::solid(Color::rgb(0xb0, 0x7d, 0x10)), "own paint kept");
    assert_eq!(stroke(cloud).dash, vec![6.0, 4.0], "own dash kept");
    let ground = find(&doc.project, "ground");
    assert!(ops::patch_element(&mut doc, ground, &json!({"stroke": {"width": 5}})).is_err(), "no stroke to merge into: needs a full stroke");
}
