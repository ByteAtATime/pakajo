use std::collections::{HashMap, HashSet, VecDeque};

use super::satisfies::split_dep;

pub struct ProvideEntry {
    pub provider: String,
    pub provide: String,
    pub provider_version: String,
}

pub struct DepGraph {
    nodes: HashSet<String>,
    deps: HashMap<String, HashSet<String>>,
    rdeps: HashMap<String, HashSet<String>>,
    provides: HashMap<String, ProvideEntry>,
}

impl DepGraph {
    pub fn new() -> Self {
        Self {
            nodes: HashSet::new(),
            deps: HashMap::new(),
            rdeps: HashMap::new(),
            provides: HashMap::new(),
        }
    }

    pub fn add_node(&mut self, name: &str) {
        self.nodes.insert(name.to_string());
    }

    pub fn add_provides(&mut self, provide: &str, provider: &str, provider_version: &str) {
        let name = split_dep(provide).name.to_string();
        self.provides.entry(name).or_insert(ProvideEntry {
            provider: provider.to_string(),
            provide: provide.to_string(),
            provider_version: provider_version.to_string(),
        });
    }

    pub fn has_provides(&self, name: &str) -> Option<&ProvideEntry> {
        self.provides.get(name)
    }

    pub fn exists(&self, name: &str) -> bool {
        self.nodes.contains(name)
    }

    pub fn depend_on(&mut self, child: &str, parent: &str) {
        if child == parent {
            return;
        }
        self.add_node(child);
        self.add_node(parent);
        self.deps
            .entry(child.to_string())
            .or_default()
            .insert(parent.to_string());
        self.rdeps
            .entry(parent.to_string())
            .or_default()
            .insert(child.to_string());
    }

    pub fn topo_layers(&self) -> std::result::Result<Vec<Vec<String>>, Vec<String>> {
        let mut indegree: HashMap<String, usize> = self
            .nodes
            .iter()
            .map(|n| (n.clone(), self.deps.get(n).map_or(0, HashSet::len)))
            .collect();
        let mut queue: VecDeque<String> = indegree
            .iter()
            .filter(|&(_, &d)| d == 0)
            .map(|(n, _)| n.clone())
            .collect();
        let mut layers: Vec<Vec<String>> = Vec::new();
        let mut processed = 0usize;
        while !queue.is_empty() {
            let mut layer: Vec<String> = queue.drain(..).collect();
            layer.sort();
            let mut next: VecDeque<String> = VecDeque::new();
            for node in &layer {
                processed += 1;
                if let Some(children) = self.rdeps.get(node) {
                    for child in children {
                        if let Some(d) = indegree.get_mut(child) {
                            *d -= 1;
                            if *d == 0 {
                                next.push_back(child.clone());
                            }
                        }
                    }
                }
            }
            layers.push(layer);
            queue = next;
        }
        if processed != self.nodes.len() {
            let mut leftover: Vec<String> = indegree
                .into_iter()
                .filter(|(_, d)| *d > 0)
                .map(|(n, _)| n)
                .collect();
            leftover.sort();
            return Err(leftover);
        }
        Ok(layers)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn topo_chain() {
        let mut g = DepGraph::new();
        g.depend_on("a", "b");
        g.depend_on("b", "c");
        let layers = g.topo_layers().unwrap();
        assert_eq!(
            layers,
            vec![
                vec!["c".to_string()],
                vec!["b".to_string()],
                vec!["a".to_string()],
            ]
        );
    }

    #[test]
    fn topo_diamond() {
        let mut g = DepGraph::new();
        g.depend_on("a", "b");
        g.depend_on("a", "c");
        g.depend_on("b", "d");
        g.depend_on("c", "d");
        let layers = g.topo_layers().unwrap();
        assert_eq!(layers.len(), 3);
        assert_eq!(layers[0], vec!["d".to_string()]);
        assert_eq!(layers[1], vec!["b".to_string(), "c".to_string()]);
        assert_eq!(layers[2], vec!["a".to_string()]);
    }

    #[test]
    fn topo_cycle() {
        let mut g = DepGraph::new();
        g.depend_on("a", "b");
        g.depend_on("b", "a");
        let err = g.topo_layers().unwrap_err();
        assert!(err.contains(&"a".to_string()));
        assert!(err.contains(&"b".to_string()));
    }
}
