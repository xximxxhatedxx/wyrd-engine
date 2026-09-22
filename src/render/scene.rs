//! Unified scene graph primitives shared by CPU and GPU renderers.

use crate::widgets::WidgetId;
use glam::Mat4;
use slotmap::{new_key_type, SlotMap};
use std::collections::HashMap;
use tiny_skia::{Color, IntRect};

new_key_type! {
    pub struct SceneNodeId;
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SceneTransform {
    pub matrix: Mat4,
    pub opacity: f32,
}

impl Default for SceneTransform {
    fn default() -> Self {
        Self {
            matrix: Mat4::IDENTITY,
            opacity: 1.0,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum Fill {
    Solid(Color),
    LinearGradient {
        angle_deg: f32,
        stops: Vec<(f32, Color)>,
    },
    RadialGradient {
        cx: f32,
        cy: f32,
        radius: f32,
        stops: Vec<(f32, Color)>,
    },
}

impl From<Color> for Fill {
    fn from(c: Color) -> Self {
        Fill::Solid(c)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum OutlineStyle {
    #[default]
    Solid,
    Dashed,
    Dotted,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Effect {
    Blur {
        radius: f32,
    },
    DropShadow {
        radius: f32,
        offset_x: f32,
        offset_y: f32,
        color: Color,
    },
    InnerGlow {
        radius: f32,
        color: Color,
    },
    Tint(Color),
}

#[derive(Debug, Clone, PartialEq)]
pub enum SceneNodeKind {
    Root,
    Container,
    Shadow {
        radius: f32,
        opacity: f32,
        offset_x: f32,
        offset_y: f32,
        color: Color,
        corner_radius: f32,
    },
    Rect {
        fill: Option<Fill>,
        radius: f32,
    },
    Outline {
        color: Color,
        width: f32,
        style: OutlineStyle,
        radius: f32,
    },
    Text {
        text: String,
        font_family: String,
        font_size: f32,
        color: Color,
        align: cosmic_text::Align,
    },
    Image {
        path: String,
    },
    Svg {
        path: String,
    },
    ProgressArc {
        cx: f32,
        cy: f32,
        radius: f32,
        stroke_width: f32,
        ratio: f32,
        track_color: Option<Color>,
        outline_color: Option<Color>,
        outline_width: f32,
        arc_color: Option<Color>,
        start_angle_deg: f32,
        is_ccw: bool,
    },
    CustomPath {
        color: Color,
        stroke_width: f32,
        path: tiny_skia::Path,
    },
    Effect(Effect),
}

#[derive(Debug, Clone, PartialEq)]
pub struct SceneNode {
    pub widget_id: Option<WidgetId>,
    pub kind: SceneNodeKind,
    pub bounds: (f32, f32, f32, f32),
    pub parent: Option<SceneNodeId>,
    pub children: Vec<SceneNodeId>,
    pub transform: SceneTransform,
    pub dirty: bool,
    pub clip_children: bool,
}

#[derive(Clone, Debug)]
pub struct SceneGraph {
    nodes: SlotMap<SceneNodeId, SceneNode>,
    root: SceneNodeId,
}

impl Default for SceneGraph {
    fn default() -> Self {
        Self::new()
    }
}

impl SceneGraph {
    pub fn new() -> Self {
        let mut nodes = SlotMap::with_key();
        let root = nodes.insert(SceneNode {
            widget_id: None,
            kind: SceneNodeKind::Root,
            bounds: (0.0, 0.0, 0.0, 0.0),
            parent: None,
            children: Vec::new(),
            transform: SceneTransform::default(),
            dirty: true,
            clip_children: false,
        });
        Self { nodes, root }
    }

    pub fn root(&self) -> SceneNodeId {
        self.root
    }

    pub fn get(&self, id: SceneNodeId) -> Option<&SceneNode> {
        self.nodes.get(id)
    }

    pub fn get_mut(&mut self, id: SceneNodeId) -> Option<&mut SceneNode> {
        self.nodes.get_mut(id)
    }

    pub fn set_clip_children(&mut self, id: SceneNodeId, clip: bool) {
        if let Some(node) = self.nodes.get_mut(id) {
            node.clip_children = clip;
        }
    }

    pub fn nodes(&self) -> &SlotMap<SceneNodeId, SceneNode> {
        &self.nodes
    }

    pub fn insert(
        &mut self,
        parent: SceneNodeId,
        kind: SceneNodeKind,
        bounds: (f32, f32, f32, f32),
        widget_id: Option<WidgetId>,
        transform: SceneTransform,
    ) -> Option<SceneNodeId> {
        if !self.nodes.contains_key(parent) {
            return None;
        }
        let id = self.nodes.insert(SceneNode {
            widget_id,
            kind,
            bounds,
            parent: Some(parent),
            children: Vec::new(),
            transform,
            dirty: true,
            clip_children: false,
        });
        self.nodes.get_mut(parent)?.children.push(id);
        self.mark_dirty(parent);
        Some(id)
    }

    pub fn mark_dirty(&mut self, id: SceneNodeId) {
        let mut current = Some(id);
        while let Some(node_id) = current {
            let Some(node) = self.nodes.get_mut(node_id) else {
                break;
            };
            node.dirty = true;
            current = node.parent;
        }
    }

    pub fn remove_subtree(&mut self, id: SceneNodeId) -> bool {
        if id == self.root || !self.nodes.contains_key(id) {
            return false;
        }
        let children = self.nodes[id].children.clone();
        for child in children {
            self.remove_subtree(child);
        }
        let parent = self.nodes[id].parent;
        self.nodes.remove(id);
        if let Some(parent) = parent {
            if let Some(node) = self.nodes.get_mut(parent) {
                node.children.retain(|child| *child != id);
            }
            self.mark_dirty(parent);
        }
        true
    }

    /// Compares this scene against the previous frame scene.
    /// Unchanged nodes are marked `dirty = false`.
    /// Returns the list of dirty node IDs.
    pub fn diff_against(&mut self, prev: Option<&SceneGraph>) -> Vec<SceneNodeId> {
        let Some(prev_scene) = prev else {
            let all_ids: Vec<SceneNodeId> = self.nodes.keys().collect();
            for node in self.nodes.values_mut() {
                node.dirty = true;
            }
            return all_ids;
        };

        let mut prev_by_widget: HashMap<WidgetId, Vec<&SceneNode>> = HashMap::new();
        let mut prev_anonymous: Vec<&SceneNode> = Vec::new();

        for prev_node in prev_scene.nodes.values() {
            if let Some(wid) = prev_node.widget_id {
                prev_by_widget.entry(wid).or_default().push(prev_node);
            } else {
                prev_anonymous.push(prev_node);
            }
        }

        let mut dirty_ids = Vec::new();

        for (id, node) in &mut self.nodes {
            let mut matched = false;
            if let Some(wid) = node.widget_id {
                if let Some(candidates) = prev_by_widget.get(&wid) {
                    for candidate in candidates {
                        if candidate.kind == node.kind
                            && candidate.bounds == node.bounds
                            && candidate.transform == node.transform
                        {
                            matched = true;
                            break;
                        }
                    }
                }
            } else {
                for candidate in &prev_anonymous {
                    if candidate.kind == node.kind
                        && candidate.bounds == node.bounds
                        && candidate.transform == node.transform
                    {
                        matched = true;
                        break;
                    }
                }
            }

            node.dirty = !matched;
            if node.dirty {
                dirty_ids.push(id);
            }
        }

        dirty_ids
    }

    pub fn dirty_nodes(&self) -> Vec<SceneNodeId> {
        self.nodes
            .iter()
            .filter_map(|(id, node)| if node.dirty { Some(id) } else { None })
            .collect()
    }

    pub fn dirty_rects(&self) -> Vec<IntRect> {
        let mut rects = Vec::new();
        for (_id, node) in &self.nodes {
            if !node.dirty || node.bounds.2 <= 0.0 || node.bounds.3 <= 0.0 {
                continue;
            }
            let (x, y, w, h) = match &node.kind {
                SceneNodeKind::Shadow {
                    radius,
                    offset_x,
                    offset_y,
                    ..
                } => {
                    let blur_pad = (radius * 1.5).ceil() + 4.0;
                    (
                        node.bounds.0 + offset_x - blur_pad,
                        node.bounds.1 + offset_y - blur_pad,
                        node.bounds.2 + blur_pad * 2.0,
                        node.bounds.3 + blur_pad * 2.0,
                    )
                }
                _ => (node.bounds.0, node.bounds.1, node.bounds.2, node.bounds.3),
            };
            if let Some(rect) = IntRect::from_xywh(
                x.floor() as i32,
                y.floor() as i32,
                w.ceil().max(1.0) as u32,
                h.ceil().max(1.0) as u32,
            ) {
                rects.push(rect);
            }
        }
        rects
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dirty_state_bubbles_and_subtree_removes() {
        let mut scene = SceneGraph::new();
        let container = scene
            .insert(
                scene.root(),
                SceneNodeKind::Container,
                (0.0, 0.0, 100.0, 50.0),
                None,
                SceneTransform::default(),
            )
            .unwrap();
        let rect = scene
            .insert(
                container,
                SceneNodeKind::Rect {
                    fill: Some(Fill::Solid(tiny_skia::Color::WHITE)),
                    radius: 4.0,
                },
                (0.0, 0.0, 100.0, 50.0),
                None,
                SceneTransform::default(),
            )
            .unwrap();
        assert!(scene.get(scene.root()).unwrap().dirty);
        assert!(scene.remove_subtree(container));
        assert!(scene.get(rect).is_none());
        assert!(scene.get(container).is_none());
    }

    #[test]
    fn test_scene_graph_diff_identical_scene_not_dirty() {
        let mut prev = SceneGraph::new();
        let _c = prev
            .insert(
                prev.root(),
                SceneNodeKind::Rect {
                    fill: Some(Fill::Solid(tiny_skia::Color::BLACK)),
                    radius: 5.0,
                },
                (10.0, 10.0, 40.0, 40.0),
                None,
                SceneTransform::default(),
            )
            .unwrap();

        let mut current = SceneGraph::new();
        let _c2 = current
            .insert(
                current.root(),
                SceneNodeKind::Rect {
                    fill: Some(Fill::Solid(tiny_skia::Color::BLACK)),
                    radius: 5.0,
                },
                (10.0, 10.0, 40.0, 40.0),
                None,
                SceneTransform::default(),
            )
            .unwrap();

        let dirty = current.diff_against(Some(&prev));
        // Only root may be compared or empty, the rect node itself is not dirty
        assert!(dirty.is_empty() || (dirty.len() == 1 && dirty[0] == current.root()));
    }
}
