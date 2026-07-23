use crate::language::mask_comments;
use crate::tokens::raw_identifiers;
use crate::types::FileEntry;
use serde::{Deserialize, Serialize};
use std::cmp::Ordering;
use std::collections::BinaryHeap;
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet, VecDeque};
use std::path::Path;

const MAX_TARGETS_PER_SYMBOL_NAME: usize = 24;
const MAX_REFERENCE_EDGES_PER_SYMBOL: usize = 64;
const MAX_SYMBOLS_FOR_REFERENCE_EDGES: usize = 30_000;
const MAX_NODES_FOR_ITERATIVE_COMMUNITIES: usize = 50_000;
const MAX_EDGES_FOR_ITERATIVE_COMMUNITIES: usize = 150_000;
const MAX_COMMUNITY_ITERATIONS: usize = 8;
const MAX_COMMUNITY_FRACTION: f64 = 0.25;
const MIN_COMMUNITY_SPLIT_SIZE: usize = 25;
const TOP_LEVEL_COMMUNITY_LABEL_DEPTH: usize = 2;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GraphNode {
    pub id: String,
    pub label: String,
    #[serde(rename = "type")]
    pub node_type: String,
    pub file_path: Option<String>,
    pub line_start: Option<usize>,
    pub line_end: Option<usize>,
    pub language: Option<String>,
    pub community: Option<usize>,
    pub confidence: String,
    pub metadata: BTreeMap<String, String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GraphEdge {
    pub source: String,
    pub target: String,
    pub relation: String,
    pub confidence: String,
    pub confidence_score: f32,
    pub weight: f32,
    pub source_file: Option<String>,
    pub source_line: Option<usize>,
    pub merge_count: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GraphCommunity {
    pub id: usize,
    pub label: String,
    pub nodes: Vec<String>,
    pub cohesion: f32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PathStep {
    pub id: String,
    pub label: String,
    #[serde(rename = "type")]
    pub node_type: String,
    pub file_path: Option<String>,
    pub via_relation: Option<String>,
    pub via_direction: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PathResult {
    pub found: bool,
    pub source: Option<String>,
    pub target: Option<String>,
    pub hops: usize,
    pub path: Vec<PathStep>,
    pub message: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct FileGraphCsr {
    pub paths: Vec<String>,
    pub offsets: Vec<u32>,
    pub neighbors: Vec<u32>,
    pub weights: Vec<f32>,
    pub communities: Vec<u32>,
    #[serde(skip)]
    path_to_id: HashMap<String, u32>,
}

#[derive(Clone, Copy, Debug)]
struct WeightedPathState {
    cost: f32,
    node: u32,
}

impl PartialEq for WeightedPathState {
    fn eq(&self, other: &Self) -> bool {
        self.node == other.node && self.cost.to_bits() == other.cost.to_bits()
    }
}

impl Eq for WeightedPathState {}

impl PartialOrd for WeightedPathState {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for WeightedPathState {
    fn cmp(&self, other: &Self) -> Ordering {
        other
            .cost
            .total_cmp(&self.cost)
            .then_with(|| other.node.cmp(&self.node))
    }
}

impl FileGraphCsr {
    fn build(
        files: &BTreeMap<String, FileEntry>,
        nodes: &BTreeMap<String, GraphNode>,
        edges: &[GraphEdge],
    ) -> Self {
        let paths = files.keys().cloned().collect::<Vec<_>>();
        let path_to_id = paths
            .iter()
            .enumerate()
            .map(|(id, path)| (path.clone(), id as u32))
            .collect::<HashMap<_, _>>();
        let mut adjacency = vec![BTreeMap::<u32, f32>::new(); paths.len()];
        for edge in edges {
            let Some(source_path) = nodes
                .get(&edge.source)
                .and_then(|node| node.file_path.as_deref())
            else {
                continue;
            };
            let Some(target_path) = nodes
                .get(&edge.target)
                .and_then(|node| node.file_path.as_deref())
            else {
                continue;
            };
            if source_path == target_path {
                continue;
            }
            let relation_weight = match edge.relation.as_str() {
                "depends_on" => 1.0,
                "references" => 0.35,
                _ => continue,
            };
            let Some(&source) = path_to_id.get(source_path) else {
                continue;
            };
            let Some(&target) = path_to_id.get(target_path) else {
                continue;
            };
            let weight = (edge.weight * edge.confidence_score * relation_weight).max(0.05);
            *adjacency[source as usize].entry(target).or_default() += weight;
            *adjacency[target as usize].entry(source).or_default() += weight;
        }

        let mut offsets = Vec::with_capacity(paths.len() + 1);
        let mut neighbors = Vec::new();
        let mut weights = Vec::new();
        offsets.push(0);
        for row in adjacency {
            for (neighbor, weight) in row {
                neighbors.push(neighbor);
                weights.push(weight.ln_1p().max(0.05));
            }
            offsets.push(neighbors.len() as u32);
        }
        let mut graph = Self {
            paths,
            offsets,
            neighbors,
            weights,
            communities: Vec::new(),
            path_to_id,
        };
        graph.communities = graph.detect_communities(1.0);
        graph
    }

    fn rebuild_runtime_indexes(&mut self) {
        self.path_to_id = self
            .paths
            .iter()
            .enumerate()
            .map(|(id, path)| (path.clone(), id as u32))
            .collect();
        if self.communities.len() != self.paths.len() {
            self.communities = self.detect_communities(1.0);
        }
    }

    pub fn id(&self, path: &str) -> Option<usize> {
        self.path_to_id.get(path).map(|id| *id as usize)
    }

    pub fn degree(&self, id: usize) -> usize {
        let Some((&start, &end)) = self.offsets.get(id).zip(self.offsets.get(id + 1)) else {
            return 0;
        };
        end.saturating_sub(start) as usize
    }

    pub fn community(&self, id: usize) -> Option<usize> {
        self.communities
            .get(id)
            .map(|community| *community as usize)
    }

    pub fn neighbor_ids(&self, id: usize) -> Vec<usize> {
        self.neighbor_range(id)
            .map(|edge_idx| self.neighbors[edge_idx] as usize)
            .collect()
    }

    fn neighbor_range(&self, id: usize) -> std::ops::Range<usize> {
        let start = self.offsets.get(id).copied().unwrap_or(0) as usize;
        let end = self.offsets.get(id + 1).copied().unwrap_or(start as u32) as usize;
        start..end
    }

    fn detect_communities(&self, resolution: f32) -> Vec<u32> {
        let node_count = self.paths.len();
        if node_count == 0 {
            return Vec::new();
        }
        let degree = (0..node_count)
            .map(|id| {
                self.neighbor_range(id)
                    .map(|edge_idx| self.weights[edge_idx])
                    .sum::<f32>()
            })
            .collect::<Vec<_>>();
        let total_weight = degree.iter().sum::<f32>().max(1e-9);
        let mut community = (0..node_count as u32).collect::<Vec<_>>();
        let mut community_weight = degree.clone();
        loop {
            let mut moved = false;
            for node in 0..node_count {
                if degree[node] <= f32::EPSILON {
                    continue;
                }
                let current = community[node] as usize;
                let mut neighboring = BTreeMap::<u32, f32>::new();
                for edge_idx in self.neighbor_range(node) {
                    let target = self.neighbors[edge_idx] as usize;
                    *neighboring.entry(community[target]).or_default() += self.weights[edge_idx];
                }
                community_weight[current] -= degree[node];
                let current_internal = neighboring.get(&(current as u32)).copied().unwrap_or(0.0);
                let mut best = current as u32;
                let mut best_gain = current_internal
                    - resolution * degree[node] * community_weight[current] / total_weight;
                for (candidate, internal_weight) in neighboring {
                    let candidate_id = candidate as usize;
                    let gain = internal_weight
                        - resolution * degree[node] * community_weight[candidate_id] / total_weight;
                    if gain > best_gain + 1e-7
                        || ((gain - best_gain).abs() <= 1e-7 && candidate < best)
                    {
                        best = candidate;
                        best_gain = gain;
                    }
                }
                community[node] = best;
                community_weight[best as usize] += degree[node];
                if best as usize != current {
                    moved = true;
                }
            }
            if !moved {
                break;
            }
        }

        let mut refined = vec![u32::MAX; node_count];
        let mut next_community = 0u32;
        for start in 0..node_count {
            if refined[start] != u32::MAX {
                continue;
            }
            let source_community = community[start];
            refined[start] = next_community;
            let mut queue = VecDeque::from([start]);
            while let Some(node) = queue.pop_front() {
                for edge_idx in self.neighbor_range(node) {
                    let target = self.neighbors[edge_idx] as usize;
                    if refined[target] == u32::MAX && community[target] == source_community {
                        refined[target] = next_community;
                        queue.push_back(target);
                    }
                }
            }
            next_community += 1;
        }
        refined
    }

    pub fn personalized_page_rank(
        &self,
        teleport: &[f32],
        allowed: &[bool],
        damping: f32,
        tolerance: f32,
    ) -> Vec<f32> {
        let node_count = self.paths.len();
        if node_count == 0 || teleport.len() != node_count || allowed.len() != node_count {
            return vec![0.0; node_count];
        }
        let mut teleport = teleport
            .iter()
            .zip(allowed)
            .map(|(value, allowed)| allowed.then_some(value.max(0.0)).unwrap_or(0.0))
            .collect::<Vec<_>>();
        let teleport_sum = teleport.iter().sum::<f32>();
        if teleport_sum > 0.0 {
            for value in &mut teleport {
                *value /= teleport_sum;
            }
        } else {
            let allowed_count = allowed.iter().filter(|value| **value).count();
            if allowed_count == 0 {
                return vec![0.0; node_count];
            }
            let uniform = 1.0 / allowed_count as f32;
            for (value, allowed) in teleport.iter_mut().zip(allowed) {
                if *allowed {
                    *value = uniform;
                }
            }
        }

        let mut strengths = vec![0.0f32; node_count];
        for id in 0..node_count {
            if !allowed[id] {
                continue;
            }
            strengths[id] = self
                .neighbor_range(id)
                .filter(|edge_idx| allowed[self.neighbors[*edge_idx] as usize])
                .map(|edge_idx| self.weights[edge_idx])
                .sum();
        }

        let damping = damping.clamp(0.0, 0.999);
        let tolerance = tolerance.max(f32::EPSILON);
        let mut rank = teleport.clone();
        let mut previous_residual = f32::INFINITY;
        let mut stagnant_steps = 0usize;
        loop {
            let dangling_mass = rank
                .iter()
                .enumerate()
                .filter(|(id, _)| allowed[*id] && strengths[*id] <= f32::EPSILON)
                .map(|(_, value)| *value)
                .sum::<f32>();
            let base_scale = 1.0 - damping + damping * dangling_mass;
            let mut next = teleport
                .iter()
                .map(|value| value * base_scale)
                .collect::<Vec<_>>();
            for source in 0..node_count {
                if !allowed[source] || strengths[source] <= f32::EPSILON {
                    continue;
                }
                let scale = damping * rank[source] / strengths[source];
                for edge_idx in self.neighbor_range(source) {
                    let target = self.neighbors[edge_idx] as usize;
                    if allowed[target] {
                        next[target] += scale * self.weights[edge_idx];
                    }
                }
            }
            let residual = next
                .iter()
                .zip(&rank)
                .map(|(left, right)| (left - right).abs())
                .sum::<f32>();
            rank = next;
            if residual <= tolerance || !residual.is_finite() {
                break;
            }
            if residual >= previous_residual * (1.0 - 1e-5) {
                stagnant_steps += 1;
                if stagnant_steps >= 8 {
                    break;
                }
            } else {
                stagnant_steps = 0;
            }
            previous_residual = residual;
        }
        rank
    }

    pub fn weighted_shortest_path(
        &self,
        source: usize,
        target: usize,
        allowed: &[bool],
    ) -> Vec<usize> {
        let node_count = self.paths.len();
        if source >= node_count
            || target >= node_count
            || allowed.len() != node_count
            || !allowed[source]
            || !allowed[target]
        {
            return Vec::new();
        }
        if source == target {
            return vec![source];
        }
        let mut distance = vec![f32::INFINITY; node_count];
        let mut parent = vec![usize::MAX; node_count];
        let mut queue = BinaryHeap::new();
        distance[source] = 0.0;
        queue.push(WeightedPathState {
            cost: 0.0,
            node: source as u32,
        });
        while let Some(WeightedPathState { cost, node }) = queue.pop() {
            let node = node as usize;
            if cost > distance[node] {
                continue;
            }
            if node == target {
                break;
            }
            for edge_idx in self.neighbor_range(node) {
                let next = self.neighbors[edge_idx] as usize;
                if !allowed[next] {
                    continue;
                }
                let hub_cost = ((self.degree(next) + 1) as f32).ln() * 0.12;
                let next_cost = cost + self.weights[edge_idx].max(0.05).recip() + hub_cost;
                if next_cost < distance[next] {
                    distance[next] = next_cost;
                    parent[next] = node;
                    queue.push(WeightedPathState {
                        cost: next_cost,
                        node: next as u32,
                    });
                }
            }
        }
        if !distance[target].is_finite() {
            return Vec::new();
        }
        let mut path = vec![target];
        let mut current = target;
        while current != source {
            current = parent[current];
            if current == usize::MAX {
                return Vec::new();
            }
            path.push(current);
        }
        path.reverse();
        path
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CodeGraph {
    pub nodes: BTreeMap<String, GraphNode>,
    pub edges: Vec<GraphEdge>,
    pub communities: Vec<GraphCommunity>,
    pub file_graph: FileGraphCsr,
    #[serde(skip)]
    adjacency: HashMap<String, Vec<usize>>,
    #[serde(skip)]
    reverse_adjacency: HashMap<String, Vec<usize>>,
}

impl CodeGraph {
    pub fn build(
        files: &BTreeMap<String, FileEntry>,
        deps_forward: &HashMap<String, Vec<String>>,
    ) -> Self {
        let symbol_count = files.values().map(|file| file.symbols.len()).sum::<usize>();
        let include_symbol_nodes = symbol_count <= MAX_SYMBOLS_FOR_REFERENCE_EDGES;
        let enable_reference_edges = include_symbol_nodes;
        let mut nodes = BTreeMap::new();
        let mut edges = EdgeAccumulator::default();
        let mut symbol_by_name: HashMap<String, Vec<SymbolRef>> = HashMap::new();
        let mut file_symbol_ids: HashMap<String, Vec<String>> = HashMap::new();

        for file in files.values() {
            let file_id = file_node_id(&file.path);
            nodes.insert(file_id.clone(), file_node(file));

            if let Some(namespace) = &file.namespace {
                let namespace_id = namespace_node_id(namespace);
                nodes
                    .entry(namespace_id.clone())
                    .or_insert_with(|| namespace_node(namespace));
                edges.add(
                    &file_id,
                    &namespace_id,
                    "declares_namespace",
                    "EXTRACTED",
                    1.0,
                    1.0,
                    Some(file.path.clone()),
                    Some(1),
                );
            }

            if include_symbol_nodes {
                for symbol in &file.symbols {
                    let symbol_id = symbol_node_id(&file.path, symbol.line_start, &symbol.name);
                    let node = symbol_node(file, symbol, &symbol_id);
                    nodes.insert(symbol_id.clone(), node);
                    file_symbol_ids
                        .entry(file.path.clone())
                        .or_default()
                        .push(symbol_id.clone());
                    symbol_by_name
                        .entry(symbol.name.clone())
                        .or_default()
                        .push(SymbolRef {
                            id: symbol_id.clone(),
                            file_path: file.path.clone(),
                            namespace: file.namespace.clone(),
                        });

                    edges.add(
                        &file_id,
                        &symbol_id,
                        "contains",
                        "EXTRACTED",
                        1.0,
                        1.0,
                        Some(file.path.clone()),
                        Some(symbol.line_start),
                    );

                    if let Some(namespace) = &file.namespace {
                        edges.add(
                            &namespace_node_id(namespace),
                            &symbol_id,
                            "contains",
                            "EXTRACTED",
                            1.0,
                            1.0,
                            Some(file.path.clone()),
                            Some(symbol.line_start),
                        );
                    }
                }
            }
        }

        for (source, targets) in deps_forward {
            let source_id = file_node_id(source);
            for target in targets {
                if files.contains_key(target) {
                    edges.add(
                        &source_id,
                        &file_node_id(target),
                        "depends_on",
                        "EXTRACTED",
                        1.0,
                        1.0,
                        Some(source.clone()),
                        None,
                    );
                }
            }
        }

        if enable_reference_edges {
            add_symbol_reference_edges(files, &symbol_by_name, &file_symbol_ids, &mut edges);
        }

        let graph_edges = edges.into_edges();
        let file_graph = FileGraphCsr::build(files, &nodes, &graph_edges);
        let mut graph = Self {
            nodes,
            edges: graph_edges,
            communities: Vec::new(),
            file_graph,
            adjacency: HashMap::new(),
            reverse_adjacency: HashMap::new(),
        };
        graph.rebuild_adjacency();
        graph.assign_communities();
        graph
    }

    pub fn shortest_path(&self, source: &str, target: &str, max_depth: usize) -> PathResult {
        let Some(source_id) = self.find_best_node(source) else {
            return PathResult {
                found: false,
                source: None,
                target: None,
                hops: 0,
                path: Vec::new(),
                message: Some(format!("source node not found: {source}")),
            };
        };
        let Some(target_id) = self.find_best_node(target) else {
            return PathResult {
                found: false,
                source: Some(source_id),
                target: None,
                hops: 0,
                path: Vec::new(),
                message: Some(format!("target node not found: {target}")),
            };
        };

        if source_id == target_id {
            return PathResult {
                found: true,
                source: Some(source_id.clone()),
                target: Some(target_id),
                hops: 0,
                path: vec![self.path_step(&source_id, None, None)],
                message: None,
            };
        }

        let mut visited = HashSet::from([source_id.clone()]);
        let mut parent: HashMap<String, (String, usize, String)> = HashMap::new();
        let mut queue = VecDeque::from([(source_id.clone(), 0usize)]);

        while let Some((current, depth)) = queue.pop_front() {
            if depth >= max_depth {
                continue;
            }
            for (next, edge_idx, direction) in self.undirected_neighbors(&current) {
                if !visited.insert(next.clone()) {
                    continue;
                }
                parent.insert(next.clone(), (current.clone(), edge_idx, direction));
                if next == target_id {
                    return self.build_path_result(source_id, target_id, parent);
                }
                queue.push_back((next, depth + 1));
            }
        }

        PathResult {
            found: false,
            source: Some(source_id),
            target: Some(target_id),
            hops: 0,
            path: Vec::new(),
            message: Some("no path found within max_depth".to_string()),
        }
    }

    fn rebuild_adjacency(&mut self) {
        self.adjacency.clear();
        self.reverse_adjacency.clear();
        for (idx, edge) in self.edges.iter().enumerate() {
            self.adjacency
                .entry(edge.source.clone())
                .or_default()
                .push(idx);
            self.reverse_adjacency
                .entry(edge.target.clone())
                .or_default()
                .push(idx);
        }
    }

    pub fn rebuild_runtime_indexes(&mut self) {
        self.rebuild_adjacency();
        self.file_graph.rebuild_runtime_indexes();
    }

    fn assign_communities(&mut self) {
        let node_to_community = detect_communities(&self.nodes, &self.edges);
        for (node_id, community_id) in &node_to_community {
            if let Some(node) = self.nodes.get_mut(node_id) {
                node.community = Some(*community_id);
            }
        }

        let mut grouped: BTreeMap<usize, Vec<String>> = BTreeMap::new();
        for node_id in self.nodes.keys() {
            let community_id = node_to_community.get(node_id).copied().unwrap_or(0);
            grouped
                .entry(community_id)
                .or_default()
                .push(node_id.clone());
        }

        let mut internal_edge_counts: HashMap<usize, usize> = HashMap::new();
        for edge in &self.edges {
            let source_community = node_to_community.get(&edge.source).copied();
            let target_community = node_to_community.get(&edge.target).copied();
            if let (Some(source_community), Some(target_community)) =
                (source_community, target_community)
            {
                if source_community == target_community {
                    *internal_edge_counts.entry(source_community).or_default() += 1;
                }
            }
        }

        self.communities = grouped
            .into_iter()
            .map(|(id, nodes)| GraphCommunity {
                id,
                label: community_label(&nodes, &self.nodes, TOP_LEVEL_COMMUNITY_LABEL_DEPTH),
                cohesion: cohesion_from_count(
                    nodes.len(),
                    internal_edge_counts.get(&id).copied().unwrap_or(0),
                ),
                nodes,
            })
            .collect();
        self.communities.sort_by(|a, b| {
            b.nodes
                .len()
                .cmp(&a.nodes.len())
                .then_with(|| a.id.cmp(&b.id))
        });

        let remap = self
            .communities
            .iter()
            .enumerate()
            .map(|(new_id, community)| (community.id, new_id))
            .collect::<HashMap<_, _>>();
        for (new_id, community) in self.communities.iter_mut().enumerate() {
            community.id = new_id;
        }
        for node in self.nodes.values_mut() {
            if let Some(old_id) = node.community {
                node.community = remap.get(&old_id).copied();
            }
        }
    }

    fn degree(&self, node_id: &str) -> usize {
        self.adjacency.get(node_id).map_or(0, Vec::len)
            + self.reverse_adjacency.get(node_id).map_or(0, Vec::len)
    }

    fn find_best_node(&self, term: &str) -> Option<String> {
        if self.nodes.contains_key(term) {
            return Some(term.to_string());
        }
        let normalized = term.to_ascii_lowercase();
        let query_words = raw_identifiers(term)
            .into_iter()
            .map(|word| word.to_ascii_lowercase())
            .collect::<Vec<_>>();
        let mut best: Option<(usize, usize, String)> = None;
        for node in self.nodes.values() {
            let id_lower = node.id.to_ascii_lowercase();
            let label_lower = node.label.to_ascii_lowercase();
            let mut score = 0usize;
            if label_lower == normalized || id_lower == normalized {
                score += 1000;
            }
            if label_lower.contains(&normalized) || id_lower.contains(&normalized) {
                score += 100;
            }
            for word in &query_words {
                if label_lower.contains(word) {
                    score += 10;
                }
                if id_lower.contains(word) {
                    score += 5;
                }
            }
            if score == 0 {
                continue;
            }
            let degree = self.degree(&node.id);
            match &best {
                Some((best_score, best_degree, best_id))
                    if *best_score > score
                        || (*best_score == score
                            && (*best_degree > degree
                                || (*best_degree == degree && best_id <= &node.id))) => {}
                _ => best = Some((score, degree, node.id.clone())),
            }
        }
        best.map(|(_, _, node_id)| node_id)
    }

    fn undirected_neighbors(&self, node_id: &str) -> Vec<(String, usize, String)> {
        let mut out = Vec::new();
        if let Some(edges) = self.adjacency.get(node_id) {
            for edge_idx in edges {
                out.push((
                    self.edges[*edge_idx].target.clone(),
                    *edge_idx,
                    "outgoing".to_string(),
                ));
            }
        }
        if let Some(edges) = self.reverse_adjacency.get(node_id) {
            for edge_idx in edges {
                out.push((
                    self.edges[*edge_idx].source.clone(),
                    *edge_idx,
                    "incoming".to_string(),
                ));
            }
        }
        out.sort_by(|a, b| a.0.cmp(&b.0));
        out
    }

    fn build_path_result(
        &self,
        source_id: String,
        target_id: String,
        parent: HashMap<String, (String, usize, String)>,
    ) -> PathResult {
        let mut ids = vec![target_id.clone()];
        let mut current = target_id.clone();
        while current != source_id {
            let Some((previous, _, _)) = parent.get(&current) else {
                break;
            };
            ids.push(previous.clone());
            current = previous.clone();
        }
        ids.reverse();

        let mut path = Vec::new();
        for (idx, node_id) in ids.iter().enumerate() {
            if idx == 0 {
                path.push(self.path_step(node_id, None, None));
            } else {
                let (_, edge_idx, direction) = parent
                    .get(node_id)
                    .expect("path parent exists for non-root node");
                path.push(self.path_step(
                    node_id,
                    Some(self.edges[*edge_idx].relation.clone()),
                    Some(direction.clone()),
                ));
            }
        }

        PathResult {
            found: true,
            source: Some(source_id),
            target: Some(target_id),
            hops: path.len().saturating_sub(1),
            path,
            message: None,
        }
    }

    fn path_step(
        &self,
        node_id: &str,
        via_relation: Option<String>,
        via_direction: Option<String>,
    ) -> PathStep {
        let node = self.nodes.get(node_id).expect("path node exists");
        PathStep {
            id: node.id.clone(),
            label: node.label.clone(),
            node_type: node.node_type.clone(),
            file_path: node.file_path.clone(),
            via_relation,
            via_direction,
        }
    }
}

#[derive(Default)]
struct EdgeAccumulator {
    edges: HashMap<(String, String, String), GraphEdge>,
}

impl EdgeAccumulator {
    #[allow(clippy::too_many_arguments)]
    fn add(
        &mut self,
        source: &str,
        target: &str,
        relation: &str,
        confidence: &str,
        confidence_score: f32,
        weight: f32,
        source_file: Option<String>,
        source_line: Option<usize>,
    ) {
        if source == target {
            return;
        }
        let key = (source.to_string(), target.to_string(), relation.to_string());
        if let Some(existing) = self.edges.get_mut(&key) {
            existing.weight += weight;
            existing.merge_count += 1;
            if confidence_rank(confidence) > confidence_rank(&existing.confidence) {
                existing.confidence = confidence.to_string();
                existing.confidence_score = confidence_score;
                existing.source_file = source_file;
                existing.source_line = source_line;
            }
            return;
        }
        self.edges.insert(
            key,
            GraphEdge {
                source: source.to_string(),
                target: target.to_string(),
                relation: relation.to_string(),
                confidence: confidence.to_string(),
                confidence_score,
                weight,
                source_file,
                source_line,
                merge_count: 1,
            },
        );
    }

    fn into_edges(self) -> Vec<GraphEdge> {
        let mut edges = self.edges.into_values().collect::<Vec<_>>();
        edges.sort_by(|a, b| {
            a.source
                .cmp(&b.source)
                .then_with(|| a.target.cmp(&b.target))
                .then_with(|| a.relation.cmp(&b.relation))
        });
        edges
    }
}

#[derive(Clone)]
struct SymbolRef {
    id: String,
    file_path: String,
    namespace: Option<String>,
}

fn add_symbol_reference_edges(
    files: &BTreeMap<String, FileEntry>,
    symbol_by_name: &HashMap<String, Vec<SymbolRef>>,
    file_symbol_ids: &HashMap<String, Vec<String>>,
    edges: &mut EdgeAccumulator,
) {
    let symbol_id_set = file_symbol_ids
        .values()
        .flatten()
        .cloned()
        .collect::<HashSet<_>>();
    for file in files.values() {
        let active_content = mask_comments(file.language.as_str(), &file.content);
        let lines = active_content.lines().collect::<Vec<_>>();
        for symbol in &file.symbols {
            let source_id = symbol_node_id(&file.path, symbol.line_start, &symbol.name);
            if !symbol_id_set.contains(&source_id) {
                continue;
            }
            let mut identifiers = BTreeSet::new();
            let start = symbol.line_start.saturating_sub(1).min(lines.len());
            let end = symbol.line_end.min(lines.len());
            for line in &lines[start..end] {
                for ident in raw_identifiers(line) {
                    if should_consider_reference(&ident, &symbol.name) {
                        identifiers.insert(ident);
                    }
                }
            }

            let mut emitted = 0usize;
            for ident in identifiers {
                let Some(candidates) = symbol_by_name.get(&ident) else {
                    continue;
                };
                if candidates.len() > MAX_TARGETS_PER_SYMBOL_NAME {
                    continue;
                }
                let mut ranked = candidates.clone();
                ranked.sort_by_key(|candidate| {
                    (
                        candidate.file_path != file.path,
                        candidate.namespace != file.namespace,
                        candidate.file_path.clone(),
                    )
                });
                for target in ranked {
                    if target.id == source_id {
                        continue;
                    }
                    edges.add(
                        &source_id,
                        &target.id,
                        "references",
                        "INFERRED",
                        0.7,
                        if target.file_path == file.path {
                            0.8
                        } else {
                            0.5
                        },
                        Some(file.path.clone()),
                        Some(symbol.line_start),
                    );
                    emitted += 1;
                    if emitted >= MAX_REFERENCE_EDGES_PER_SYMBOL {
                        break;
                    }
                }
                if emitted >= MAX_REFERENCE_EDGES_PER_SYMBOL {
                    break;
                }
            }
        }
    }
}

fn detect_communities(
    nodes: &BTreeMap<String, GraphNode>,
    edges: &[GraphEdge],
) -> HashMap<String, usize> {
    if nodes.is_empty() {
        return HashMap::new();
    }

    let mut label_to_id = BTreeMap::<String, usize>::new();
    let mut node_to_community = HashMap::<String, usize>::new();
    for node in nodes.values() {
        let label = initial_community_label(node);
        let next_id = label_to_id.len();
        let community_id = *label_to_id.entry(label).or_insert(next_id);
        node_to_community.insert(node.id.clone(), community_id);
    }

    if nodes.len() > MAX_NODES_FOR_ITERATIVE_COMMUNITIES
        || edges.len() > MAX_EDGES_FOR_ITERATIVE_COMMUNITIES
    {
        return split_oversized_communities(nodes, node_to_community);
    }

    let mut adjacency: HashMap<String, HashMap<String, f32>> = HashMap::new();
    for edge in edges {
        let weight = edge.weight.max(0.1);
        adjacency
            .entry(edge.source.clone())
            .or_default()
            .entry(edge.target.clone())
            .and_modify(|value| *value += weight)
            .or_insert(weight);
        adjacency
            .entry(edge.target.clone())
            .or_default()
            .entry(edge.source.clone())
            .and_modify(|value| *value += weight)
            .or_insert(weight);
    }

    for _ in 0..MAX_COMMUNITY_ITERATIONS {
        let mut changed = false;
        for node_id in nodes.keys() {
            let Some(neighbors) = adjacency.get(node_id) else {
                continue;
            };
            let mut weights = BTreeMap::<usize, f32>::new();
            for (neighbor, weight) in neighbors {
                if let Some(community) = node_to_community.get(neighbor) {
                    *weights.entry(*community).or_default() += weight;
                }
            }
            let Some((&best_community, _)) = weights
                .iter()
                .max_by(|a, b| a.1.total_cmp(b.1).then_with(|| b.0.cmp(a.0)))
            else {
                continue;
            };
            if node_to_community.get(node_id).copied() != Some(best_community) {
                node_to_community.insert(node_id.clone(), best_community);
                changed = true;
            }
        }
        if !changed {
            break;
        }
    }

    split_oversized_communities(nodes, node_to_community)
}

fn split_oversized_communities(
    nodes: &BTreeMap<String, GraphNode>,
    node_to_community: HashMap<String, usize>,
) -> HashMap<String, usize> {
    let mut grouped: BTreeMap<usize, Vec<String>> = BTreeMap::new();
    for (node, community) in node_to_community {
        grouped.entry(community).or_default().push(node);
    }

    let max_size =
        MIN_COMMUNITY_SPLIT_SIZE.max((nodes.len() as f64 * MAX_COMMUNITY_FRACTION) as usize);
    let mut result = HashMap::new();
    let mut next_id = 0usize;

    for (_, members) in grouped {
        if members.len() <= max_size {
            for member in members {
                result.insert(member, next_id);
            }
            next_id += 1;
            continue;
        }

        let mut buckets: BTreeMap<String, Vec<String>> = BTreeMap::new();
        for member in members {
            let key = nodes
                .get(&member)
                .map(split_key_for_node)
                .unwrap_or_else(|| "unknown".to_string());
            buckets.entry(key).or_default().push(member);
        }

        for (_, bucket) in buckets {
            if bucket.len() <= max_size {
                for member in bucket {
                    result.insert(member, next_id);
                }
                next_id += 1;
            } else {
                for chunk in bucket.chunks(max_size) {
                    for member in chunk {
                        result.insert(member.clone(), next_id);
                    }
                    next_id += 1;
                }
            }
        }
    }

    result
}

fn file_node(file: &FileEntry) -> GraphNode {
    let mut metadata = BTreeMap::new();
    metadata.insert("line_count".to_string(), file.line_count.to_string());
    metadata.insert("byte_size".to_string(), file.byte_size.to_string());
    metadata.insert("content_hash".to_string(), file.content_hash.clone());
    GraphNode {
        id: file_node_id(&file.path),
        label: Path::new(&file.path)
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or(&file.path)
            .to_string(),
        node_type: "file".to_string(),
        file_path: Some(file.path.clone()),
        line_start: Some(1),
        line_end: Some(file.line_count.max(1)),
        language: Some(file.language.to_string()),
        community: None,
        confidence: "EXTRACTED".to_string(),
        metadata,
    }
}

fn namespace_node(namespace: &str) -> GraphNode {
    GraphNode {
        id: namespace_node_id(namespace),
        label: namespace.to_string(),
        node_type: "namespace".to_string(),
        file_path: None,
        line_start: None,
        line_end: None,
        language: None,
        community: None,
        confidence: "EXTRACTED".to_string(),
        metadata: BTreeMap::new(),
    }
}

fn symbol_node(file: &FileEntry, symbol: &crate::types::Symbol, id: &str) -> GraphNode {
    let mut metadata = BTreeMap::new();
    metadata.insert("detail".to_string(), symbol.detail.clone());
    if let Some(namespace) = &file.namespace {
        metadata.insert("namespace".to_string(), namespace.clone());
    }
    GraphNode {
        id: id.to_string(),
        label: symbol_label(symbol.kind.as_str(), &symbol.name),
        node_type: symbol.kind.to_string(),
        file_path: Some(file.path.clone()),
        line_start: Some(symbol.line_start),
        line_end: Some(symbol.line_end),
        language: Some(file.language.to_string()),
        community: None,
        confidence: "EXTRACTED".to_string(),
        metadata,
    }
}

fn file_node_id(path: &str) -> String {
    format!("file:{path}")
}

fn namespace_node_id(namespace: &str) -> String {
    format!("namespace:{namespace}")
}

fn symbol_node_id(path: &str, line: usize, name: &str) -> String {
    format!("symbol:{path}:{line}:{name}")
}

fn symbol_label(kind: &str, name: &str) -> String {
    if matches!(kind, "method" | "constructor") {
        format!("{name}()")
    } else {
        name.to_string()
    }
}

fn should_consider_reference(ident: &str, self_name: &str) -> bool {
    let ident_lower = ident.to_ascii_lowercase();
    ident != self_name
        && ident.len() > 2
        && !is_common_code_stop_token(&ident_lower)
        && ident.chars().any(|ch| ch.is_ascii_alphabetic())
}

fn confidence_rank(confidence: &str) -> usize {
    match confidence {
        "EXTRACTED" => 3,
        "INFERRED" => 2,
        "AMBIGUOUS" => 1,
        _ => 0,
    }
}

fn initial_community_label(node: &GraphNode) -> String {
    community_label_for_node(node, TOP_LEVEL_COMMUNITY_LABEL_DEPTH)
}

fn community_label_for_node(node: &GraphNode, path_depth: usize) -> String {
    if let Some(file_path) = &node.file_path {
        let mut parts = file_path
            .split('/')
            .take(path_depth.max(1))
            .collect::<Vec<_>>();
        if parts.is_empty() {
            return file_path.clone();
        }
        if parts.len() == 1 {
            parts.push(&node.node_type);
        }
        return parts.join("/");
    }
    if node.node_type == "namespace" {
        return node
            .label
            .split('.')
            .take(path_depth.max(1))
            .collect::<Vec<_>>()
            .join(".");
    }
    node.node_type.clone()
}

fn split_key_for_node(node: &GraphNode) -> String {
    node.file_path
        .as_deref()
        .map(|path| path.split('/').take(3).collect::<Vec<_>>().join("/"))
        .unwrap_or_else(|| initial_community_label(node))
}

fn community_label(
    nodes: &[String],
    all_nodes: &BTreeMap<String, GraphNode>,
    path_depth: usize,
) -> String {
    let mut labels = BTreeMap::<String, usize>::new();
    for node_id in nodes {
        if let Some(node) = all_nodes.get(node_id) {
            let label = community_label_for_node(node, path_depth);
            *labels.entry(label).or_default() += 1;
        }
    }
    labels
        .into_iter()
        .max_by(|a, b| a.1.cmp(&b.1).then_with(|| b.0.cmp(&a.0)))
        .map(|(label, _)| label)
        .unwrap_or_else(|| "Community".to_string())
}

fn round2(value: f32) -> f32 {
    (value * 100.0).round() / 100.0
}

fn cohesion_from_count(node_count: usize, actual_edges: usize) -> f32 {
    if node_count <= 1 {
        return 1.0;
    }
    let possible = node_count * (node_count - 1) / 2;
    if possible == 0 {
        0.0
    } else {
        round2(actual_edges as f32 / possible as f32)
    }
}

fn is_common_code_stop_token(token: &str) -> bool {
    COMMON_CODE_STOP_TOKENS.contains(&token)
}

const COMMON_CODE_STOP_TOKENS: &[&str] = &[
    "args",
    "async",
    "await",
    "base",
    "break",
    "case",
    "catch",
    "class",
    "const",
    "continue",
    "default",
    "do",
    "else",
    "enum",
    "error",
    "event",
    "export",
    "false",
    "field",
    "file",
    "finally",
    "for",
    "from",
    "function",
    "get",
    "if",
    "impl",
    "import",
    "init",
    "interface",
    "let",
    "macro",
    "method",
    "mod",
    "module",
    "mut",
    "namespace",
    "new",
    "none",
    "null",
    "package",
    "param",
    "params",
    "property",
    "return",
    "self",
    "set",
    "static",
    "struct",
    "super",
    "switch",
    "this",
    "throw",
    "trait",
    "true",
    "try",
    "type",
    "undefined",
    "use",
    "using",
    "var",
    "void",
    "while",
    "yield",
    "value",
];

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{FileEntry, Symbol};

    #[test]
    fn builds_graph_with_path_and_stats() {
        let mut files = BTreeMap::new();
        files.insert(
            "Services/UserService.cs".to_string(),
            FileEntry {
                path: "Services/UserService.cs".to_string(),
                language: "csharp".into(),
                line_count: 6,
                byte_size: 120,
                modified_unix_ms: 0,
                content_hash: "a".to_string(),
                namespace: Some("Game.Services".to_string()),
                imports: vec![],
                symbols: vec![Symbol {
                    name: "UserService".to_string(),
                    kind: "class".into(),
                    line_start: 1,
                    line_end: 2,
                    detail: "class UserService".to_string(),
                }],
                content: "class UserService {\n  UserRepository repo;\n}".to_string(),
            },
        );
        files.insert(
            "Data/UserRepository.cs".to_string(),
            FileEntry {
                path: "Data/UserRepository.cs".to_string(),
                language: "csharp".into(),
                line_count: 3,
                byte_size: 80,
                modified_unix_ms: 0,
                content_hash: "b".to_string(),
                namespace: Some("Game.Data".to_string()),
                imports: vec![],
                symbols: vec![Symbol {
                    name: "UserRepository".to_string(),
                    kind: "class".into(),
                    line_start: 1,
                    line_end: 3,
                    detail: "class UserRepository".to_string(),
                }],
                content: "class UserRepository {}".to_string(),
            },
        );
        let deps = HashMap::from([(
            "Services/UserService.cs".to_string(),
            vec!["Data/UserRepository.cs".to_string()],
        )]);
        let graph = CodeGraph::build(&files, &deps);
        assert!(graph.nodes.len() >= 4);
        assert!(!graph.communities.is_empty());
        let path = graph.shortest_path("UserService", "UserRepository", 5);
        assert!(path.found);
        let service_id = graph.file_graph.id("Services/UserService.cs").unwrap();
        let repository_id = graph.file_graph.id("Data/UserRepository.cs").unwrap();
        let allowed = vec![true; graph.file_graph.paths.len()];
        let mut teleport = vec![0.0; graph.file_graph.paths.len()];
        teleport[service_id] = 1.0;
        let rank = graph
            .file_graph
            .personalized_page_rank(&teleport, &allowed, 0.85, 1e-7);
        assert!(rank[service_id] > rank[repository_id]);
        assert!(rank[repository_id] > 0.0);
        assert_eq!(
            graph
                .file_graph
                .weighted_shortest_path(service_id, repository_id, &allowed),
            vec![service_id, repository_id]
        );
    }
}
