//! Headless traversal profile: a deep, wide tree of nested instances.
//! Run with: cargo run -p zoetrope-core --release --example profile

use std::time::Instant;
use zoetrope_core::demo::demo_project;
use zoetrope_core::render::{render_frame, NullRenderer, RecordingRenderer, RenderOptions};
use zoetrope_core::*;

fn main() {
    // Scene with N flowers (each = 1 stem + 6 nested petals + center).
    for n in [3usize, 100, 1000] {
        let mut p = demo_project();
        let flower = p.symbols.iter().find(|s| s.name == "Flower").unwrap().id;
        let root = p.root;
        let mut extra = Vec::new();
        for i in 0..n {
            let id = ElementId(p.alloc_id());
            let t = Transform { x: (i % 40) as f64 * 24.0, y: (i / 40) as f64 * 20.0, scale_x: 0.2, scale_y: 0.2, ..Default::default() };
            let mut e = Element::new(id, ElementKind::instance(flower));
            e.transform = t;
            extra.push(e);
        }
        let flowers = p.symbol(root).unwrap().layers.iter().find(|l| l.name == "Flowers").unwrap().id;
        p.layer_mut(flowers).unwrap().keyframes[0].elements = extra;

        let opts = RenderOptions::player(Matrix::IDENTITY);
        let mut rec = RecordingRenderer::default();
        render_frame(&p, 0, opts, &mut rec);
        let draws = rec.ops.len() - 2;

        let iters = 200;
        let t0 = Instant::now();
        for _ in 0..iters {
            render_frame(&p, 0, opts, &mut NullRenderer);
        }
        let per = t0.elapsed().as_secs_f64() * 1000.0 / iters as f64;
        println!("{n:>5} flower instances, {draws:>6} draw calls: {per:.3} ms/frame traversal (core only)");
    }
}
