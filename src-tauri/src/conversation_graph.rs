use crate::lineage::{Kind, Lineage, Problem};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::path::{Path, PathBuf};

#[derive(Clone)]
pub struct Node {
    pub guest_namespace: Option<String>,
    pub lineage: Lineage,
    pub problem: Option<Problem>,
    pub cwd: PathBuf,
    pub source_root_id: Option<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Status {
    Root,
    Resolved,
    Orphan,
    Invalid,
    #[default]
    Unknown,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Relation {
    pub status: Status,
    pub root_session_id: Option<String>,
}

pub struct Graph {
    nodes: BTreeMap<String, Node>,
    relations: BTreeMap<String, Relation>,
    children: BTreeMap<String, Vec<String>>,
    rejected: HashSet<String>,
}

impl Graph {
    pub fn new(mut nodes: BTreeMap<String, Node>) -> Self {
        let initial: BTreeMap<_, _> = nodes
            .keys()
            .map(|id| (id.clone(), resolve(id, &nodes)))
            .collect();
        let mut rejected = HashSet::new();
        for (id, node) in &nodes {
            let Some(actual) = initial[id].root_session_id.as_ref() else {
                continue;
            };
            let Some(hint) = node.source_root_id.as_ref() else {
                continue;
            };
            if !same_guest(&nodes, id, hint) {
                continue;
            }
            if initial
                .get(hint)
                .and_then(|relation| relation.root_session_id.as_ref())
                .is_some_and(|hint_root| hint_root != actual)
            {
                rejected.insert(id.clone());
            }
        }
        for id in &rejected {
            nodes.get_mut(id).unwrap().problem = Some(Problem::Conflicting);
        }
        let relations: BTreeMap<_, _> = nodes
            .keys()
            .map(|id| (id.clone(), resolve(id, &nodes)))
            .collect();
        let mut children: BTreeMap<String, Vec<String>> = BTreeMap::new();
        for (id, node) in &nodes {
            if !trusted_link(node) || relations[id].status == Status::Invalid {
                continue;
            }
            let Some(parent) = node.lineage.parent_session_id.as_ref() else {
                continue;
            };
            if !same_guest(&nodes, id, parent) {
                continue;
            }
            if relations.get(parent).is_some_and(|relation| {
                matches!(
                    relation.status,
                    Status::Root | Status::Resolved | Status::Orphan | Status::Unknown
                )
            }) {
                children.entry(parent.clone()).or_default().push(id.clone());
            }
        }
        for values in children.values_mut() {
            values.sort_by(|first, second| {
                nodes[first]
                    .lineage
                    .agent_path
                    .cmp(&nodes[second].lineage.agent_path)
                    .then_with(|| first.cmp(second))
            });
        }
        Self {
            nodes,
            relations,
            children,
            rejected,
        }
    }

    pub fn relation(&self, id: &str) -> Relation {
        self.relations.get(id).cloned().unwrap_or_default()
    }

    pub fn problem(&self, id: &str) -> Option<Problem> {
        self.nodes.get(id).and_then(|node| node.problem.clone())
    }

    pub fn resolved_root_cwd(&self, id: &str) -> Option<(String, PathBuf)> {
        let root = self.relations.get(id)?.root_session_id.as_ref()?;
        Some((root.clone(), self.nodes.get(root)?.cwd.clone()))
    }

    pub fn blocked(&self, id: &str) -> bool {
        let mut current = id;
        for _ in 0..=64 {
            if self.rejected.contains(current) {
                return true;
            }
            let Some(parent) = self
                .nodes
                .get(current)
                .and_then(|node| node.lineage.parent_session_id.as_deref())
            else {
                return false;
            };
            if !same_guest(&self.nodes, current, parent) {
                return false;
            }
            current = parent;
        }
        false
    }

    pub fn descendants(&self, id: &str) -> Vec<String> {
        let mut result = Vec::new();
        let mut pending: Vec<_> = self
            .children
            .get(id)
            .into_iter()
            .flatten()
            .rev()
            .cloned()
            .collect();
        let mut seen = HashSet::new();
        while let Some(next) = pending.pop() {
            if !seen.insert(next.clone()) {
                continue;
            }
            pending.extend(
                self.children
                    .get(&next)
                    .into_iter()
                    .flatten()
                    .rev()
                    .cloned(),
            );
            result.push(next);
        }
        result
    }

    pub fn project_members(&self, project: &Path) -> BTreeSet<String> {
        self.nodes
            .keys()
            .filter(|id| self.belongs(id, project))
            .cloned()
            .collect()
    }

    fn belongs(&self, id: &str, project: &Path) -> bool {
        if self.blocked(id) {
            return false;
        }
        let relation = &self.relations[id];
        if let Some(root) = relation.root_session_id.as_ref() {
            return self.nodes[root].cwd.starts_with(project);
        }
        if self.nodes[id].cwd.starts_with(project) {
            return true;
        }
        if !trusted_link(&self.nodes[id]) || relation.status == Status::Invalid {
            return false;
        }
        let mut current = id;
        for _ in 0..64 {
            let Some(parent) = self.nodes[current].lineage.parent_session_id.as_deref() else {
                return false;
            };
            let Some(node) = self.nodes.get(parent) else {
                return false;
            };
            if !same_guest(&self.nodes, current, parent) {
                return false;
            }
            if self.relations[parent].status == Status::Invalid {
                return false;
            }
            if node.cwd.starts_with(project) {
                return true;
            }
            if !trusted_link(node) {
                return false;
            }
            current = parent;
        }
        false
    }
}

fn same_guest(nodes: &BTreeMap<String, Node>, first: &str, second: &str) -> bool {
    nodes
        .get(first)
        .and_then(|node| node.guest_namespace.as_ref())
        .zip(
            nodes
                .get(second)
                .and_then(|node| node.guest_namespace.as_ref()),
        )
        .is_some_and(|(first, second)| first == second)
}

fn trusted_link(node: &Node) -> bool {
    node.lineage.kind == Kind::Subagent
        && node.lineage.lineage_version == 1
        && node.lineage.source_version.as_deref() == Some("0.167.0")
        && matches!(node.problem, None | Some(Problem::MissingSource))
}

fn resolve(id: &str, nodes: &BTreeMap<String, Node>) -> Relation {
    let mut current = id;
    let mut seen = HashSet::new();
    for _ in 0..=64 {
        if !seen.insert(current) {
            return Relation {
                status: Status::Invalid,
                root_session_id: None,
            };
        }
        let Some(node) = nodes.get(current) else {
            return Relation {
                status: Status::Orphan,
                root_session_id: None,
            };
        };
        if !crate::conversations::valid_id(current)
            || (node.lineage.kind == Kind::Primary && node.lineage.parent_session_id.is_some())
        {
            return Relation {
                status: Status::Invalid,
                root_session_id: None,
            };
        }
        if matches!(
            node.problem,
            Some(Problem::Malformed | Problem::Conflicting)
        ) {
            return Relation {
                status: Status::Invalid,
                root_session_id: None,
            };
        }
        if node.lineage.lineage_version != 1
            || node.lineage.source_version.as_deref() != Some("0.167.0")
            || matches!(
                node.problem,
                Some(Problem::UnsupportedSource | Problem::UnsupportedVersion)
            )
        {
            return Relation::default();
        }
        match node.lineage.kind {
            Kind::Unknown => return Relation::default(),
            Kind::Primary => {
                return Relation {
                    status: if current == id {
                        Status::Root
                    } else {
                        Status::Resolved
                    },
                    root_session_id: Some(current.into()),
                }
            }
            Kind::Subagent => {
                let Some(parent) = node
                    .lineage
                    .parent_session_id
                    .as_deref()
                    .filter(|parent| crate::conversations::valid_id(parent))
                else {
                    return Relation::default();
                };
                if !same_guest(nodes, current, parent) {
                    return Relation {
                        status: Status::Orphan,
                        root_session_id: None,
                    };
                }
                current = parent;
            }
        }
    }
    Relation {
        status: Status::Invalid,
        root_session_id: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn id(index: u32) -> String {
        format!("{index:08x}-0000-4000-8000-000000000001")
    }
    fn node(parent: Option<u32>, cwd: &str) -> Node {
        Node {
            guest_namespace: Some("verified-test-guest".into()),
            lineage: Lineage {
                kind: if parent.is_some() {
                    Kind::Subagent
                } else {
                    Kind::Primary
                },
                parent_session_id: parent.map(id),
                source_version: Some("0.167.0".into()),
                ..Default::default()
            },
            problem: None,
            cwd: cwd.into(),
            source_root_id: None,
        }
    }

    #[test]
    fn late_parent_resolves_and_normal_roots_stay_separate() {
        let mut nodes = BTreeMap::from([(id(2), node(Some(1), "/work/project"))]);
        assert_eq!(
            Graph::new(nodes.clone()).relation(&id(2)).status,
            Status::Orphan
        );
        nodes.insert(id(1), node(None, "/work/project"));
        nodes.insert(id(3), node(None, "/work/project"));
        let graph = Graph::new(nodes);
        assert_eq!(graph.relation(&id(2)).root_session_id, Some(id(1)));
        assert_eq!(graph.descendants(&id(1)), [id(2)]);
        assert!(graph.descendants(&id(3)).is_empty());
    }

    #[test]
    fn project_membership_uses_proven_parent_for_other_cwd_and_excludes_other_roots() {
        let graph = Graph::new(BTreeMap::from([
            (id(1), node(None, "/work/project")),
            (id(2), node(Some(1), "/work/worktree")),
            (id(3), node(None, "/work/other")),
            (id(4), node(Some(3), "/work/project")),
            (id(5), node(None, "/work/project")),
        ]));
        assert_eq!(
            graph.project_members(Path::new("/work/project")),
            BTreeSet::from([id(1), id(2), id(5)])
        );
    }

    #[test]
    fn cycles_self_parent_and_excess_depth_never_form_a_family() {
        let graph = Graph::new(BTreeMap::from([
            (id(1), node(Some(2), "/work/project")),
            (id(2), node(Some(1), "/work/project")),
            (id(3), node(Some(3), "/work/project")),
        ]));
        for index in 1..=3 {
            assert_eq!(graph.relation(&id(index)).status, Status::Invalid);
            assert!(graph.descendants(&id(index)).is_empty());
        }
        let mut nodes = BTreeMap::from([(id(0), node(None, "/work/project"))]);
        for index in 1..=65 {
            nodes.insert(id(index), node(Some(index - 1), "/work/project"));
        }
        let graph = Graph::new(nodes);
        assert_eq!(graph.relation(&id(0)).status, Status::Root);
        assert_eq!(graph.relation(&id(64)).status, Status::Resolved);
        assert_eq!(graph.relation(&id(65)).status, Status::Invalid);
    }
}
