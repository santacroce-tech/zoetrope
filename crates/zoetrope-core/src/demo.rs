//! Hardcoded demo scene. Three levels of nesting:
//! Scene → Flower (instanced 3×) → Petal (instanced 6× per flower) → shape,
//! plus a layer folder and a guide layer.

use crate::color::Color;
use crate::model::*;

struct Builder {
    project: Project,
}

/// Where to put a new layer: a symbol's top level or inside a folder.
#[derive(Clone, Copy)]
enum Parent {
    Symbol(SymbolId),
    Folder(LayerId),
}

impl Builder {
    fn id(&mut self) -> u32 {
        self.project.alloc_id()
    }

    fn symbol(&mut self, name: &str) -> SymbolId {
        let id = SymbolId(self.id());
        self.project.symbols.push(Symbol { id, name: name.into(), layers: Vec::new() });
        id
    }

    fn layer(&mut self, parent: Parent, name: &str, kind: LayerKind) -> LayerId {
        let id = LayerId(self.id());
        let layer = Layer::new(id, name, kind);
        match parent {
            Parent::Symbol(s) => self.project.symbol_mut(s).unwrap().layers.push(layer),
            Parent::Folder(f) => self.project.layer_mut(f).unwrap().children.push(layer),
        }
        id
    }

    fn add(&mut self, layer: LayerId, name: &str, transform: Transform, kind: ElementKind) {
        let mut e = Element::new(ElementId(self.id()), kind);
        e.name = name.into();
        e.transform = transform;
        self.project.layer_mut(layer).unwrap().elements.push(e);
    }
}

fn shape(geometry: Geometry, fill: Option<Color>, stroke: Option<(f64, Color)>) -> ElementKind {
    ElementKind::Shape(Shape {
        geometry,
        fill: fill.map(|color| Fill::Solid { color }),
        stroke: stroke.map(|(width, color)| Stroke { width, color }),
    })
}

pub fn demo_project() -> Project {
    let mut b = Builder {
        project: Project {
            next_id: 1,
            stage: Stage { width: 960.0, height: 540.0, background: Color::rgb(0xf4, 0xef, 0xe6), fps: 24.0 },
            root: SymbolId(0),
            symbols: Vec::new(),
            assets: Vec::new(),
        },
    };

    let scene = b.symbol("Scene 1");
    b.project.root = scene;

    let petal = b.symbol("Petal");
    let pl = b.layer(Parent::Symbol(petal), "Petal", LayerKind::Normal);
    b.add(
        pl,
        "",
        Transform::at(0.0, -52.0),
        shape(
            Geometry::Ellipse { width: 44.0, height: 96.0 },
            Some(Color::rgb(0xe8, 0x6a, 0x92)),
            Some((2.0, Color::rgb(0xa8, 0x3a, 0x62))),
        ),
    );

    let flower = b.symbol("Flower");
    let stem = b.layer(Parent::Symbol(flower), "Stem", LayerKind::Normal);
    b.add(stem, "stem", Transform::at(0.0, 150.0), shape(Geometry::Rect { width: 10.0, height: 300.0 }, Some(Color::rgb(0x4c, 0x8c, 0x4a)), None));
    let petals = b.layer(Parent::Symbol(flower), "Petals", LayerKind::Normal);
    for i in 0..6 {
        let t = Transform { rotation: i as f64 * 60.0, ..Default::default() };
        b.add(petals, "", t, ElementKind::Instance { symbol: petal });
    }
    let center = b.layer(Parent::Symbol(flower), "Center", LayerKind::Normal);
    b.add(
        center,
        "center",
        Transform::default(),
        shape(Geometry::Ellipse { width: 56.0, height: 56.0 }, Some(Color::rgb(0xf5, 0xc5, 0x18)), Some((3.0, Color::rgb(0xb0, 0x7d, 0x10)))),
    );

    let scenery = b.layer(Parent::Symbol(scene), "Scenery", LayerKind::Folder);
    let bg = b.layer(Parent::Folder(scenery), "Background", LayerKind::Normal);
    b.add(bg, "sky", Transform::at(480.0, 190.0), shape(Geometry::Rect { width: 960.0, height: 380.0 }, Some(Color::rgb(0xbf, 0xe3, 0xf2)), None));
    b.add(bg, "ground", Transform::at(480.0, 460.0), shape(Geometry::Rect { width: 960.0, height: 160.0 }, Some(Color::rgb(0x8c, 0xc0, 0x6b)), None));
    let sky = b.layer(Parent::Folder(scenery), "Sky", LayerKind::Normal);
    b.add(sky, "sun", Transform::at(820.0, 90.0), shape(Geometry::Ellipse { width: 110.0, height: 110.0 }, Some(Color::rgb(0xff, 0xd8, 0x4d)), None));

    let flowers = b.layer(Parent::Symbol(scene), "Flowers", LayerKind::Normal);
    let place = |x, y, s: f64, rotation, skew_x| Transform { x, y, scale_x: s, scale_y: s, rotation, skew_x, ..Default::default() };
    b.add(flowers, "flower A", place(220.0, 250.0, 0.8, -8.0, 0.0), ElementKind::Instance { symbol: flower });
    b.add(flowers, "flower B", place(480.0, 210.0, 1.0, 0.0, 0.0), ElementKind::Instance { symbol: flower });
    b.add(flowers, "flower C", place(730.0, 290.0, 0.6, 12.0, -15.0), ElementKind::Instance { symbol: flower });

    let guides = b.layer(Parent::Symbol(scene), "Layout guide", LayerKind::Guide);
    b.add(
        guides,
        "horizon",
        Transform::at(480.0, 380.0),
        shape(Geometry::Line { dx: 960.0, dy: 0.0 }, None, Some((1.0, Color::rgb(0x00, 0x99, 0xff)))),
    );

    debug_assert!(b.project.validate().is_ok());
    b.project
}
