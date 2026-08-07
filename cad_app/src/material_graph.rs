//! Node-based material authoring — the "Materials Factory".
//!
//! A small **shader node graph** per material that COMPILES DOWN to the flat renderer parameters the
//! 3D view already consumes (procedural def, roughness, reflect, opacity, normal map…). This mirrors
//! how Blender works: the node graph (Texture → Principled BSDF → Material Output) is the *authoring*
//! model, and each renderer (EEVEE raster / Cycles path-trace) *evaluates* the same graph its own
//! way. Here the graph is authored in the editor, then `compile()` produces a [`CompiledMaterial`]
//! that both our real-time raster shader and (later) the CPU path tracer read.
//!
//! The graph is transient authoring state: it is SEEDED from an existing [`crate::factory::TextureAsset`]
//! (`from_texture`) and compiled back onto it, so nothing new has to be persisted — the material's
//! flat fields already round-trip through the project file.

use crate::factory::{ProcDef, ProcPattern};

pub type NodeId = u32;

/// A socket's data type — governs which outputs may feed which inputs, and the wire colour.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum SockTy {
    Color,
    Float,
    Vector,
    Shader,
}

impl SockTy {
    /// Wire / socket-dot colour (RGB 0..255).
    pub fn color(self) -> [u8; 3] {
        match self {
            SockTy::Color => [0xC8, 0xC8, 0x3A],  // yellow
            SockTy::Float => [0xA1, 0xA1, 0xA1],  // grey
            SockTy::Vector => [0x63, 0x63, 0xC8], // blue
            SockTy::Shader => [0x63, 0xC8, 0x83], // green
        }
    }
}

/// The Principled BSDF parameters — the Blender master shader, our subset. Each field is the socket's
/// value when NOTHING is wired into it; a connected node overrides at compile time.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Principled {
    pub base_color: [f32; 3],
    pub metallic: f32,
    pub roughness: f32,
    pub ior: f32,
    pub alpha: f32,
    pub emission: [f32; 3],
    pub emission_strength: f32,
}

impl Default for Principled {
    fn default() -> Self {
        // Blender's Principled BSDF defaults.
        Self { base_color: [0.8, 0.8, 0.8], metallic: 0.0, roughness: 0.5, ior: 1.5, alpha: 1.0, emission: [0.0, 0.0, 0.0], emission_strength: 0.0 }
    }
}

/// The kinds of node the graph supports. Source nodes (RGB/Value/Procedural/Image/NormalMap) have no
/// inputs; `Principled` gathers them; `Output` is the sink.
#[derive(Clone, Debug, PartialEq)]
pub enum NodeKind {
    /// Material Output — one input: Surface (Shader). The graph's sink.
    Output,
    /// Principled BSDF — the master shader. Output: BSDF (Shader).
    Principled(Principled),
    /// A constant colour. Output: Color.
    Rgb([f32; 3]),
    /// A constant scalar. Output: Value (Float).
    Value(f32),
    /// A procedural pattern (wood / marble / noise / checker). Output: Color.
    Procedural(ProcDef),
    /// "Use this material's own bound bitmap." Outputs: Color, Alpha.
    ImageTex,
    /// A tangent-space normal map (references another texture by index). Output: Normal (Vector).
    NormalMap { tex: Option<usize>, strength: f32 },
}

impl NodeKind {
    /// Display title.
    pub fn title(&self) -> &'static str {
        match self {
            NodeKind::Output => "Material Output",
            NodeKind::Principled(_) => "Principled BSDF",
            NodeKind::Rgb(_) => "RGB",
            NodeKind::Value(_) => "Value",
            NodeKind::Procedural(_) => "Procedural Texture",
            NodeKind::ImageTex => "Image Texture",
            NodeKind::NormalMap { .. } => "Normal Map",
        }
    }

    /// Input sockets `(name, type)` in top-to-bottom order.
    pub fn inputs(&self) -> &'static [(&'static str, SockTy)] {
        match self {
            NodeKind::Output => &[("Surface", SockTy::Shader)],
            NodeKind::Principled(_) => &[
                ("Base Color", SockTy::Color),
                ("Metallic", SockTy::Float),
                ("Roughness", SockTy::Float),
                ("IOR", SockTy::Float),
                ("Alpha", SockTy::Float),
                ("Emission", SockTy::Color),
                ("Emission Str", SockTy::Float),
                ("Normal", SockTy::Vector),
            ],
            _ => &[],
        }
    }

    /// Output sockets `(name, type)` in top-to-bottom order.
    pub fn outputs(&self) -> &'static [(&'static str, SockTy)] {
        match self {
            NodeKind::Output => &[],
            NodeKind::Principled(_) => &[("BSDF", SockTy::Shader)],
            NodeKind::Rgb(_) => &[("Color", SockTy::Color)],
            NodeKind::Value(_) => &[("Value", SockTy::Float)],
            NodeKind::Procedural(_) => &[("Color", SockTy::Color)],
            NodeKind::ImageTex => &[("Color", SockTy::Color), ("Alpha", SockTy::Float)],
            NodeKind::NormalMap { .. } => &[("Normal", SockTy::Vector)],
        }
    }
}

/// Principled input socket indices (must match [`NodeKind::inputs`] order for `Principled`).
pub mod pin {
    pub const BASE_COLOR: u8 = 0;
    pub const METALLIC: u8 = 1;
    pub const ROUGHNESS: u8 = 2;
    pub const IOR: u8 = 3;
    pub const ALPHA: u8 = 4;
    pub const EMISSION: u8 = 5;
    pub const EMISSION_STR: u8 = 6;
    pub const NORMAL: u8 = 7;
}

/// One node: identity, kind (carrying its socket defaults), and canvas position for the editor.
#[derive(Clone, Debug, PartialEq)]
pub struct Node {
    pub id: NodeId,
    pub kind: NodeKind,
    pub pos: [f32; 2],
}

/// A wire from a node's OUTPUT socket to another node's INPUT socket.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Edge {
    pub from: NodeId,
    pub from_out: u8,
    pub to: NodeId,
    pub to_in: u8,
}

/// A material's shader graph.
#[derive(Clone, Debug, PartialEq)]
pub struct MaterialGraph {
    pub nodes: Vec<Node>,
    pub edges: Vec<Edge>,
    next_id: NodeId,
}

/// The flat result of compiling a graph — exactly what the renderers need. The raster path uses
/// `proc` / `roughness` / `reflect` / `opacity` / `normal_map`; the path tracer (later) also uses
/// `metallic` / `ior` / `emission`.
#[derive(Clone, Debug, PartialEq)]
pub struct CompiledMaterial {
    pub base_color: [f32; 3],
    pub roughness: f32,
    pub metallic: f32,
    pub ior: f32,
    pub opacity: f32,
    /// How much of the physically correct environment reflection the raster shader keeps. 1.0 —
    /// all of it — for everything a graph compiles; metallic reaches the specular through `f0`,
    /// not through here.
    pub reflect: f32,
    /// `Some` ⇒ the base colour is driven by a procedural (a real pattern, OR a degenerate SOLID
    /// colour — see [`ProcDef::solid`] — so plain colours update live through the shader uniforms).
    /// `None` only when `use_image` is true.
    pub proc: Option<ProcDef>,
    /// True ⇒ the base colour is the material's OWN bound bitmap (keep `rgba`, no proc).
    pub use_image: bool,
    /// Tangent-space normal map, as a texture index.
    pub normal_map: Option<usize>,
    pub emission: [f32; 3],
    pub emission_strength: f32,
}

impl MaterialGraph {
    /// Mint the next node id.
    fn mint(&mut self) -> NodeId {
        let id = self.next_id;
        self.next_id += 1;
        id
    }

    /// Add a node at a canvas position, returning its id.
    pub fn add(&mut self, kind: NodeKind, pos: [f32; 2]) -> NodeId {
        let id = self.mint();
        self.nodes.push(Node { id, kind, pos });
        id
    }

    pub fn node(&self, id: NodeId) -> Option<&Node> {
        self.nodes.iter().find(|n| n.id == id)
    }

    pub fn node_mut(&mut self, id: NodeId) -> Option<&mut Node> {
        self.nodes.iter_mut().find(|n| n.id == id)
    }

    /// The node feeding `(to, to_in)`, if any (last wire wins — one wire per input).
    pub fn source_of(&self, to: NodeId, to_in: u8) -> Option<&Node> {
        self.edges
            .iter()
            .rev()
            .find(|e| e.to == to && e.to_in == to_in)
            .and_then(|e| self.node(e.from))
    }

    /// Connect `from`'s output `from_out` to `to`'s input `to_in`. An input holds ONE wire, so any
    /// existing wire into that input is replaced. Rejects type-mismatched or self / cyclic-to-output
    /// links defensively. Returns whether it connected.
    pub fn connect(&mut self, from: NodeId, from_out: u8, to: NodeId, to_in: u8) -> bool {
        if from == to {
            return false;
        }
        let (Some(src), Some(dst)) = (self.node(from), self.node(to)) else {
            return false;
        };
        let out_ty = src.kind.outputs().get(from_out as usize).map(|s| s.1);
        let in_ty = dst.kind.inputs().get(to_in as usize).map(|s| s.1);
        if out_ty.is_none() || out_ty != in_ty {
            return false; // no such socket, or type mismatch
        }
        self.edges.retain(|e| !(e.to == to && e.to_in == to_in));
        self.edges.push(Edge { from, from_out, to, to_in });
        true
    }

    /// Remove any wire into `(to, to_in)`.
    pub fn disconnect_input(&mut self, to: NodeId, to_in: u8) {
        self.edges.retain(|e| !(e.to == to && e.to_in == to_in));
    }

    /// Delete a node and every wire touching it. The `Output` and the (first) `Principled` are the
    /// graph's spine — deleting them is disallowed so `compile` always has something to chew on.
    pub fn remove(&mut self, id: NodeId) -> bool {
        let is_spine = self
            .node(id)
            .map(|n| matches!(n.kind, NodeKind::Output) || (matches!(n.kind, NodeKind::Principled(_)) && self.principled_id() == Some(id)))
            .unwrap_or(false);
        if is_spine {
            return false;
        }
        self.nodes.retain(|n| n.id != id);
        self.edges.retain(|e| e.from != id && e.to != id);
        true
    }

    /// The Output node's id (the first one).
    pub fn output_id(&self) -> Option<NodeId> {
        self.nodes.iter().find(|n| matches!(n.kind, NodeKind::Output)).map(|n| n.id)
    }

    /// The Principled feeding the Output's Surface, else the first Principled in the graph.
    pub fn principled_id(&self) -> Option<NodeId> {
        if let Some(out) = self.output_id() {
            if let Some(src) = self.source_of(out, 0) {
                if matches!(src.kind, NodeKind::Principled(_)) {
                    return Some(src.id);
                }
            }
        }
        self.nodes.iter().find(|n| matches!(n.kind, NodeKind::Principled(_))).map(|n| n.id)
    }

    /// Flatten the graph to the renderer parameters. Walks Output ← Principled ← source nodes,
    /// taking each Principled input from its wired source (if any) or its stored socket default.
    pub fn compile(&self) -> CompiledMaterial {
        let pid = self.principled_id();
        let pr = pid
            .and_then(|id| self.node(id))
            .and_then(|n| if let NodeKind::Principled(p) = &n.kind { Some(*p) } else { None })
            .unwrap_or_default();

        // Base colour: procedural node → proc; image node → own bitmap; RGB node → its colour; else
        // the Principled's own Base Color default.
        let mut base_color = pr.base_color;
        let mut proc = None;
        let mut use_image = false;
        if let Some(pid) = pid {
            match self.source_of(pid, pin::BASE_COLOR).map(|n| &n.kind) {
                Some(NodeKind::Procedural(def)) => {
                    proc = Some(*def);
                    base_color = def.avg_color();
                }
                Some(NodeKind::ImageTex) => use_image = true,
                Some(NodeKind::Rgb(c)) => base_color = *c,
                _ => {}
            }
        }
        // A plain colour (RGB node, or the Principled's own default) is emitted as a SOLID procedural
        // so it renders through the live per-frame uniforms — an image source is the only exception.
        if proc.is_none() && !use_image {
            proc = Some(ProcDef::solid(base_color));
        }

        // Scalars: a wired Value node overrides the socket default.
        let val_at = |slot: u8| -> Option<f32> {
            pid.and_then(|pid| self.source_of(pid, slot))
                .and_then(|n| if let NodeKind::Value(v) = n.kind { Some(v) } else { None })
        };
        let roughness = val_at(pin::ROUGHNESS).unwrap_or(pr.roughness).clamp(0.0, 1.0);
        let metallic = val_at(pin::METALLIC).unwrap_or(pr.metallic).clamp(0.0, 1.0);
        let ior = val_at(pin::IOR).unwrap_or(pr.ior).clamp(1.0, 4.0);
        let opacity = val_at(pin::ALPHA).unwrap_or(pr.alpha).clamp(0.0, 1.0);
        let emission_strength = val_at(pin::EMISSION_STR).unwrap_or(pr.emission_strength).max(0.0);
        let emission = match pid.and_then(|pid| self.source_of(pid, pin::EMISSION)).map(|n| &n.kind) {
            Some(NodeKind::Rgb(c)) => *c,
            _ => pr.emission,
        };

        // Normal: a Normal Map node feeds a tangent-space map index.
        let normal_map = match pid.and_then(|pid| self.source_of(pid, pin::NORMAL)).map(|n| &n.kind) {
            Some(NodeKind::NormalMap { tex, .. }) => *tex,
            _ => None,
        };

        // Every material reflects its surroundings by the physically correct amount — the shader
        // works that out from `f0` (which carries metallic) and the split-sum BRDF. Deriving a
        // separate `metallic × (1 − 0.6·roughness)` gate here is what left every dielectric a node
        // graph could author — water, glass, polished stone — with no reflection whatsoever.
        let reflect = 1.0;

        CompiledMaterial {
            base_color,
            roughness,
            metallic,
            ior,
            opacity,
            reflect,
            proc,
            use_image,
            normal_map,
            emission,
            emission_strength,
        }
    }
}

/// A canonical starter graph: `Source → Principled → Output`, laid out left-to-right. `source` is the
/// node feeding Base Color (procedural, image, or an RGB swatch). Reused by [`MaterialGraph::from_texture`].
fn spine(principled: Principled, source: NodeKind, source_is_normal_seed: Option<(Option<usize>, f32)>) -> MaterialGraph {
    let mut g = MaterialGraph { nodes: Vec::new(), edges: Vec::new(), next_id: 0 };
    let out = g.add(NodeKind::Output, [520.0, 120.0]);
    let bsdf = g.add(NodeKind::Principled(principled), [250.0, 90.0]);
    g.connect(bsdf, 0, out, 0);
    let src = g.add(source, [10.0, 60.0]);
    // Wire the source into Base Color (Procedural/RGB/Image all output Color at socket 0).
    g.connect(src, 0, bsdf, pin::BASE_COLOR);
    if let Some((tex, strength)) = source_is_normal_seed {
        let nm = g.add(NodeKind::NormalMap { tex, strength }, [10.0, 300.0]);
        g.connect(nm, 0, bsdf, pin::NORMAL);
    }
    g
}

impl MaterialGraph {
    /// Seed a graph from an existing material so the node editor opens showing its current setup:
    /// a Principled fed by the material's procedural pattern, its own image, or a flat colour, plus a
    /// Normal Map node if one is set. Compiling this straight back is a no-op on the material.
    pub fn from_texture(t: &crate::factory::TextureAsset) -> Self {
        let principled = Principled {
            base_color: t.avg,
            metallic: t.metallic,
            roughness: t.roughness,
            ior: t.ior,
            alpha: t.opacity,
            emission: t.emission,
            emission_strength: t.emission_strength,
        };
        let source = match &t.proc {
            Some(def) if def.is_solid() => NodeKind::Rgb(def.col_a), // a solid proc ↔ an RGB node
            Some(def) => NodeKind::Procedural(*def),
            None if t.w > 1 || t.h > 1 => NodeKind::ImageTex, // a real bitmap is bound
            None => NodeKind::Rgb(t.avg),                     // a flat 1×1 swatch → an RGB node
        };
        let nm_seed = t.normal_map.map(|idx| (Some(idx), 1.0));
        spine(principled, source, nm_seed)
    }

    /// A brand-new default material graph (grey Principled fed by an oak procedural), for "New material".
    pub fn new_default() -> Self {
        spine(Principled::default(), NodeKind::Procedural(ProcDef::oak()), None)
    }

    /// Convenience for the palette: the node kinds a user can add (spine nodes excluded).
    pub fn addable() -> Vec<NodeKind> {
        vec![
            NodeKind::Rgb([0.8, 0.4, 0.2]),
            NodeKind::Value(0.5),
            NodeKind::Procedural(ProcDef::oak()),
            NodeKind::ImageTex,
            NodeKind::NormalMap { tex: None, strength: 1.0 },
        ]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn oak() -> ProcDef {
        ProcDef::oak()
    }

    #[test]
    fn default_graph_has_the_spine_and_compiles() {
        let g = MaterialGraph::new_default();
        assert!(g.output_id().is_some(), "has an Output");
        assert!(g.principled_id().is_some(), "has a Principled");
        // Oak procedural feeds Base Color → compile yields a proc + its midpoint colour.
        let c = g.compile();
        assert!(c.proc.is_some(), "procedural source compiles to a proc def");
        assert_eq!(c.base_color, oak().avg_color());
        assert_eq!(c.opacity, 1.0);
        assert!(!c.use_image);
    }

    #[test]
    fn principled_defaults_flow_through_when_nothing_wired() {
        // A lone Principled → Output, no source wired: its socket defaults are the result.
        let mut g = MaterialGraph { nodes: Vec::new(), edges: Vec::new(), next_id: 0 };
        let out = g.add(NodeKind::Output, [0.0, 0.0]);
        let mut p = Principled::default();
        p.roughness = 0.2;
        p.metallic = 1.0;
        p.alpha = 0.5;
        let bsdf = g.add(NodeKind::Principled(p), [0.0, 0.0]);
        g.connect(bsdf, 0, out, 0);
        let c = g.compile();
        assert!((c.roughness - 0.2).abs() < 1e-6);
        assert!((c.metallic - 1.0).abs() < 1e-6);
        assert!((c.opacity - 0.5).abs() < 1e-6);
        assert_eq!(c.reflect, 1.0, "the environment lobe is kept in full; f0 carries metallic");
        // A plain Principled colour compiles to a SOLID procedural (so it updates live).
        assert!(!c.use_image, "no bitmap");
        assert!(c.proc.map(|p| p.is_solid()).unwrap_or(false), "flat colour → solid proc");
        assert_eq!(c.base_color, p.base_color);
    }

    /// The bug this file carried: a DIELECTRIC compiled to `reflect = 0`, because the old mapping
    /// was `metallic × (1 − 0.6·roughness)` and every non-metal has `metallic = 0`. Water at
    /// roughness 0.035 is a mirror; it must not compile to "matte".
    #[test]
    fn a_polished_dielectric_still_reflects() {
        let mut g = MaterialGraph { nodes: Vec::new(), edges: Vec::new(), next_id: 0 };
        let out = g.add(NodeKind::Output, [0.0, 0.0]);
        let mut p = Principled::default();
        p.roughness = 0.035; // pool water
        p.metallic = 0.0;    // …and it is not a metal
        let bsdf = g.add(NodeKind::Principled(p), [0.0, 0.0]);
        g.connect(bsdf, 0, out, 0);
        let c = g.compile();
        assert_eq!(c.metallic, 0.0, "still a dielectric");
        assert_eq!(c.reflect, 1.0, "and it reflects the environment in full");
    }

    #[test]
    fn wired_value_nodes_override_socket_defaults() {
        let mut g = MaterialGraph::new_default();
        let pid = g.principled_id().unwrap();
        let v = g.add(NodeKind::Value(0.05), [0.0, 0.0]);
        assert!(g.connect(v, 0, pid, pin::ROUGHNESS), "Float → Roughness connects");
        assert!((g.compile().roughness - 0.05).abs() < 1e-6);
    }

    #[test]
    fn image_source_keeps_the_bitmap() {
        let mut g = MaterialGraph { nodes: Vec::new(), edges: Vec::new(), next_id: 0 };
        let out = g.add(NodeKind::Output, [0.0, 0.0]);
        let bsdf = g.add(NodeKind::Principled(Principled::default()), [0.0, 0.0]);
        g.connect(bsdf, 0, out, 0);
        let img = g.add(NodeKind::ImageTex, [0.0, 0.0]);
        g.connect(img, 0, bsdf, pin::BASE_COLOR);
        let c = g.compile();
        assert!(c.use_image && c.proc.is_none(), "image node keeps the bound bitmap");
    }

    #[test]
    fn connect_rejects_type_mismatch_and_replaces_one_input() {
        let mut g = MaterialGraph::new_default();
        let pid = g.principled_id().unwrap();
        // A Value (Float) may NOT drive Base Color (Color).
        let v = g.add(NodeKind::Value(0.5), [0.0, 0.0]);
        assert!(!g.connect(v, 0, pid, pin::BASE_COLOR), "Float↛Color rejected");
        // Two RGBs into Base Color: the second replaces the first (one wire per input).
        let a = g.add(NodeKind::Rgb([1.0, 0.0, 0.0]), [0.0, 0.0]);
        let b = g.add(NodeKind::Rgb([0.0, 1.0, 0.0]), [0.0, 0.0]);
        g.connect(a, 0, pid, pin::BASE_COLOR);
        g.connect(b, 0, pid, pin::BASE_COLOR);
        assert_eq!(g.edges.iter().filter(|e| e.to == pid && e.to_in == pin::BASE_COLOR).count(), 1);
        assert_eq!(g.compile().base_color, [0.0, 1.0, 0.0], "the later wire wins");
    }

    #[test]
    fn spine_nodes_cannot_be_deleted() {
        let mut g = MaterialGraph::new_default();
        let out = g.output_id().unwrap();
        let pid = g.principled_id().unwrap();
        assert!(!g.remove(out), "Output is protected");
        assert!(!g.remove(pid), "Principled is protected");
        // But an added helper node can go.
        let v = g.add(NodeKind::Value(0.5), [0.0, 0.0]);
        assert!(g.remove(v));
        assert!(g.node(v).is_none());
    }
}
