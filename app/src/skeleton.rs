//! Where the window's parts sit: rows and columns over the bar, the two rails and the stage. The shell draws whatever skeleton it is given; the default is the window as
//! it has always been.

use gpui_kit::prelude::FluentBuilder;
use gpui_kit::{div, px, AnyElement, Div, IntoElement, ParentElement, StyleRefinement, Styled};

use crate::panes::GAP;
use crate::rail::{Edge, Rail};
use crate::workbench::Workbench;

/// The room between the window's edge and what is inside it.
pub const INSET: f32 = 10.;
/// The room between a bar and what is beside it, less than [`INSET`] so the bar's border reads
/// as the edge.
pub const UNDER_BAR: f32 = 4.;
/// The least room the stage keeps, however wide the rails beside it are.
pub const STAGE_MIN: (f32, f32) = (260., 240.);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Part {
    /// The row of tabs and chips across the window.
    Bar,
    Side,
    Agents,
    /// The views of the tab in front.
    Stage,
}

impl Part {
    /// Whether the part is a row of fixed height, which reaches every window edge it touches.
    pub fn is_bar(self) -> bool {
        self == Part::Bar
    }

    pub fn rail(self) -> Option<Rail> {
        match self {
            Part::Side => Some(Rail::Side),
            Part::Agents => Some(Rail::Agents),
            _ => None,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum Node {
    Part(Part),
    /// Side by side.
    Row(Vec<Node>),
    /// One over another.
    Column(Vec<Node>),
}

impl Node {
    fn parts(&self, found: &mut Vec<Part>) {
        match self {
            Node::Part(part) => found.push(*part),
            Node::Row(nodes) | Node::Column(nodes) => {
                nodes.iter().for_each(|node| node.parts(found))
            }
        }
    }

    fn holds(&self, part: Part) -> bool {
        match self {
            Node::Part(own) => *own == part,
            Node::Row(nodes) | Node::Column(nodes) => nodes.iter().any(|node| node.holds(part)),
        }
    }

    /// Whether a bar reaches this node's `side`, so room owed there passes through to it.
    fn bar_at(&self, side: Side) -> bool {
        let (nodes, along) = match self {
            Node::Part(part) => return part.is_bar(),
            Node::Row(nodes) => (nodes, matches!(side, Side::Left | Side::Right)),
            Node::Column(nodes) => (nodes, matches!(side, Side::Top | Side::Bottom)),
        };
        if !along {
            return nodes.iter().any(|node| node.bar_at(side));
        }
        let end = match side {
            Side::Left | Side::Top => nodes.first(),
            Side::Right | Side::Bottom => nodes.last(),
        };
        end.is_some_and(|node| node.bar_at(side))
    }

    /// This node without what draws nothing now, or nothing at all.
    fn shown(&self, drawn: &impl Fn(Part) -> bool) -> Option<Node> {
        match self {
            Node::Part(part) => drawn(*part).then_some(self.clone()),
            Node::Row(nodes) | Node::Column(nodes) => {
                let kept: Vec<Node> = nodes.iter().filter_map(|node| node.shown(drawn)).collect();
                match (kept.len(), self) {
                    (0, _) => None,
                    (_, Node::Row(_)) => Some(Node::Row(kept)),
                    _ => Some(Node::Column(kept)),
                }
            }
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Side {
    Top,
    Right,
    Bottom,
    Left,
}

/// Room a node owes on each side: the window's inset, or the room under a bar.
#[derive(Clone, Copy, Default)]
struct Owed {
    top: f32,
    right: f32,
    bottom: f32,
    left: f32,
}

impl Owed {
    fn get(&self, side: Side) -> f32 {
        match side {
            Side::Top => self.top,
            Side::Right => self.right,
            Side::Bottom => self.bottom,
            Side::Left => self.left,
        }
    }

    fn set(&mut self, side: Side, room: f32) {
        match side {
            Side::Top => self.top = room,
            Side::Right => self.right = room,
            Side::Bottom => self.bottom = room,
            Side::Left => self.left = room,
        }
    }

    fn pad(&self, element: Div) -> Div {
        element
            .pt(px(self.top))
            .pr(px(self.right))
            .pb(px(self.bottom))
            .pl(px(self.left))
    }
}

/// The window's arrangement, with what drawing and the keys need of it worked out once.
#[derive(Clone, Debug, PartialEq)]
pub struct Skeleton {
    root: Node,
    /// Where the keys stop as they walk the window, in reading order.
    areas: Vec<Option<Rail>>,
    /// Which window edge each rail comes out from while closed.
    peeks: [Edge; 2],
    /// The rails beside the stage, which give way before it does.
    beside_stage: Vec<Rail>,
}

impl Default for Skeleton {
    fn default() -> Self {
        Self::new(Node::Column(vec![
            Node::Part(Part::Bar),
            Node::Row(vec![
                Node::Part(Part::Side),
                Node::Part(Part::Stage),
                Node::Part(Part::Agents),
            ]),
        ]))
    }
}

impl Skeleton {
    pub fn new(root: Node) -> Self {
        let mut parts = Vec::new();
        root.parts(&mut parts);
        let areas = (parts.iter())
            .filter(|part| !part.is_bar())
            .map(|part| part.rail())
            .collect();
        let stage = parts.iter().position(|part| *part == Part::Stage);
        let peek = |rail: Rail, away: Edge| {
            let at = parts.iter().position(|part| part.rail() == Some(rail));
            match (at, stage) {
                (Some(at), Some(stage)) if at < stage => Edge::Left,
                (Some(_), Some(_)) => Edge::Right,
                _ => away,
            }
        };
        let peeks = [
            peek(Rail::Side, Edge::Left),
            peek(Rail::Agents, Edge::Right),
        ];
        let mut beside_stage = Vec::new();
        stage_row(&root, &mut beside_stage);
        Self {
            root,
            areas,
            peeks,
            beside_stage,
        }
    }

    /// The rails and the stage in the order the keys walk them, `None` being the stage.
    pub fn areas(&self) -> impl Iterator<Item = Option<Rail>> + '_ {
        self.areas.iter().copied()
    }

    pub fn peek_edge(&self, rail: Rail) -> Edge {
        self.peeks[rail as usize]
    }
}

/// The rails in the same row as the stage.
fn stage_row(node: &Node, found: &mut Vec<Rail>) {
    match node {
        Node::Part(_) => {}
        Node::Row(nodes) if nodes.contains(&Node::Part(Part::Stage)) => {
            found.extend(nodes.iter().filter_map(|node| match node {
                Node::Part(part) => part.rail(),
                _ => None,
            }))
        }
        Node::Row(nodes) | Node::Column(nodes) => {
            nodes.iter().for_each(|node| stage_row(node, found))
        }
    }
}

impl Workbench {
    /// How wide each rail is drawn: as kept, unless the window is too narrow for the stage to
    /// keep its least room beside them, when each gives way toward its own least width.
    pub(crate) fn fit_rails(&mut self, width: f32, tucks: [f32; 2]) {
        let rails = &mut self.rails;
        rails.drawn = [rails.side_width, rails.tab_width];
        let beside: Vec<Rail> = (self.skeleton.beside_stage.iter().copied())
            .filter(|rail| tucks[*rail as usize] < 1.)
            .collect();
        let taken: f32 = beside
            .iter()
            .map(|rail| rails.drawn[*rail as usize] + GAP)
            .sum();
        let room = width - 2. * INSET - STAGE_MIN.0;
        let over = taken - room;
        let give: f32 = (beside.iter())
            .map(|rail| rails.drawn[*rail as usize] - rail.widths().min)
            .sum();
        if over <= 0. || give <= 0. {
            return;
        }
        for rail in beside {
            let drawn = &mut rails.drawn[rail as usize];
            let share = (*drawn - rail.widths().min) / give;
            *drawn = (*drawn - over * share).max(rail.widths().min).floor();
        }
    }

    /// The window's parts as the skeleton places them, each rail `tucks` of the way away.
    pub(crate) fn draw_skeleton(
        &self,
        tucks: [f32; 2],
        cx: &mut gpui_kit::Context<Self>,
    ) -> AnyElement {
        let drawn = |part: Part| match part {
            Part::Side | Part::Agents => part.rail().is_some_and(|rail| tucks[rail as usize] < 1.),
            Part::Bar | Part::Stage => true,
        };
        let edges = Owed {
            top: INSET,
            right: INSET,
            bottom: INSET,
            left: INSET,
        };
        match self.skeleton.root.shown(&drawn) {
            Some(root) => self.node(&root, edges, None, tucks, cx).size_full(),
            None => div(),
        }
        .into_any_element()
    }

    /// `node`, owing `owed` room on its sides, inside a row when `in_row`, a column when
    /// `Some(false)`, or the window.
    fn node(
        &self,
        node: &Node,
        owed: Owed,
        in_row: Option<bool>,
        tucks: [f32; 2],
        cx: &mut gpui_kit::Context<Self>,
    ) -> Div {
        let grows = node.holds(Part::Stage);
        let sized = |element: Div| match in_row {
            Some(true) if grows => element.flex_1().min_w_0().h_full(),
            Some(true) => element.flex_none().h_full(),
            Some(false) if grows => element.flex_1().min_h_0().w_full(),
            Some(false) => element.flex_none().w_full(),
            None => element,
        };
        let nodes = match node {
            Node::Part(Part::Stage) => {
                return sized(owed.pad(self.part(Part::Stage, tucks, cx)))
                    .min_w(px(STAGE_MIN.0))
                    .min_h(px(STAGE_MIN.1))
            }
            Node::Part(part) => return sized(owed.pad(self.part(*part, tucks, cx))),
            Node::Row(nodes) | Node::Column(nodes) => nodes,
        };
        let row = matches!(node, Node::Row(_));
        let mut taken = Owed::default();
        for side in [Side::Top, Side::Right, Side::Bottom, Side::Left] {
            if !node.bar_at(side) {
                taken.set(side, owed.get(side));
            }
        }
        let passed = |at: usize| {
            let mut passing = Owed::default();
            let last = nodes.len() - 1;
            for side in [Side::Top, Side::Right, Side::Bottom, Side::Left] {
                let first_side = matches!(side, Side::Left | Side::Top);
                let along = row == matches!(side, Side::Left | Side::Right);
                let reaches = !along || if first_side { at == 0 } else { at == last };
                if reaches && taken.get(side) == 0. {
                    passing.set(side, owed.get(side));
                }
            }
            if !row {
                let above = at.checked_sub(1).and_then(|above| nodes.get(above));
                if above.is_some_and(|above| above.bar_at(Side::Bottom)) {
                    passing.top = UNDER_BAR;
                }
                if (nodes.get(at + 1)).is_some_and(|below| below.bar_at(Side::Top)) {
                    passing.bottom = UNDER_BAR;
                }
            }
            passing
        };
        let mut drawn = sized(taken.pad(div().flex().when(!row, |column| column.flex_col())));
        for (at, child) in nodes.iter().enumerate() {
            if at > 0 && self.spaced(&nodes[at - 1], child, row) {
                drawn = drawn.child(match row {
                    true => div().flex_none().w(px(GAP)),
                    false => div().flex_none().h(px(GAP)),
                });
            }
            drawn = drawn.child(self.node(child, passed(at), Some(row), tucks, cx));
        }
        drawn
    }

    /// Whether `before` and `after` need the gap put between them: not beside a bar, which
    /// keeps its own room, nor beside a rail's handle, which is the gap.
    fn spaced(&self, before: &Node, after: &Node, row: bool) -> bool {
        if !row {
            return !before.bar_at(Side::Bottom) && !after.bar_at(Side::Top);
        }
        let handle_after = |node: &Node| match node {
            Node::Part(part) => {
                (part.rail()).map(|rail| self.skeleton.peek_edge(rail) == Edge::Left)
            }
            _ => None,
        };
        handle_after(before) != Some(true) && handle_after(after) != Some(false)
    }

    fn part(&self, part: Part, tucks: [f32; 2], cx: &mut gpui_kit::Context<Self>) -> Div {
        let parts = &self.parts;
        match part {
            Part::Bar => div().child(
                parts.bar.clone().cached(
                    StyleRefinement::default()
                        .w_full()
                        .h(px(crate::tabs::BAR_HEIGHT)),
                ),
            ),
            Part::Stage => div().flex().child(
                parts
                    .stage
                    .clone()
                    .cached(StyleRefinement::default().size_full()),
            ),
            Part::Side | Part::Agents => {
                let rail = part.rail().unwrap_or(Rail::Side);
                let handle_after = self.skeleton.peek_edge(rail) == Edge::Left;
                div()
                    .flex()
                    .child(self.docked_rail(rail, tucks[rail as usize], handle_after, cx))
            }
        }
    }
}
