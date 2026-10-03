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
            if let Some(e) = l.all_elements().find(|e| e.name == name) {
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
    render_frame(p, 0, RenderOptions { view: Matrix::IDENTITY, clip_to_stage: true, show_guides, onion: None }, &mut r);
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
    // + bee (body, 2 stripes, 2 wings) + button Up (face, icon) + title text
    assert_eq!(count(&ops, |o| matches!(o, DrawOp::Fill { .. })), 4 + 3 * (1 + 6 + 1) + 5 + 2 + 1);
    // petals+center strokes ×3, guide line, cloud outline, bee (body, 2 wings), button face
    assert_eq!(count(&ops, |o| matches!(o, DrawOp::Stroke { .. })), 3 * (6 + 1) + 2 + 3 + 1);
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
    let el = Element::new(id, ElementKind::instance(flower));
    let err = doc.execute("bad", vec![Edit::InsertElement { layer, keyframe: 0, index: 0, element: el }]);
    assert!(matches!(err, Err(Error::Invalid(_))));
    assert!(!doc.is_dirty());

    let mut v: Value = serde_json::from_str(&save_to_string(&demo_project())).unwrap();
    let petal_json = v["project"]["symbols"].as_array_mut().unwrap().iter_mut().find(|s| s["name"] == "Petal").unwrap();
    petal_json["layers"][0]["keyframes"][0]["elements"]
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
    let id = ops::create_shape(&mut doc, layer, ShapeTool::Rect, Point::new(10.0, 10.0), Point::new(110.0, 60.0), Modifiers::default(), &OPTS, &style, 0)
        .unwrap()
        .unwrap();
    let e = doc.project.element(id).unwrap();
    assert_eq!((e.transform.x, e.transform.y), (60.0, 35.0));
    let b = element_bounds(&doc.project, e, 0).unwrap();
    assert_eq!((b.width(), b.height()), (104.0, 54.0), "stroke half-width included");
    assert!(ops::create_shape(&mut doc, layer, ShapeTool::Rect, Point::new(0.0, 0.0), Point::new(0.0, 0.0), Modifiers::default(), &OPTS, &style, 0)
        .unwrap()
        .is_none());

    ops::set_layer_props(&mut doc, layer, &LayerPatch { locked: Some(true), ..Default::default() }).unwrap();
    let err = ops::create_shape(&mut doc, layer, ShapeTool::Line, Point::new(0.0, 0.0), Point::new(50.0, 0.0), Modifiers::default(), &OPTS, &style, 0);
    assert!(matches!(err, Err(Error::Invalid(m)) if m.contains("locked")));
}

#[test]
fn align_and_distribute() {
    let mut doc = Document::new(demo_project());
    let layer = layer_named(&doc.project, "Flowers");
    let style = style(Some(Color::BLACK), None);
    let mk = |doc: &mut Document, x0: f64, y0: f64, w: f64| {
        ops::create_shape(doc, layer, ShapeTool::Rect, Point::new(x0, y0), Point::new(x0 + w, y0 + 20.0), Modifiers::default(), &OPTS, &style, 0)
            .unwrap()
            .unwrap()
    };
    let a = mk(&mut doc, 0.0, 0.0, 10.0);
    let b = mk(&mut doc, 50.0, 30.0, 30.0);
    let c = mk(&mut doc, 200.0, 70.0, 20.0);
    let bounds = |doc: &Document, id| element_bounds(&doc.project, doc.project.element(id).unwrap(), 0).unwrap();

    ops::align(&mut doc, &[a, b, c], Align::Top, false, 0).unwrap();
    assert!([a, b, c].iter().all(|id| bounds(&doc, *id).min.y == 0.0));

    ops::distribute(&mut doc, &[c, a, b], Distribute::SpaceX, false, 0).unwrap();
    let (ba, bb, bc) = (bounds(&doc, a), bounds(&doc, b), bounds(&doc, c));
    assert!((bb.min.x - ba.max.x - (bc.min.x - bb.max.x)).abs() < 1e-9, "equal gaps");
    assert_eq!((ba.min.x, bc.max.x), (0.0, 220.0), "outer items stay put");

    ops::align(&mut doc, &[b], Align::CenterX, false, 0).unwrap();
    assert_eq!(bounds(&doc, b).center().x, 480.0, "single element aligns to stage");
    assert_eq!(doc.undo_label(), Some("Align"));
}

#[test]
fn arrange_z_order() {
    let mut doc = Document::new(demo_project());
    let (a, b, c) = (find(&doc.project, "flower A"), find(&doc.project, "flower B"), find(&doc.project, "flower C"));
    let layer = layer_named(&doc.project, "Flowers");
    let order = |doc: &Document| doc.project.layer(layer).unwrap().keyframes[0].elements.iter().map(|e| e.id).collect::<Vec<_>>();
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
    let id = ops::import_image(&mut doc, layer, "photo.png", &png(64, 32), Point::new(100.0, 100.0), 0).unwrap();
    let ops_ = record(&doc.project);
    assert!(ops_.iter().any(|o| matches!(o, DrawOp::Image { width, height, .. } if *width == 64.0 && *height == 32.0)));
    let b = element_bounds(&doc.project, doc.project.element(id).unwrap(), 0).unwrap();
    assert_eq!((b.min.x, b.min.y, b.width()), (68.0, 84.0, 64.0));

    let json = save_to_string(&doc.project);
    assert!(json.contains("\"data\": \"iVBORw0KGgo"), "bytes stored as base64");
    let loaded = load_from_str(&json).unwrap();
    assert_eq!(loaded, doc.project);

    doc.undo().unwrap();
    assert!(doc.project.assets.iter().all(|a| a.name != "photo.png") && doc.project.element(id).is_none());
    assert!(ops::import_image(&mut doc, layer, "x.txt", b"not an image at all", Point::default(), 0).is_err());
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
    let r = ops::create_shape(&mut doc, top, ShapeTool::Rect, Point::new(100.0, 100.0), Point::new(200.0, 160.0), Modifiers::default(), &OPTS, &style, 0).unwrap().unwrap();
    let e = ops::create_shape(&mut doc, top, ShapeTool::Ellipse, Point::new(300.0, 100.0), Point::new(380.0, 200.0), Modifiers::default(), &OPTS, &style, 0).unwrap().unwrap();
    let l = ops::create_shape(&mut doc, top, ShapeTool::Line, Point::new(50.0, 300.0), Point::new(250.0, 330.0), Modifiers { shift: true, alt: false }, &OPTS, &style, 0).unwrap().unwrap();

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
    folder["keyframes"] = json!([{ "duration": 1, "elements": [{ "id": 9000, "type": "shape", "geometry": { "kind": "rect", "width": 1, "height": 1 } }] }]);
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
    assert!(save_to_string(&p).contains(&format!("\"schemaVersion\": {SCHEMA_VERSION}")));
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
    let pen_id = ops::create_path(&mut doc, layer, pen.finish().unwrap(), &grad, "Pen", 0).unwrap();

    // 2. Pencil: an open freehand wave (no fill on open paths).
    let wave: Vec<Point> = (0..120).map(|i| Point::new(300.0 + i as f64 * 2.0, 400.0 + 30.0 * (i as f64 / 10.0).sin())).collect();
    let pencil_id = ops::create_freehand(&mut doc, layer, &wave, true, 1.5, &grad, 0).unwrap().unwrap();

    // 3. Star with the polygon tool.
    let star_id = ops::create_shape(&mut doc, layer, ShapeTool::Polygon, Point::new(700.0, 400.0), Point::new(700.0, 340.0), m,
        &ShapeOptions { sides: 5, star: Some(0.45) }, &grad, 0).unwrap().unwrap();

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

// ---------- Phase 4: timeline & tweening ----------

fn scene_transform(p: &Project, frame: u32, name: &str) -> Transform {
    let scope = Scope::root(p).at(frame);
    zoetrope_core::query::scene_elements(p, scope.symbol, frame)
        .into_iter()
        .find(|se| se.element.name == name)
        .unwrap_or_else(|| panic!("{name} not shown at frame {frame}"))
        .element
        .transform
}

#[test]
fn demo_tween_interpolates_with_easing() {
    let p = demo_project();
    assert_eq!(p.symbol(p.root).unwrap().length(), 48);
    let sun0 = scene_transform(&p, 0, "sun");
    let sun36 = scene_transform(&p, 36, "sun");
    assert_eq!((sun0.y, sun36.y), (90.0, 60.0));
    // easeInOutSine at t = 0.5 is exactly halfway; at t = 0.25 it lags linear.
    assert!((scene_transform(&p, 18, "sun").y - 75.0).abs() < 1e-9);
    let q = scene_transform(&p, 9, "sun").y;
    let linear = 90.0 - 30.0 * 0.25;
    assert!(q > linear, "eased progress lags linear early on: {q} vs {linear}");
    // After the tween the end keyframe holds.
    assert_eq!(scene_transform(&p, 47, "sun").y, 60.0);
    // Past the end, nothing is drawn on that layer.
    assert!(zoetrope_core::query::scene_elements(&p, p.root, 48).is_empty());
}

#[test]
fn scrubbing_renders_are_deterministic_and_distinct() {
    let p = demo_project();
    let at = |f: u32| {
        let mut r = RecordingRenderer::default();
        render_frame(&p, f, RenderOptions::player(Matrix::IDENTITY), &mut r);
        r.ops
    };
    for f in [0, 7, 18, 35, 36, 47] {
        assert_eq!(at(f), at(f), "frame {f} deterministic");
    }
    assert_ne!(at(10), at(11));
    assert_eq!(scene_transform(&p, 36, "sun"), scene_transform(&p, 47, "sun"), "held after the tween");
}

#[test]
fn custom_bezier_easing_drives_the_tween() {
    let mut doc = Document::new(demo_project());
    let sky = layer_named(&doc.project, "Sky");
    let ease = Easing::Bezier { x1: 0.0, y1: 1.0, x2: 0.0, y2: 1.0 }; // very fast start
    ops::set_tween(&mut doc, &[sky], 5, Some(Tween { kind: TweenKind::Motion, easing: ease, rotate: 0 })).unwrap();
    let y = scene_transform(&doc.project, 4, "sun").y;
    let t = ease.apply(4.0 / 36.0);
    assert!((y - (90.0 - 30.0 * t)).abs() < 1e-9);
    assert!(t > 0.5, "bezier front-loads progress");
    // Rotation turns add full spins.
    ops::set_tween(&mut doc, &[sky], 0, Some(Tween { kind: TweenKind::Motion, easing: Easing::Linear, rotate: 1 })).unwrap();
    assert!((scene_transform(&doc.project, 18, "sun").rotation - 180.0).abs() < 1e-9);
    assert!(matches!(
        ops::set_tween(&mut doc, &[sky], 40, Some(Tween { kind: TweenKind::Motion, easing: Easing::Linear, rotate: 0 })),
        Err(Error::Invalid(m)) if m.contains("keyframe later")
    ));
}

#[test]
fn frame_ops_reshape_spans_and_undo() {
    let mut doc = Document::new(demo_project());
    let original = doc.project.clone();
    let flowers = layer_named(&doc.project, "Flowers");
    let spans = |doc: &Document| doc.project.layer(flowers).unwrap().keyframes.iter().map(|k| k.duration).collect::<Vec<_>>();
    ops::insert_frames(&mut doc, &[flowers], 10, 2).unwrap();
    assert_eq!(spans(&doc), vec![50]);
    ops::remove_frames(&mut doc, &[flowers], 10, 1).unwrap();
    assert_eq!(spans(&doc), vec![49]);
    ops::insert_keyframe(&mut doc, &[flowers], 20, false).unwrap();
    assert_eq!(spans(&doc), vec![20, 29]);
    let kf = &doc.project.layer(flowers).unwrap().keyframes;
    assert_eq!(kf[1].elements.len(), 3);
    assert_eq!(kf[1].elements[0].track(), kf[0].elements[0].track(), "copies keep their track");
    assert_ne!(kf[1].elements[0].id, kf[0].elements[0].id, "copies get fresh ids");
    ops::insert_keyframe(&mut doc, &[flowers], 30, true).unwrap();
    assert_eq!(spans(&doc), vec![20, 10, 19]);
    assert!(doc.project.layer(flowers).unwrap().keyframes[2].elements.is_empty());
    // Beyond the end: F6 stretches the last span and adds a keyframe.
    ops::insert_keyframe(&mut doc, &[flowers], 60, false).unwrap();
    assert_eq!(spans(&doc), vec![20, 10, 30, 1]);
    assert!(matches!(ops::clear_keyframe(&mut doc, &[flowers], 25), Err(Error::Invalid(_))));
    assert!(matches!(ops::clear_keyframe(&mut doc, &[flowers], 0), Err(Error::Invalid(_))));
    ops::clear_keyframe(&mut doc, &[flowers], 30).unwrap();
    assert_eq!(spans(&doc), vec![20, 40, 1]);
    while doc.undo().unwrap() {}
    assert_eq!(Project { next_id: original.next_id, ..doc.project.clone() }, original);
}

#[test]
fn keyframe_inside_a_tween_captures_the_interpolated_state() {
    let mut doc = Document::new(demo_project());
    let sky = layer_named(&doc.project, "Sky");
    let mid = scene_transform(&doc.project, 18, "sun");
    ops::insert_keyframe(&mut doc, &[sky], 18, false).unwrap();
    let l = doc.project.layer(sky).unwrap();
    assert_eq!(l.keyframes.iter().map(|k| k.duration).collect::<Vec<_>>(), vec![18, 18, 12]);
    assert!(l.keyframes[1].tween.is_some(), "both halves stay tweened");
    let sun = l.keyframes[1].elements.iter().find(|e| e.name == "sun").unwrap();
    assert_eq!(sun.transform, mid);
    // Now the first half tweens toward the captured state: the path is the same at 9…
    // (the easing restarts per half, so only endpoints are guaranteed to match)
    assert_eq!(scene_transform(&doc.project, 18, "sun"), mid);
}

#[test]
fn tweened_frames_block_direct_manipulation() {
    let mut doc = Document::new(demo_project());
    let sun = find(&doc.project, "sun");
    let scope = Scope::root(&doc.project);
    // Selectable and hit-testable at an interpolated position…
    let pos = scene_transform(&doc.project, 18, "sun");
    assert_eq!(zoetrope_core::query::hit_test(&doc.project, &scope.at(18), Point::new(pos.x, pos.y), 1.0), Some(sun));
    // …but not draggable there.
    let err = TransformSession::begin(&doc.project, scope.at(18), &[sun], DragMode::Move, Point::new(pos.x, pos.y));
    assert!(matches!(err, Err(Error::Invalid(m)) if m.contains("tweened")));
    // On the keyframe itself it works.
    assert!(TransformSession::begin(&doc.project, scope.at(0), &[sun], DragMode::Move, Point::new(820.0, 90.0)).is_ok());
    // Elements of another keyframe aren't on the current frame.
    let end_sun = doc.project.layer(layer_named(&doc.project, "Sky")).unwrap().keyframes[1].elements[0].id;
    assert!(TransformSession::begin(&doc.project, scope.at(0), &[end_sun], DragMode::Move, Point::default()).is_err());
    // Drawing on a frame past a layer's end extends it with a keyframe.
    let flowers = layer_named(&doc.project, "Flowers");
    let st = style(Some(Color::BLACK), None);
    let id = ops::create_shape(&mut doc, flowers, ShapeTool::Rect, Point::new(0.0, 0.0), Point::new(10.0, 10.0), Modifiers::default(), &OPTS, &st, 60)
        .unwrap()
        .unwrap();
    let l = doc.project.layer(flowers).unwrap();
    assert_eq!(l.keyframes.iter().map(|k| k.duration).collect::<Vec<_>>(), vec![60, 1]);
    assert_eq!(doc.project.locate(id).unwrap().keyframe, 1);
    doc.undo().unwrap();
    assert_eq!(doc.project.layer(flowers).unwrap().length(), 48, "one undo removes shape and extension");
}

#[test]
fn shape_tween_morphs_geometry_and_paint() {
    let mut doc = Document::new(demo_project());
    let root = doc.project.root;
    let layer = ops::add_layer(&mut doc, root, None, LayerKind::Normal).unwrap();
    let red = style(Some(Color::rgb(255, 0, 0)), None);
    let a = ops::create_shape(&mut doc, layer, ShapeTool::Rect, Point::new(0.0, 0.0), Point::new(100.0, 100.0), Modifiers::default(), &OPTS, &red, 0)
        .unwrap()
        .unwrap();
    ops::insert_keyframe(&mut doc, &[layer], 10, false).unwrap();
    // Turn the copy at frame 10 into a blue star.
    let copy = doc.project.layer(layer).unwrap().keyframes[1].elements[0].id;
    let star = VectorPath::polystar(5, 50.0, Some(0.5), 0.0);
    ops::patch_element(&mut doc, copy, &json!({"geometry": Geometry::Path(star), "fill": {"type": "solid", "color": "#0000ff"}})).unwrap();
    ops::set_tween(&mut doc, &[layer], 0, Some(Tween { kind: TweenKind::Shape, easing: Easing::Linear, rotate: 0 })).unwrap();
    let mid = zoetrope_core::query::scene_elements(&doc.project, doc.project.root, 5).into_iter().find(|se| se.element.id == a).unwrap().element.into_owned();
    let ElementKind::Shape(s) = &mid.kind else { panic!() };
    let Geometry::Path(v) = &s.geometry else { panic!("morphed to a path") };
    assert_eq!(v.subpaths[0].nodes.len(), 10, "rect grown to the star's node count");
    assert_eq!(s.fill, Some(Paint::solid(Color::rgb(128, 0, 128))));
    // A motion tween leaves the geometry alone.
    ops::set_tween(&mut doc, &[layer], 0, Some(Tween { kind: TweenKind::Motion, easing: Easing::Linear, rotate: 0 })).unwrap();
    let mid = zoetrope_core::query::scene_elements(&doc.project, doc.project.root, 5).into_iter().find(|se| se.element.id == a).unwrap().element.into_owned();
    let ElementKind::Shape(s) = &mid.kind else { panic!() };
    assert!(matches!(s.geometry, Geometry::Rect { .. }));
}

#[test]
fn onion_skin_draws_neighbors_faded() {
    let p = demo_project();
    let mut r = RecordingRenderer::default();
    let opts = RenderOptions { onion: Some(zoetrope_core::render::Onion { before: 2, after: 1, alpha: 0.4 }), ..RenderOptions::player(Matrix::IDENTITY) };
    render_frame(&p, 10, opts, &mut r);
    let groups: Vec<f64> = r.ops.iter().filter_map(|o| if let DrawOp::BeginGroup { alpha, .. } = o { Some(*alpha) } else { None }).collect();
    assert_eq!(groups, vec![0.2, 0.4, 0.4], "two before (farther = fainter), one after");
    // At frame 0 there is nothing before.
    let mut r0 = RecordingRenderer::default();
    render_frame(&p, 0, opts, &mut r0);
    assert_eq!(r0.ops.iter().filter(|o| matches!(o, DrawOp::BeginGroup { .. })).count(), 1);
    // Ghosts are drawn per layer (just under the Sky layer's own content, i.e.
    // above the background), and only for layers whose picture changes.
    let first_group = r.ops.iter().position(|o| matches!(o, DrawOp::BeginGroup { .. })).unwrap();
    let fills_before = r.ops[..first_group].iter().filter(|o| matches!(o, DrawOp::Fill { .. })).count();
    assert_eq!(fills_before, 2, "sky + ground drawn first");
    // Locking the animated layer turns its onion skin off (as in Flash).
    let mut locked = p.clone();
    locked.layer_mut(layer_named(&p, "Sky")).unwrap().locked = true;
    let mut rl = RecordingRenderer::default();
    render_frame(&locked, 10, opts, &mut rl);
    assert!(!rl.ops.iter().any(|o| matches!(o, DrawOp::BeginGroup { .. })));
}

#[test]
fn v2_files_migrate_to_keyframes() {
    let v2 = json!({
        "format": "zoetrope-project", "schemaVersion": 2,
        "project": { "nextId": 6, "root": 1,
            "stage": { "width": 100.0, "height": 50.0, "background": "#ffffff", "fps": 12.0 },
            "symbols": [{ "id": 1, "name": "S", "layers": [
                { "id": 2, "name": "F", "kind": "folder", "children": [
                    { "id": 3, "name": "L", "elements": [
                        { "id": 4, "type": "shape", "geometry": { "kind": "rect", "width": 10.0, "height": 10.0 } }
                    ]}
                ]},
                { "id": 5, "name": "Empty" }
            ]}]
        }
    });
    let p = load_from_str(&v2.to_string()).unwrap();
    let inner = p.layer(LayerId(3)).unwrap();
    assert_eq!(inner.keyframes.len(), 1);
    assert_eq!(inner.keyframes[0].duration, 1);
    assert_eq!(inner.keyframes[0].elements[0].id, ElementId(4));
    assert_eq!(p.layer(LayerId(5)).unwrap().keyframes, vec![Keyframe::blank(1)]);
    assert!(p.layer(LayerId(2)).unwrap().keyframes.is_empty());
    assert!(save_to_string(&p).contains(&format!("\"schemaVersion\": {SCHEMA_VERSION}")));
}

#[test]
fn gate_tweened_animation_round_trips_and_undoes() {
    let mut doc = Document::new(demo_project());
    let original = doc.project.clone();
    let root = doc.project.root;
    let layer = ops::add_layer(&mut doc, root, None, LayerKind::Normal).unwrap();
    let st = style(Some(Color::rgb(200, 30, 90)), Some((Color::BLACK, 2.0)));
    let ball = ops::create_shape(&mut doc, layer, ShapeTool::Ellipse, Point::new(40.0, 40.0), Point::new(100.0, 100.0), Modifiers::default(), &OPTS, &st, 0)
        .unwrap()
        .unwrap();
    ops::insert_keyframe(&mut doc, &[layer], 24, false).unwrap();
    // Edit the end keyframe (scope at frame 24): move and spin the ball.
    let end = doc.project.layer(layer).unwrap().keyframes[1].elements[0].id;
    let scope = Scope::root(&doc.project).at(24);
    let s = TransformSession::begin(&doc.project, scope, &[end], DragMode::Move, Point::new(70.0, 70.0)).unwrap();
    s.update(&mut doc.project, Point::new(870.0, 470.0), Modifiers::default(), &SnapConfig::default());
    s.commit(&mut doc).unwrap();
    ops::patch_element(&mut doc, end, &json!({"opacity": 0.3, "transform": {"scaleX": 2, "scaleY": 2}})).unwrap();
    let easing = Easing::Bezier { x1: 0.68, y1: -0.55, x2: 0.27, y2: 1.55 }; // "back in-out" style
    ops::set_tween(&mut doc, &[layer], 0, Some(Tween { kind: TweenKind::Motion, easing, rotate: 2 })).unwrap();

    let at = |p: &Project, f: u32| -> Element {
        zoetrope_core::query::scene_elements(p, p.root, f).into_iter().find(|se| se.element.id == ball).unwrap().element.into_owned()
    };
    let e12 = at(&doc.project, 12);
    let k = easing.apply(0.5);
    assert!((e12.transform.x - (70.0 + 800.0 * k)).abs() < 1e-9);
    assert!((e12.transform.rotation - 720.0 * k).abs() < 1e-9);
    assert!((e12.opacity - (1.0 + (0.3 - 1.0) * k)).abs() < 1e-9);
    assert!(easing.apply(0.1) < 0.0, "custom curve anticipates (goes negative)");

    // Every frame renders deterministically, identically after a round-trip.
    let json = save_to_string(&doc.project);
    let loaded = load_from_str(&json).unwrap();
    assert_eq!(loaded, doc.project);
    for f in 0..=24 {
        let (mut r1, mut r2) = (RecordingRenderer::default(), RecordingRenderer::default());
        render_frame(&doc.project, f, RenderOptions::player(Matrix::IDENTITY), &mut r1);
        render_frame(&loaded, f, RenderOptions::player(Matrix::IDENTITY), &mut r2);
        assert_eq!(r1.ops, r2.ops, "frame {f}");
    }

    let end_state = doc.project.clone();
    let mut steps = 0;
    while doc.undo().unwrap() {
        steps += 1;
    }
    assert_eq!(steps, 6, "layer, ellipse, keyframe, move, properties, tween");
    assert_eq!(Project { next_id: original.next_id, ..doc.project.clone() }, original);
    while doc.redo().unwrap() {}
    assert_eq!(doc.project, end_state);
}

// ---------- Phase 5: symbols & nested timelines ----------

use zoetrope_core::player::{Player, PlayerEvent};
use zoetrope_core::render::{render_editing, EditView};

fn record_player(p: &Project, pl: &Player) -> Vec<DrawOp> {
    let mut r = RecordingRenderer::default();
    pl.render(p, RenderOptions::player(Matrix::IDENTITY), &mut r);
    r.ops
}

fn record_stateless(p: &Project, f: u32) -> Vec<DrawOp> {
    let mut r = RecordingRenderer::default();
    render_frame(p, f, RenderOptions::player(Matrix::IDENTITY), &mut r);
    r.ops
}

fn bee_frames(p: &Project, pl: &Player) -> Vec<u32> {
    let bees = layer_named(p, "Bees").0;
    let mut v: Vec<(u32, u32)> = pl.clip_frames().iter().filter(|(path, _)| path.len() == 1 && path[0].0 == bees).map(|(path, f)| (path[0].1, *f)).collect();
    v.sort();
    v.into_iter().map(|(_, f)| f).collect()
}

#[test]
fn movie_clips_run_independent_clocks() {
    let p = demo_project();
    let mut pl = Player::new(&p, 0);
    assert_eq!(bee_frames(&p, &pl), vec![0]);
    for _ in 0..20 {
        pl.tick(&p);
    }
    // bee1 appeared at 0, bee2 at 8, bee3 at 16; each loops over 13 frames.
    assert_eq!(pl.frame, 20);
    assert_eq!(bee_frames(&p, &pl), vec![20 % 13, 12, 4]);
}

#[test]
fn runtime_matches_stateless_until_the_root_loops() {
    let p = demo_project();
    let mut pl = Player::new(&p, 0);
    for f in 0..48 {
        assert_eq!(record_player(&p, &pl), record_stateless(&p, f), "frame {f}");
        pl.tick(&p);
    }
    // After the root loops, a movie clip that is still on stage keeps running
    // (bee1, present in every keyframe); the others left and will restart.
    assert_eq!(pl.frame, 0);
    assert_eq!(bee_frames(&p, &pl), vec![48 % 13]);
    assert_ne!(record_player(&p, &pl), record_stateless(&p, 0));
}

#[test]
fn graphic_symbols_follow_their_parent() {
    let mut doc = Document::new(demo_project());
    let bee = doc.project.symbols.iter().find(|s| s.name == "Bee").unwrap().id;
    ops::set_symbol_props(&mut doc, bee, None, Some(SymbolKind::Graphic)).unwrap();
    let bee2 = doc.project.layer(layer_named(&doc.project, "Bees")).unwrap().keyframes[1].elements[1].id;
    let bees = doc.project.layer(layer_named(&doc.project, "Bees")).unwrap().keyframes.clone();
    let child = |doc: &Document, id, f| zoetrope_core::query::child_frame_of(&doc.project, id, f);
    assert_eq!(child(&doc, bees[0].elements[0].id, 5), 5);
    // bee1's copy in keyframe 2 (starting at 16) restarts its graphic count there.
    assert_eq!(child(&doc, bees[2].elements[0].id, 20), 4);
    ops::patch_element(&mut doc, bee2, &json!({"firstFrame": 3, "loopMode": "singleFrame"})).unwrap();
    assert_eq!(zoetrope_core::query::child_frame_of(&doc.project, bee2, 12), 3);
    ops::patch_element(&mut doc, bee2, &json!({"loopMode": "playOnce"})).unwrap();
    assert_eq!(zoetrope_core::query::child_frame_of(&doc.project, bee2, 15), 10);
}

#[test]
fn buttons_respond_to_the_pointer() {
    let p = demo_project();
    let mut pl = Player::new(&p, 0);
    let frame_of_button = |pl: &Player| {
        use zoetrope_core::render::Clock;
        let ui = layer_named(&p, "UI").0;
        let track = find(&p, "playButton").0;
        pl.frame(&[(ui, track)], SymbolKind::Button, 0)
    };
    assert_eq!(frame_of_button(&pl), 0, "up");
    assert!(pl.pointer(&p, Some(Point::new(500.0, 300.0)), false).is_empty());
    assert!(!pl.over_button());
    pl.pointer(&p, Some(Point::new(110.0, 505.0)), false);
    assert!(pl.over_button());
    assert_eq!(frame_of_button(&pl), 1, "over");
    let press = pl.pointer(&p, Some(Point::new(110.0, 505.0)), true);
    assert!(matches!(&press[..], [PlayerEvent::Press { name, .. }] if name == "playButton"));
    assert_eq!(frame_of_button(&pl), 2, "down");
    let click = pl.pointer(&p, Some(Point::new(112.0, 506.0)), false);
    assert!(matches!(&click[..], [PlayerEvent::Release { .. }, PlayerEvent::Click { name, .. }] if name == "playButton"));
    // Pressing then releasing outside is a release but not a click.
    pl.pointer(&p, Some(Point::new(110.0, 505.0)), true);
    let outside = pl.pointer(&p, Some(Point::new(600.0, 100.0)), false);
    assert!(matches!(&outside[..], [PlayerEvent::Release { name, .. }] if name == "playButton"));
    // The Hit frame is never drawn: the button renders its Up art (2 fills).
    let ops_ = record_player(&p, &pl);
    assert!(ops_.iter().all(|o| !matches!(o, DrawOp::Fill { paint: Paint::Solid { color }, .. } if *color == Color::BLACK)));
}

#[test]
fn convert_to_symbol_keeps_the_picture() {
    let mut doc = Document::new(demo_project());
    let ids = [find(&doc.project, "sun"), find(&doc.project, "cloud")];
    let before = record(&doc.project);
    let ground = find(&doc.project, "ground");
    let (instance, symbol) = ops::convert_to_symbol(&mut doc, &[ground], "Ground", SymbolKind::Graphic, 0).unwrap();
    let after = record(&doc.project);
    assert_eq!(before.len(), after.len());
    for (a, b) in before.iter().zip(&after) {
        if let (DrawOp::Fill { transform: ta, .. }, DrawOp::Fill { transform: tb, .. }) = (a, b) {
            assert!(ta.approx_eq(tb, 1e-9));
        }
    }
    let inst = doc.project.element(instance).unwrap();
    assert_eq!((inst.transform.x, inst.transform.y), (480.0, 460.0));
    assert_eq!(doc.project.symbol(symbol).unwrap().name, "Ground");
    // Tweened elements can't be converted mid-tween.
    assert!(ops::convert_to_symbol(&mut doc, &ids, "Sky stuff", SymbolKind::MovieClip, 10).is_err());
    // One undo restores everything.
    doc.undo().unwrap();
    assert_eq!(record(&doc.project), before);
    assert!(doc.project.symbol(symbol).is_none());
}

#[test]
fn library_operations() {
    let mut doc = Document::new(demo_project());
    let flower = doc.project.symbols.iter().find(|s| s.name == "Flower").unwrap().id;
    let petal = doc.project.symbols.iter().find(|s| s.name == "Petal").unwrap().id;
    ops::set_symbol_props(&mut doc, flower, Some("Daisy"), None).unwrap();
    assert_eq!(doc.project.symbol(flower).unwrap().name, "Daisy");
    let copy = ops::duplicate_symbol(&mut doc, flower).unwrap();
    assert_eq!(doc.project.symbol(copy).unwrap().name, "Daisy copy");
    doc.project.validate().unwrap();
    assert!(ops::delete_symbol(&mut doc, flower).is_err(), "in use");
    let root = doc.project.root;
    assert!(ops::delete_symbol(&mut doc, root).is_err(), "root");
    ops::delete_symbol(&mut doc, copy).unwrap();
    // Swap: flower A becomes a petal; swapping a petal instance inside Petal → cycle.
    let a = find(&doc.project, "flower A");
    ops::swap_symbol(&mut doc, &[a], petal).unwrap();
    let inner_petal = doc.project.symbol(flower).unwrap().layers[1].keyframes[0].elements[0].id;
    assert!(ops::swap_symbol(&mut doc, &[inner_petal], flower).is_err(), "Daisy would contain itself");
    let flowers = layer_named(&doc.project, "Flowers");
    let placed = ops::place_instance(&mut doc, flowers, flower, Point::new(10.0, 20.0), 0).unwrap();
    assert_eq!(doc.project.element(placed).unwrap().transform.x, 10.0);
    let json = save_to_string(&doc.project);
    assert_eq!(load_from_str(&json).unwrap(), doc.project);
    while doc.undo().unwrap() {}
    assert_eq!(Project { next_id: demo_project().next_id, ..doc.project.clone() }, demo_project());
}

#[test]
fn edit_in_place_view_dims_context_and_skips_the_instance() {
    let p = demo_project();
    let b = find(&p, "flower B");
    let flower = p.symbols.iter().find(|s| s.name == "Flower").unwrap().id;
    let m = p.element(b).unwrap().transform.matrix();
    let view = EditView { path: &[b], root_frame: 0, symbol: flower, frame: 0, matrix: m, context_alpha: 0.3 };
    let mut r = RecordingRenderer::default();
    render_editing(&p, &view, RenderOptions { view: Matrix::IDENTITY, clip_to_stage: false, show_guides: true, onion: None }, &mut r);
    assert!(matches!(r.ops[1], DrawOp::BeginGroup { alpha, .. } if alpha == 0.3));
    let end = r.ops.iter().position(|o| *o == DrawOp::EndGroup).unwrap();
    // Context: everything but flower B (8 fills fewer); then flower B's symbol at full strength.
    let context_fills = r.ops[..end].iter().filter(|o| matches!(o, DrawOp::Fill { .. })).count();
    let full_fills = record(&p).iter().filter(|o| matches!(o, DrawOp::Fill { .. })).count();
    assert_eq!(context_fills, full_fills - 8);
    let edited_fills = r.ops[end..].iter().filter(|o| matches!(o, DrawOp::Fill { .. })).count();
    assert_eq!(edited_fills, 8);
}

#[test]
fn edits_inside_a_symbol_use_symbol_space() {
    let mut doc = Document::new(demo_project());
    let b = find(&doc.project, "flower B");
    let flower = doc.project.symbols.iter().find(|s| s.name == "Flower").unwrap().id;
    let scope = Scope { symbol: flower, matrix: doc.project.element(b).unwrap().transform.matrix(), frame: 0 };
    let center = find(&doc.project, "center");
    // Flower B sits at (480, 210) unscaled, so its center is at stage (480, 210).
    assert_eq!(zoetrope_core::query::hit_test(&doc.project, &scope, Point::new(480.0, 210.0), 1.0), Some(center));
    let s = TransformSession::begin(&doc.project, scope, &[center], DragMode::Move, Point::new(480.0, 210.0)).unwrap();
    s.update(&mut doc.project, Point::new(490.0, 200.0), Modifiers::default(), &SnapConfig::default());
    s.commit(&mut doc).unwrap();
    let t = doc.project.element(center).unwrap().transform;
    assert_eq!((t.x, t.y), (10.0, -10.0));
}

// ---------- Phase 6: text & audio ----------

use zoetrope_core::ops::TextStyle;
use zoetrope_core::text::{TextAlign, DEFAULT_FONT};

fn fonts(p: &Project) -> usize {
    p.assets.iter().filter(|a| matches!(a.kind, zoetrope_core::asset::AssetKind::Font { .. })).count()
}

#[test]
fn text_embeds_its_font_and_renders_as_paths() {
    let mut p = demo_project();
    // Start from a project without the bundled font.
    p.symbol_mut(p.root).unwrap().layers.retain(|l| l.name != "Title");
    p.assets.retain(|a| !matches!(a.kind, zoetrope_core::asset::AssetKind::Font { .. }));
    let mut doc = Document::new(p);
    assert_eq!(fonts(&doc.project), 0);
    let layer = layer_named(&doc.project, "Flowers");
    let style = TextStyle { size: 40.0, color: Color::rgb(200, 0, 0), ..Default::default() };
    let a = ops::create_text(&mut doc, layer, Point::new(100.0, 100.0), "Hello", &style, None, 0).unwrap();
    let b = ops::create_text(&mut doc, layer, Point::new(100.0, 200.0), "World", &style, None, 0).unwrap();
    assert_eq!(fonts(&doc.project), 1, "the default font is embedded once");
    let ops_ = record(&doc.project);
    assert!(ops_.iter().any(|o| matches!(o, DrawOp::Fill { paint: Paint::Solid { color }, transform, .. }
        if *color == Color::rgb(200, 0, 0) && transform.e == 100.0 && transform.f == 100.0)));
    // Bounds = the laid-out box; the whole box is clickable.
    let e = doc.project.element(a).unwrap();
    let bx = element_bounds(&doc.project, e, 0).unwrap();
    assert_eq!((bx.min.x, bx.min.y), (100.0, 100.0));
    assert!(bx.width() > 60.0 && bx.height() > 40.0);
    let scope = Scope::root(&doc.project);
    assert_eq!(zoetrope_core::query::hit_test(&doc.project, &scope, Point::new(bx.max.x - 1.0, bx.max.y - 1.0), 0.0), Some(a));
    // Text properties patch like any other.
    ops::patch_element(&mut doc, b, &json!({"text": "Wide\nLines", "align": "center", "size": 20, "width": 300})).unwrap();
    let ElementKind::Text(t) = doc.project.element(b).unwrap().kind.clone() else { panic!() };
    assert_eq!((t.text.as_str(), t.align, t.width), ("Wide\nLines", TextAlign::Center, Some(300.0)));
    assert!(ops::patch_element(&mut doc, b, &json!({"size": -1})).is_err());
    let sun = find(&doc.project, "sun");
    assert!(ops::patch_element(&mut doc, sun, &json!({"text": "x"})).is_err());
    // Paint styles fit gradients to the text box; text has no stroke.
    let stops = vec![GradientStop { offset: 0.0, color: Color::BLACK }, GradientStop { offset: 1.0, color: Color::WHITE }];
    ops::set_paint_style(&mut doc, &[b], PaintPart::Fill, Some(&PaintStyle::Linear { stops })).unwrap();
    let ElementKind::Text(g) = &doc.project.element(b).unwrap().kind else { panic!() };
    assert!(matches!(&g.fill, Paint::Linear { end, .. } if (end.x - 300.0).abs() < 1e-9));
    assert!(ops::set_paint_style(&mut doc, &[b], PaintPart::Stroke, Some(&PaintStyle::Solid { color: Color::BLACK })).is_err());
    assert!(ops::set_paint_style(&mut doc, &[b], PaintPart::Fill, None).is_err());
    // The font can't be deleted while text uses it.
    let font = t.font;
    assert!(doc.execute("bad", vec![Edit::RemoveAsset { asset: font }]).is_err());
    // Round trip and undo.
    let json = save_to_string(&doc.project);
    assert_eq!(load_from_str(&json).unwrap(), doc.project);
    assert_eq!(record(&load_from_str(&json).unwrap()), record(&doc.project));
    for _ in 0..4 {
        doc.undo().unwrap();
    }
    assert_eq!(fonts(&doc.project), 0);
}

#[test]
fn fonts_import_and_validate() {
    let mut doc = Document::new(demo_project());
    let id = ops::import_font(&mut doc, "copy.ttf", DEFAULT_FONT).unwrap();
    assert!(matches!(&doc.project.asset(id).unwrap().kind, zoetrope_core::asset::AssetKind::Font { family, .. } if family == "Zoetrope Sans"));
    assert!(ops::import_font(&mut doc, "junk.ttf", b"nope").is_err());
    let style = TextStyle { font: Some(id), ..Default::default() };
    let layer = layer_named(&doc.project, "Flowers");
    assert!(ops::create_text(&mut doc, layer, Point::default(), "x", &style, None, 0).is_ok());
    let bad = TextStyle { font: Some(find(&doc.project, "sun").0.into_asset()), ..Default::default() };
    assert!(ops::create_text(&mut doc, layer, Point::default(), "x", &bad, None, 0).is_err());
}

trait IntoAsset {
    fn into_asset(self) -> AssetId;
}
impl IntoAsset for u32 {
    fn into_asset(self) -> AssetId {
        AssetId(self)
    }
}

#[test]
fn audio_import_sound_cues_and_validation() {
    let mut doc = Document::new(demo_project());
    let wav = zoetrope_core::demo::chime_wav();
    let clip = ops::import_audio(&mut doc, "beep.wav", &wav, 2.0).unwrap();
    assert!(ops::import_audio(&mut doc, "x.wav", b"not audio", 1.0).is_err());
    assert!(ops::import_audio(&mut doc, "y.wav", &wav, f64::NAN).is_err());
    let bees = layer_named(&doc.project, "Bees");
    // An event sound on the keyframe starting at frame 8.
    ops::set_sound(&mut doc, &[bees], 8, Some(SoundRef { asset: clip, sync: SoundSync::Event, volume: 0.5, loops: 1 })).unwrap();
    assert!(ops::set_sound(&mut doc, &[bees], 8, Some(SoundRef { asset: clip, sync: SoundSync::Event, volume: 2.0, loops: 0 })).is_err());
    let sun_id = find(&doc.project, "sun").0;
    assert!(ops::set_sound(&mut doc, &[bees], 8, Some(SoundRef { asset: AssetId(sun_id), sync: SoundSync::Event, volume: 1.0, loops: 0 })).is_err());
    // The clip can't be removed while a keyframe uses it.
    assert!(doc.execute("bad", vec![Edit::RemoveAsset { asset: clip }]).is_err());

    let p = &doc.project;
    let mut pl = Player::new(p, 0);
    // The demo's tune streams in sync from frame 0.
    let streams = pl.sound_streams().to_vec();
    assert_eq!(streams.len(), 1);
    assert_eq!((streams[0].position, streams[0].sync), (0.0, SoundSync::Stream));
    assert!(pl.take_sound_events().is_empty());
    for _ in 0..8 {
        pl.tick(p);
    }
    // Frame 8: the stream is 8/24 s in; the event fires exactly once.
    assert!((pl.sound_streams()[0].position - 8.0 / 24.0).abs() < 1e-12);
    let ev = pl.take_sound_events();
    assert_eq!(ev.len(), 1);
    assert_eq!((ev[0].asset, ev[0].volume, ev[0].loops), (clip, 0.5, 1));
    pl.tick(p);
    assert!(pl.take_sound_events().is_empty());
    // Removing the sound is an undoable timeline edit.
    ops::set_sound(&mut doc, &[bees], 8, None).unwrap();
    assert!(doc.project.layer(bees).unwrap().keyframes[1].sound.is_none());
    doc.undo().unwrap();
    assert!(doc.project.layer(bees).unwrap().keyframes[1].sound.is_some());
    let json = save_to_string(&doc.project);
    assert_eq!(load_from_str(&json).unwrap(), doc.project);
}

#[test]
fn v3_files_load_as_v4() {
    let mut v: Value = serde_json::from_str(&save_to_string(&demo_project())).unwrap();
    v["schemaVersion"] = json!(3);
    let p = load_from_str(&v.to_string()).unwrap();
    assert_eq!(p, demo_project());
}

// ---------------------------------------------------------------- Phase 7: scripting runtime

use zoetrope_core::demo::{game_project, GAME_OVER_SCRIPT, GAME_PLAY_SCRIPT};
use zoetrope_core::player::{FrameTarget, Override};
use zoetrope_core::script::{self, Call};

fn script_call(p: &Project, pl: &mut Player, v: Value) -> Value {
    script::call(p, pl, serde_json::from_value::<Call>(v).unwrap()).unwrap()
}

#[test]
fn game_demo_is_valid_and_round_trips() {
    let p = game_project();
    p.validate().unwrap();
    let json = save_to_string(&p);
    assert_eq!(load_from_str(&json).unwrap(), p);
    assert!(json.contains("\"schemaVersion\": 5"));
}

#[test]
fn entering_frames_queues_frame_and_symbol_scripts() {
    let p = game_project();
    let mut pl = Player::new(&p, 0);
    let frames = pl.take_frame_scripts(&p);
    assert_eq!(frames.len(), 1);
    assert_eq!((frames[0].path.len(), frames[0].frame, frames[0].script.as_str()), (0, 0, GAME_PLAY_SCRIPT));
    assert_eq!(script::frame_script_location(&p, frames[0].symbol, frames[0].layer, 0), "Bee Catcher › Actions › frame 1");
    // The flower's symbol script runs for its instance; the bee has none.
    let inst = pl.take_instance_scripts(&p);
    assert_eq!(inst.len(), 1);
    assert_eq!(pl.resolve(&p, &inst[0].path).unwrap().element.name, "flower");
    // Nothing is due again until a frame is entered.
    assert!(pl.take_frame_scripts(&p).is_empty());

    // stop() holds the root; movie clips keep playing.
    script_call(&p, &mut pl, json!({"op": "stop", "path": []}));
    let bee = pl.child_named(&p, &[], "bee").unwrap();
    let bee_frame = pl.timeline(&p, &bee).unwrap().frame;
    pl.tick(&p);
    assert_eq!(pl.frame, 0);
    assert!(pl.take_frame_scripts(&p).is_empty(), "a stopped root doesn't re-enter its frame");
    assert_eq!(pl.timeline(&p, &bee).unwrap().frame, bee_frame + 1);
    // ...unless stopped themselves.
    script_call(&p, &mut pl, json!({"op": "stop", "path": bee}));
    pl.tick(&p);
    assert_eq!(pl.timeline(&p, &bee).unwrap(), zoetrope_core::player::TimelineState { frame: bee_frame + 1, length: 13, playing: false });

    // Jumping by label enters the frame: its script is due, and the bee and
    // flower, which only exist on frame 1, leave the stage.
    script_call(&p, &mut pl, json!({"op": "goto", "path": [], "frame": "over", "play": false}));
    assert_eq!(pl.frame, 1);
    let over = pl.take_frame_scripts(&p);
    assert_eq!(over.iter().map(|f| f.script.as_str()).collect::<Vec<_>>(), [GAME_OVER_SCRIPT]);
    let removed = pl.take_removed();
    assert!(removed.contains(&bee));
    assert_eq!(removed.len(), 8, "bee, flower and the flower's six petals");
    assert!(pl.child_named(&p, &[], "bee").is_none());
    // Back to "play" (0-based index 0 works too): a fresh bee and flower.
    script_call(&p, &mut pl, json!({"op": "goto", "path": [], "frame": 0, "play": false}));
    assert_eq!(pl.take_frame_scripts(&p).len(), 1);
    assert_eq!(pl.take_instance_scripts(&p).len(), 1, "the new flower runs its symbol script again");
    assert_eq!(pl.timeline(&p, &bee).unwrap().frame, 0, "the new bee starts from its first frame");
    // Errors are reported, not ignored.
    assert!(script::call(&p, &mut pl, Call::Goto { path: vec![], frame: FrameTarget::Label("nope".into()), play: true }).is_err());
    let flower = pl.child_named(&p, &[], "flower").unwrap();
    assert!(script::call(&p, &mut pl, Call::Stop { path: flower.clone() }).is_err(), "graphics have no playhead");
}

#[test]
fn scripted_properties_override_the_timeline() {
    let p = game_project();
    let mut pl = Player::new(&p, 0);
    pl.set_playing(&p, &[], false).unwrap();
    let bee = pl.child_named(&p, &[], "bee").unwrap();
    let flower = pl.child_named(&p, &[], "flower").unwrap();
    let props = script_call(&p, &mut pl, json!({"op": "get", "path": bee}));
    assert_eq!((props["kind"].as_str(), props["x"].as_f64(), props["symbol"].as_str()), (Some("movieClip"), Some(140.0), Some("Bee")));
    assert!(!pl.hit_test_object(&p, &bee, &flower));

    // Move the bee onto the flower: rendering, bounds and collisions follow.
    script_call(&p, &mut pl, json!({"op": "set", "path": bee, "props": {"x": 600.0, "y": 300.0, "rotation": 30.0}}));
    assert_eq!(script_call(&p, &mut pl, json!({"op": "get", "path": bee}))["x"], 600.0);
    assert!(pl.hit_test_object(&p, &bee, &flower));
    assert!(pl.hit_test_point(&p, &bee, Point::new(600.0, 300.0), true));
    assert!(!pl.hit_test_point(&p, &bee, Point::new(140.0, 300.0), false));
    let moved = record_player(&p, &pl);
    assert!(moved.iter().any(|o| matches!(o, DrawOp::Fill { transform, .. } if (transform.e - 600.0).abs() < 30.0)));
    // The override survives ticks (the timeline no longer moves it).
    pl.tick(&p);
    assert_eq!(pl.resolve(&p, &bee).unwrap().element.transform.x, 600.0);

    // Invisible objects don't draw or collide.
    let before = record_player(&p, &pl).len();
    script_call(&p, &mut pl, json!({"op": "set", "path": bee, "props": {"visible": false}}));
    assert!(record_player(&p, &pl).len() < before);
    assert!(!pl.hit_test_object(&p, &bee, &flower));

    // Text content can be scripted (and only on text).
    let score = pl.child_named(&p, &[], "scoreText").unwrap();
    script_call(&p, &mut pl, json!({"op": "set", "path": score, "props": {"text": "Score: 42"}}));
    assert_eq!(script_call(&p, &mut pl, json!({"op": "get", "path": score}))["text"], "Score: 42");
    assert!(script::call(&p, &mut pl, Call::Set { path: flower.clone(), props: Override { text: Some("x".into()), ..Default::default() } }).is_err());
    assert!(script::call(&p, &mut pl, Call::Set { path: flower, props: Override { x: Some(f64::NAN), ..Default::default() } }).is_err());

    let names: Vec<String> = script_call(&p, &mut pl, json!({"op": "children", "path": []}))
        .as_array()
        .unwrap()
        .iter()
        .map(|c| c["name"].as_str().unwrap().to_string())
        .collect();
    for n in ["flower", "bee", "scoreText", "timeText", "restartButton"] {
        assert!(names.iter().any(|m| m == n), "{n} in {names:?}");
    }
    let b = script_call(&p, &mut pl, json!({"op": "bounds", "path": score}));
    assert!(b["width"].as_f64().unwrap() > 50.0);
}

#[test]
fn scripted_buttons_move_their_hit_area() {
    let p = game_project();
    let mut pl = Player::new(&p, 0);
    let button = pl.child_named(&p, &[], "restartButton").unwrap();
    pl.pointer(&p, Some(Point::new(860.0, 500.0)), false);
    assert!(pl.over_button());
    pl.set_properties(&p, &button, Override { x: Some(100.0), ..Default::default() }).unwrap();
    assert!(!pl.over_button());
    pl.pointer(&p, Some(Point::new(100.0, 500.0)), true);
    let events = pl.pointer(&p, Some(Point::new(100.0, 500.0)), false);
    assert!(matches!(&events[..], [PlayerEvent::Release { name, .. }, PlayerEvent::Click { path, .. }] if name == "restartButton" && *path == button));
}

#[test]
fn frame_labels_and_scripts_are_editable_and_undoable() {
    let mut doc = Document::new(demo_project());
    let ui = layer_named(&doc.project, "UI");
    ops::set_frame_script(&mut doc, &[ui], 5, Some("stop();")).unwrap();
    ops::set_frame_label(&mut doc, &[ui], 5, Some("  intro ")).unwrap();
    let k = &doc.project.layer(ui).unwrap().keyframes[0];
    assert_eq!((k.script.as_deref(), k.label.as_deref()), (Some("stop();"), Some("intro")));
    // Blank text removes them.
    ops::set_frame_label(&mut doc, &[ui], 0, Some(" ")).unwrap();
    assert_eq!(doc.project.layer(ui).unwrap().keyframes[0].label, None);
    let bee = doc.project.symbols.iter().find(|s| s.name == "Bee").unwrap().id;
    ops::set_symbol_script(&mut doc, bee, Some("this.alpha = 0.5;")).unwrap();
    assert_eq!(doc.project.symbol(bee).unwrap().script.as_deref(), Some("this.alpha = 0.5;"));
    let json = save_to_string(&doc.project);
    assert_eq!(load_from_str(&json).unwrap(), doc.project);
    for _ in 0..4 {
        doc.undo().unwrap();
    }
    assert_eq!(doc.project, demo_project());
}

#[test]
fn v4_files_load_as_v5() {
    let mut v: Value = serde_json::from_str(&save_to_string(&demo_project())).unwrap();
    v["schemaVersion"] = json!(4);
    assert_eq!(load_from_str(&v.to_string()).unwrap(), demo_project());
}
