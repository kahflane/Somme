//! A dry run: every step in the order it would run, with the values known
//! before the run filled in. Values from earlier steps show as their names.
//! Nothing runs.

use std::collections::BTreeMap;

use serde_json::{Value, json};

use crate::schema::{Automation, Direction, NodeKind};
use crate::values::{lookup, render_with};

/// One step of a dry run.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DryStep {
    pub id: String,
    pub title: String,
    /// What it would do, values filled in.
    pub does: String,
}

pub fn dry_run(automation: &Automation, inputs: &BTreeMap<String, String>) -> Vec<DryStep> {
    let mut input: serde_json::Map<String, Value> = automation.inputs.iter().map(|i| (i.name.clone(), json!(i.default))).collect();
    input.extend(inputs.iter().map(|(name, value)| (name.clone(), json!(value))));
    let context = json!({ "input": input, "automation": { "name": automation.name } });
    let fill = |text: &str| {
        render_with(text, |path| {
            if path.starts_with("nodes.") || path.starts_with("vault.") {
                Some(format!("\u{2039}{path}\u{203a}"))
            } else {
                lookup(&context, path).map(crate::values::as_text)
            }
        })
    };
    order(automation)
        .into_iter()
        .map(|node| {
            let does = match &node.kind {
                NodeKind::Command { hosts, command, one_by_one, sudo } => {
                    let mut on: Vec<String> = hosts.names.clone();
                    on.extend(hosts.ids.iter().map(|id| format!("server {id}")));
                    if let Some(tag) = hosts.tag.as_ref().filter(|tag| !tag.trim().is_empty()) {
                        on.push(format!("every server tagged {}", tag.trim()));
                    }
                    let how = match (one_by_one, sudo) {
                        (true, true) => " (one at a time, with sudo)",
                        (true, false) => " (one at a time)",
                        (false, true) => " (with sudo)",
                        (false, false) => "",
                    };
                    format!("On {}{how}: {}", if on.is_empty() { "no servers".to_owned() } else { on.join(", ") }, fill(command))
                }
                NodeKind::Local { command, dir } => match dir.as_deref().filter(|dir| !dir.trim().is_empty()) {
                    Some(dir) => format!("On this computer, in {}: {}", fill(dir), fill(command)),
                    None => format!("On this computer: {}", fill(command)),
                },
                NodeKind::If { value, test, against } if test.needs_against() => {
                    format!("If {} {} {}", fill(value), test.words(), fill(against))
                }
                NodeKind::If { value, test, .. } => format!("If {} {}", fill(value), test.words()),
                NodeKind::Wait { seconds } => format!("Wait {seconds} seconds"),
                NodeKind::Notify { title, body } => format!("Notify: {} \u{2014} {}", fill(title), fill(body)),
                NodeKind::Decide { yes_no, about, .. } => format!("Ask Jev, then the LLM: {} About: {}", fill(yes_no), fill(about)),
                NodeKind::Write { prompt } => format!("The LLM writes: {}", fill(prompt)),
                NodeKind::Copy { host, direction, local, remote } => {
                    let server = host.names.first().cloned().or_else(|| host.ids.first().map(|id| format!("server {id}"))).unwrap_or_else(|| "no server".into());
                    match direction {
                        Direction::Upload => format!("Copy {} to {server}:{}", fill(local), fill(remote)),
                        Direction::Download => format!("Copy {server}:{} to {}", fill(remote), fill(local)),
                    }
                }
                NodeKind::Ask { title, message } if message.trim().is_empty() => format!("Ask you: {}", fill(title)),
                NodeKind::Ask { title, message } => format!("Ask you: {} \u{2014} {}", fill(title), fill(message)),
                NodeKind::Http { method, url, .. } => format!("{} {}", method.to_uppercase(), fill(url)),
                NodeKind::Tls { host, port } => format!("Check the certificate of {}:{port}", fill(host)),
                NodeKind::Dns { name, record } => format!("Look up {} records of {}", record.to_uppercase(), fill(name)),
                NodeKind::Agent { goal, hosts, local, computer, apps, .. } => {
                    let mut on: Vec<String> = hosts.names.clone();
                    on.extend(hosts.ids.iter().map(|id| format!("server {id}")));
                    if let Some(tag) = hosts.tag.as_ref().filter(|tag| !tag.trim().is_empty()) {
                        on.push(format!("every server tagged {}", tag.trim()));
                    }
                    if *local {
                        on.push("this computer".into());
                    }
                    if *computer {
                        on.push(format!("the screen in {}", apps.join(", ")));
                    }
                    format!("An AI agent works on {}: {}", if on.is_empty() { "nothing".to_owned() } else { on.join(", ") }, fill(goal))
                }
            };
            DryStep { id: node.id.clone(), title: node.title(), does }
        })
        .collect()
}

/// As plain text, one step per paragraph.
pub fn as_text(steps: &[DryStep]) -> String {
    steps.iter().enumerate().map(|(ix, step)| format!("{}. {} [{}]\n{}", ix + 1, step.title, step.id, step.does)).collect::<Vec<_>>().join("\n\n")
}

/// The steps in an order they could run: each after the steps before it.
fn order(automation: &Automation) -> Vec<&crate::schema::Node> {
    let mut incoming: BTreeMap<&str, usize> = automation.nodes.iter().map(|node| (node.id.as_str(), 0)).collect();
    for edge in &automation.edges {
        if let Some(n) = incoming.get_mut(edge.to.as_str()) {
            *n += 1;
        }
    }
    let mut done: Vec<&crate::schema::Node> = Vec::new();
    while let Some(next) = automation.nodes.iter().find(|node| incoming.get(node.id.as_str()) == Some(&0)) {
        incoming.remove(next.id.as_str());
        for edge in automation.edges.iter().filter(|edge| edge.from == next.id) {
            if let Some(n) = incoming.get_mut(edge.to.as_str()) {
                *n = n.saturating_sub(1);
            }
        }
        done.push(next);
    }
    // Steps in a loop, if any, at the end.
    done.extend(automation.nodes.iter().filter(|node| incoming.contains_key(node.id.as_str())));
    done
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::schema::{Edge, HostPick, Input, Node};

    #[test]
    fn steps_come_in_order_with_values_filled() {
        let mut automation = Automation::new("Deploy");
        automation.inputs.push(Input { name: "branch".into(), label: String::new(), default: "main".into() });
        automation.nodes = vec![
            Node::new("tell", NodeKind::Notify { title: "{{automation.name}}".into(), body: "{{nodes.pull.output}}".into() }),
            Node::new(
                "pull",
                NodeKind::Command {
                    hosts: HostPick { tag: Some("web".into()), ..HostPick::default() },
                    command: "git pull origin {{input.branch}}".into(),
                    one_by_one: true,
                    sudo: false,
                },
            ),
        ];
        automation.edges = vec![Edge::new("pull", "tell")];
        let steps = dry_run(&automation, &BTreeMap::from([("branch".to_owned(), "dev".to_owned())]));
        assert_eq!(steps[0].does, "On every server tagged web (one at a time): git pull origin dev");
        assert_eq!(steps[1].does, "Notify: Deploy \u{2014} \u{2039}nodes.pull.output\u{203a}");
        assert!(as_text(&steps).starts_with("1. Run a command [pull]"));
    }
}
