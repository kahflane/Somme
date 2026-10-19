//! Places steps on the canvas so lines run down the page: each step one row
//! below the lowest step that leads to it, and the steps of a row side by side
//! under the ones they come from. Used by the Tidy button and when the chat
//! builds a flow.

use std::collections::BTreeMap;

use crate::schema::Automation;

/// A step's size on the canvas, at 100%.
pub const STEP_WIDTH: f32 = 240.;
pub const STEP_HEIGHT: f32 = 84.;
const GAP_X: f32 = 40.;
const GAP_Y: f32 = 64.;

/// Moves every step to a tidy place.
pub fn tidy(automation: &mut Automation) {
    let rows = rows(automation);
    let mut column: BTreeMap<String, f32> = BTreeMap::new();
    let mut placed: BTreeMap<String, [f32; 2]> = BTreeMap::new();
    for (depth, row) in rows.iter().enumerate() {
        // Each step goes under the steps it comes from; ties keep the order of
        // the ways out, so `ok` sits left of `failed` and `yes` left of `no`.
        let mut keyed: Vec<(f32, usize, &String)> = row
            .iter()
            .enumerate()
            .map(|(order, id)| {
                let from: Vec<f32> = automation
                    .edges
                    .iter()
                    .filter(|edge| edge.to == *id)
                    .filter_map(|edge| {
                        let port = automation
                            .node(&edge.from)
                            .map_or(0, |node| node.kind.ports().iter().position(|p| *p == edge.port_of(&node.kind)).unwrap_or(0));
                        column.get(&edge.from).map(|x| x + port as f32 * 0.01)
                    })
                    .collect();
                let key = if from.is_empty() { order as f32 } else { from.iter().sum::<f32>() / from.len() as f32 };
                (key, order, id)
            })
            .collect();
        keyed.sort_by(|a, b| a.0.total_cmp(&b.0).then(a.1.cmp(&b.1)));
        let width = keyed.len() as f32;
        for (ix, (_, _, id)) in keyed.into_iter().enumerate() {
            let x = (ix as f32 - (width - 1.) / 2.) * (STEP_WIDTH + GAP_X);
            column.insert(id.clone(), x);
            placed.insert(id.clone(), [x, depth as f32 * (STEP_HEIGHT + GAP_Y)]);
        }
    }
    for node in &mut automation.nodes {
        if let Some(at) = placed.get(&node.id) {
            node.at = *at;
        }
    }
}

/// Where a new step goes: under `after`, or under everything, moved right
/// until it covers no other step.
pub fn place_new(automation: &Automation, after: Option<&str>) -> [f32; 2] {
    let below = |at: [f32; 2]| [at[0], at[1] + STEP_HEIGHT + GAP_Y];
    let mut at = match after.and_then(|id| automation.node(id)) {
        Some(node) => below(node.at),
        None => match automation.nodes.iter().max_by(|a, b| a.at[1].total_cmp(&b.at[1])) {
            Some(lowest) => below(lowest.at),
            None => [0., 0.],
        },
    };
    let covers = |at: [f32; 2]| {
        automation.nodes.iter().any(|node| {
            (node.at[0] - at[0]).abs() < STEP_WIDTH + GAP_X / 2. && (node.at[1] - at[1]).abs() < STEP_HEIGHT + GAP_Y / 2.
        })
    };
    while covers(at) {
        at[0] += STEP_WIDTH + GAP_X;
    }
    at
}

/// The steps by row: a step's row is the longest way to it from a step
/// nothing leads to. Steps caught in a loop go in a last row.
fn rows(automation: &Automation) -> Vec<Vec<String>> {
    let mut incoming: BTreeMap<&str, usize> = automation.nodes.iter().map(|node| (node.id.as_str(), 0)).collect();
    for edge in &automation.edges {
        if let Some(count) = incoming.get_mut(edge.to.as_str()) {
            *count += 1;
        }
    }
    let mut depth: BTreeMap<&str, usize> = BTreeMap::new();
    // Steps in the order they are written, so rows keep the user's order.
    let mut ready: Vec<&str> =
        automation.nodes.iter().map(|node| node.id.as_str()).filter(|id| incoming.get(id) == Some(&0)).collect();
    for id in &ready {
        depth.insert(id, 0);
    }
    let mut ix = 0;
    while ix < ready.len() {
        let id = ready[ix];
        ix += 1;
        let here = depth[id];
        for edge in automation.edges.iter().filter(|edge| edge.from == id) {
            let Some(count) = incoming.get_mut(edge.to.as_str()) else { continue };
            let next = depth.entry(edge.to.as_str()).or_insert(0);
            *next = (*next).max(here + 1);
            *count -= 1;
            if *count == 0 {
                ready.push(edge.to.as_str());
            }
        }
    }
    let last = depth.values().max().map_or(0, |deepest| deepest + 1);
    let mut rows: Vec<Vec<String>> = Vec::new();
    for node in &automation.nodes {
        let row = if ready.contains(&node.id.as_str()) { depth[node.id.as_str()] } else { last };
        if rows.len() <= row {
            rows.resize(row + 1, Vec::new());
        }
        rows[row].push(node.id.clone());
    }
    rows.retain(|row| !row.is_empty());
    rows
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::schema::{Edge, FAILED, Node, NodeKind};

    fn wait(id: &str) -> Node {
        Node::new(id, NodeKind::Wait { seconds: 1 })
    }

    fn local(id: &str) -> Node {
        Node::new(id, NodeKind::Local { command: "true".into(), dir: None })
    }

    #[test]
    fn a_chain_runs_straight_down() {
        let mut flow = Automation::new("chain");
        flow.nodes = vec![wait("a"), wait("b"), wait("c")];
        flow.edges = vec![Edge::new("a", "b"), Edge::new("b", "c")];
        tidy(&mut flow);
        let at: Vec<_> = flow.nodes.iter().map(|node| node.at).collect();
        assert_eq!(at[0][0], at[1][0]);
        assert_eq!(at[1][0], at[2][0]);
        assert!(at[0][1] < at[1][1] && at[1][1] < at[2][1]);
    }

    #[test]
    fn the_failed_way_sits_right_of_the_ok_way() {
        let mut flow = Automation::new("branch");
        flow.nodes = vec![local("check"), wait("alarm"), wait("fine")];
        flow.edges = vec![Edge::on("check", FAILED, "alarm"), Edge::new("check", "fine")];
        tidy(&mut flow);
        let (alarm, fine) = (flow.node("alarm").unwrap().at, flow.node("fine").unwrap().at);
        assert_eq!(alarm[1], fine[1]);
        assert!(fine[0] < alarm[0]);
    }

    #[test]
    fn a_step_goes_below_the_longest_way_to_it() {
        let mut flow = Automation::new("join");
        flow.nodes = vec![wait("a"), wait("b"), wait("end")];
        flow.edges = vec![Edge::new("a", "b"), Edge::new("b", "end"), Edge::new("a", "end")];
        tidy(&mut flow);
        assert!(flow.node("end").unwrap().at[1] > flow.node("b").unwrap().at[1]);
    }

    #[test]
    fn new_steps_never_cover_old_ones() {
        let mut flow = Automation::new("new");
        flow.nodes = vec![wait("a"), wait("b")];
        flow.nodes[1].at = place_new(&flow, Some("a"));
        let next = place_new(&flow, Some("a"));
        assert_ne!(next, flow.nodes[1].at);
        assert_eq!(next[1], flow.nodes[1].at[1]);
    }
}
