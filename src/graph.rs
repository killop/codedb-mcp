use crate::tokens::{raw_identifiers, split_identifier};
use crate::types::FileEntry;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
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
const LOUVAIN_MAX_ITERATIONS: usize = 20;
const LOUVAIN_RESOLUTION: f64 = 1.0;
const TOP_LEVEL_COMMUNITY_LABEL_DEPTH: usize = 2;
const SUBCOMMUNITY_LABEL_DEPTH: usize = 6;
const SUBCOMMUNITY_MAX_FRACTION: f64 = 0.08;
const MODULE_SUBCOMMUNITY_MAX_FRACTION: f64 = 1.0;
const MAX_TARGET_FILES_PER_SYMBOL_NAME: usize = 48;
const MAX_FILE_REFERENCE_EDGES_PER_FILE: usize = 256;
const MAX_FEATURE_AFFINITY_FILES: usize = 512;
const FEATURE_AFFINITY_WEIGHT: f32 = 1.0;

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
pub struct GraphStats {
    pub nodes: usize,
    pub edges: usize,
    pub communities: usize,
    pub isolated_nodes: usize,
    pub average_degree: f32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GraphAnalysis {
    pub stats: GraphStats,
    pub top_nodes: Vec<NodeDegree>,
    pub relation_counts: Vec<CountItem>,
    pub type_counts: Vec<CountItem>,
    pub surprising_connections: Vec<SurprisingConnection>,
    pub suggested_questions: Vec<SuggestedQuestion>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NodeDegree {
    pub id: String,
    pub label: String,
    #[serde(rename = "type")]
    pub node_type: String,
    pub degree: usize,
    pub file_path: Option<String>,
    pub community: Option<usize>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CountItem {
    pub name: String,
    pub count: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SurprisingConnection {
    pub source: String,
    pub target: String,
    pub relation: String,
    pub confidence: String,
    pub source_files: Vec<String>,
    pub why: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SuggestedQuestion {
    #[serde(rename = "type")]
    pub question_type: String,
    pub question: String,
    pub why: String,
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

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExplainResult {
    pub node: GraphNode,
    pub degree: usize,
    pub incoming: Vec<EdgeNeighbor>,
    pub outgoing: Vec<EdgeNeighbor>,
    pub summary: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EdgeNeighbor {
    pub node_id: String,
    pub label: String,
    #[serde(rename = "type")]
    pub node_type: String,
    pub relation: String,
    pub confidence: String,
    pub file_path: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CodeGraph {
    pub nodes: BTreeMap<String, GraphNode>,
    pub edges: Vec<GraphEdge>,
    pub communities: Vec<GraphCommunity>,
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
        let enable_reference_edges = symbol_count <= MAX_SYMBOLS_FOR_REFERENCE_EDGES;
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

            for symbol in &file.symbols {
                let symbol_id = symbol_node_id(&file.path, symbol.line_start, &symbol.name);
                let node = symbol_node(file, symbol, &symbol_id);
                nodes.insert(symbol_id.clone(), node);
                if enable_reference_edges {
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
                }

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

        let mut graph = Self {
            nodes,
            edges: edges.into_edges(),
            communities: Vec::new(),
            adjacency: HashMap::new(),
            reverse_adjacency: HashMap::new(),
        };
        graph.rebuild_adjacency();
        graph.assign_communities();
        graph
    }

    pub fn stats(&self) -> GraphStats {
        let degree_sum: usize = self.nodes.keys().map(|id| self.degree(id)).sum();
        let isolated_nodes = self
            .nodes
            .keys()
            .filter(|id| self.degree(id.as_str()) == 0)
            .count();
        let average_degree = if self.nodes.is_empty() {
            0.0
        } else {
            degree_sum as f32 / self.nodes.len() as f32
        };
        GraphStats {
            nodes: self.nodes.len(),
            edges: self.edges.len(),
            communities: self.communities.len(),
            isolated_nodes,
            average_degree: round2(average_degree),
        }
    }

    pub fn analysis(&self, top_n: usize) -> GraphAnalysis {
        let top_nodes = self.top_nodes(top_n);
        GraphAnalysis {
            stats: self.stats(),
            top_nodes,
            relation_counts: self.relation_counts(),
            type_counts: self.type_counts(),
            surprising_connections: self.surprising_connections(10),
            suggested_questions: self.suggested_questions(7),
        }
    }

    pub fn limited_json(&self, max_nodes: usize, max_edges: usize) -> Value {
        let nodes = self
            .nodes
            .values()
            .take(max_nodes)
            .collect::<Vec<&GraphNode>>();
        let edges = self
            .edges
            .iter()
            .take(max_edges)
            .collect::<Vec<&GraphEdge>>();
        json!({
            "metadata": {
                "node_count": self.nodes.len(),
                "edge_count": self.edges.len(),
                "community_count": self.communities.len(),
                "truncated": self.nodes.len() > max_nodes || self.edges.len() > max_edges,
                "max_nodes": max_nodes,
                "max_edges": max_edges,
            },
            "nodes": nodes,
            "links": edges,
            "communities": self.communities,
        })
    }

    pub fn explain(&self, term: &str, limit: usize) -> Option<ExplainResult> {
        let node_id = self.find_best_node(term)?;
        let node = self.nodes.get(&node_id)?.clone();
        let mut incoming = Vec::new();
        let mut outgoing = Vec::new();

        for edge_idx in self
            .reverse_adjacency
            .get(&node_id)
            .into_iter()
            .flatten()
            .take(limit)
        {
            let edge = &self.edges[*edge_idx];
            if let Some(other) = self.nodes.get(&edge.source) {
                incoming.push(EdgeNeighbor {
                    node_id: other.id.clone(),
                    label: other.label.clone(),
                    node_type: other.node_type.clone(),
                    relation: edge.relation.clone(),
                    confidence: edge.confidence.clone(),
                    file_path: other.file_path.clone(),
                });
            }
        }

        for edge_idx in self
            .adjacency
            .get(&node_id)
            .into_iter()
            .flatten()
            .take(limit)
        {
            let edge = &self.edges[*edge_idx];
            if let Some(other) = self.nodes.get(&edge.target) {
                outgoing.push(EdgeNeighbor {
                    node_id: other.id.clone(),
                    label: other.label.clone(),
                    node_type: other.node_type.clone(),
                    relation: edge.relation.clone(),
                    confidence: edge.confidence.clone(),
                    file_path: other.file_path.clone(),
                });
            }
        }

        let degree = self.degree(&node_id);
        let summary = summarize_node(&node, incoming.len(), outgoing.len(), degree);
        Some(ExplainResult {
            node,
            degree,
            incoming,
            outgoing,
            summary,
        })
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

    pub fn communities_summary_for(
        &self,
        communities: &[GraphCommunity],
        algorithm: &str,
        community_id: Option<usize>,
        limit: usize,
        community_limit: usize,
        include_members: bool,
    ) -> Value {
        if let Some(id) = community_id {
            let Some(community) = communities.iter().find(|community| community.id == id) else {
                return json!({"error": format!("community not found: {id}")});
            };
            let mut members = community
                .nodes
                .iter()
                .filter_map(|node_id| self.nodes.get(node_id))
                .map(|node| {
                    node_degree_json_for_community(node, self.degree(&node.id), community.id)
                })
                .collect::<Vec<_>>();
            members.sort_by(|a, b| {
                b.get("degree")
                    .and_then(Value::as_u64)
                    .cmp(&a.get("degree").and_then(Value::as_u64))
            });
            members.truncate(limit);
            return json!({
                "algorithm": algorithm,
                "id": community.id,
                "label": community.label,
                "member_count": community.nodes.len(),
                "file_count": file_count_for_nodes(&community.nodes, &self.nodes),
                "cohesion": community.cohesion,
                "members": members,
            });
        }

        let items = communities
            .iter()
            .take(community_limit)
            .map(|community| {
                if !include_members {
                    return json!({
                        "id": community.id,
                        "label": community.label,
                        "member_count": community.nodes.len(),
                        "file_count": file_count_for_nodes(&community.nodes, &self.nodes),
                        "cohesion": community.cohesion,
                    });
                }
                let mut top_members = community
                    .nodes
                    .iter()
                    .filter_map(|node_id| self.nodes.get(node_id))
                    .map(|node| {
                        node_degree_json_for_community(node, self.degree(&node.id), community.id)
                    })
                    .collect::<Vec<_>>();
                top_members.sort_by(|a, b| {
                    b.get("degree")
                        .and_then(Value::as_u64)
                        .cmp(&a.get("degree").and_then(Value::as_u64))
                });
                top_members.truncate(limit.min(10));
                json!({
                    "id": community.id,
                    "label": community.label,
                    "member_count": community.nodes.len(),
                    "file_count": file_count_for_nodes(&community.nodes, &self.nodes),
                    "cohesion": community.cohesion,
                    "top_members": top_members,
                })
            })
            .collect::<Vec<_>>();
        json!({
            "algorithm": algorithm,
            "total_communities": communities.len(),
            "returned_communities": items.len(),
            "community_limit": community_limit,
            "include_members": include_members,
            "communities": items,
        })
    }

    pub fn louvain_communities(&self) -> Vec<GraphCommunity> {
        detect_louvain_communities_with_label_depth(
            &self.nodes,
            &self.edges,
            TOP_LEVEL_COMMUNITY_LABEL_DEPTH,
            MAX_COMMUNITY_FRACTION,
        )
    }

    pub fn louvain_subcommunities(
        &self,
        node_ids: &[String],
        files: &BTreeMap<String, FileEntry>,
        parent_label: Option<&str>,
    ) -> Vec<GraphCommunity> {
        if let Some(communities) =
            self.louvain_file_module_subcommunities(node_ids, files, parent_label)
        {
            return communities;
        }

        let node_id_set = node_ids.iter().map(String::as_str).collect::<HashSet<_>>();
        let sub_nodes = node_ids
            .iter()
            .filter_map(|node_id| {
                self.nodes
                    .get(node_id)
                    .map(|node| (node_id.clone(), node.clone()))
            })
            .collect::<BTreeMap<_, _>>();
        let sub_edges = self
            .edges
            .iter()
            .filter(|edge| {
                node_id_set.contains(edge.source.as_str())
                    && node_id_set.contains(edge.target.as_str())
            })
            .cloned()
            .collect::<Vec<_>>();
        detect_louvain_communities_with_label_depth(
            &sub_nodes,
            &sub_edges,
            SUBCOMMUNITY_LABEL_DEPTH,
            SUBCOMMUNITY_MAX_FRACTION,
        )
    }

    fn louvain_file_module_subcommunities(
        &self,
        node_ids: &[String],
        files: &BTreeMap<String, FileEntry>,
        parent_label: Option<&str>,
    ) -> Option<Vec<GraphCommunity>> {
        let scope_prefix = parent_label.and_then(community_scope_prefix);
        let parent_files = node_ids
            .iter()
            .filter_map(|node_id| self.nodes.get(node_id))
            .filter_map(|node| node.file_path.as_deref())
            .filter(|path| files.contains_key(*path))
            .filter(|path| {
                scope_prefix
                    .as_deref()
                    .is_none_or(|prefix| path_matches_scope(path, prefix))
            })
            .map(str::to_string)
            .collect::<BTreeSet<_>>();
        if parent_files.len() < 2 {
            return None;
        }

        let file_nodes = parent_files
            .iter()
            .filter_map(|path| {
                let id = file_node_id(path);
                self.nodes.get(&id).map(|node| (id, node.clone()))
            })
            .collect::<BTreeMap<_, _>>();
        if file_nodes.len() < 2 {
            return None;
        }

        let module_edges = self.module_projection_edges(files, &parent_files);
        if module_edges.is_empty() {
            return None;
        }

        let mut file_communities = detect_louvain_communities_with_label_depth(
            &file_nodes,
            &module_edges,
            SUBCOMMUNITY_LABEL_DEPTH,
            MODULE_SUBCOMMUNITY_MAX_FRACTION,
        );
        merge_file_communities_by_feature(&mut file_communities);
        if file_communities.len() <= 1 {
            return None;
        }

        let file_community_cohesion =
            module_cohesion_by_file_community(&file_communities, &module_edges);
        let file_community_labels = file_communities
            .iter()
            .map(|community| (community.id, community.label.clone()))
            .collect::<HashMap<_, _>>();
        let mut file_to_community = HashMap::<String, usize>::new();
        for community in &file_communities {
            for file_node in &community.nodes {
                if let Some(node) = file_nodes.get(file_node) {
                    if let Some(file_path) = &node.file_path {
                        file_to_community.insert(file_path.clone(), community.id);
                    }
                }
            }
        }

        let mut grouped = BTreeMap::<usize, Vec<String>>::new();
        for node_id in node_ids {
            let Some(node) = self.nodes.get(node_id) else {
                continue;
            };
            let Some(file_path) = &node.file_path else {
                continue;
            };
            let Some(community_id) = file_to_community.get(file_path).copied() else {
                continue;
            };
            grouped
                .entry(community_id)
                .or_default()
                .push(node_id.clone());
        }
        if grouped.len() <= 1 {
            return None;
        }

        let mut communities = grouped
            .into_iter()
            .map(|(file_community_id, members)| GraphCommunity {
                id: file_community_id,
                label: file_community_labels
                    .get(&file_community_id)
                    .cloned()
                    .unwrap_or_else(|| {
                        community_label(&members, &self.nodes, SUBCOMMUNITY_LABEL_DEPTH)
                    }),
                cohesion: file_community_cohesion
                    .get(&file_community_id)
                    .copied()
                    .unwrap_or(0.0),
                nodes: members,
            })
            .collect::<Vec<_>>();
        disambiguate_duplicate_community_labels(&mut communities, &self.nodes);
        sort_communities_by_size_and_renumber(&mut communities);
        Some(communities)
    }

    fn module_projection_edges(
        &self,
        files: &BTreeMap<String, FileEntry>,
        allowed_files: &BTreeSet<String>,
    ) -> Vec<GraphEdge> {
        let mut edges = build_file_reference_edges(files, allowed_files);
        add_feature_affinity_edges(files, allowed_files, &mut edges);
        self.add_existing_file_projection_edges(allowed_files, &mut edges);
        edges.into_edges()
    }

    fn add_existing_file_projection_edges(
        &self,
        allowed_files: &BTreeSet<String>,
        edges: &mut EdgeAccumulator,
    ) {
        for edge in &self.edges {
            let Some(weight) = module_edge_weight(edge) else {
                continue;
            };
            let Some(source_file) = self
                .nodes
                .get(&edge.source)
                .and_then(|node| node.file_path.as_deref())
            else {
                continue;
            };
            let Some(target_file) = self
                .nodes
                .get(&edge.target)
                .and_then(|node| node.file_path.as_deref())
            else {
                continue;
            };
            if source_file == target_file
                || !allowed_files.contains(source_file)
                || !allowed_files.contains(target_file)
            {
                continue;
            }
            edges.add(
                &file_node_id(source_file),
                &file_node_id(target_file),
                "module_reference",
                &edge.confidence,
                edge.confidence_score,
                weight,
                edge.source_file.clone(),
                edge.source_line,
            );
        }
    }

    pub fn subcommunities_summary_for(
        &self,
        parent: &GraphCommunity,
        subcommunities: &[GraphCommunity],
        child_id: Option<usize>,
        limit: usize,
        community_limit: usize,
        include_members: bool,
    ) -> Value {
        let parent_json = json!({
            "id": parent.id,
            "label": parent.label,
            "member_count": parent.nodes.len(),
            "file_count": file_count_for_nodes(&parent.nodes, &self.nodes),
            "cohesion": parent.cohesion,
        });

        if let Some(id) = child_id {
            let Some(community) = subcommunities.iter().find(|community| community.id == id) else {
                return json!({"error": format!("subcommunity not found: {id}")});
            };
            let mut members = community
                .nodes
                .iter()
                .filter_map(|node_id| self.nodes.get(node_id))
                .map(|node| {
                    node_degree_json_for_community(node, self.degree(&node.id), community.id)
                })
                .collect::<Vec<_>>();
            members.sort_by(|a, b| {
                b.get("degree")
                    .and_then(Value::as_u64)
                    .cmp(&a.get("degree").and_then(Value::as_u64))
            });
            members.truncate(limit);
            return json!({
                "algorithm": "lazy-louvain-subcommunities",
                "parent_community": parent_json,
                "id": community.id,
                "label": community.label,
                "member_count": community.nodes.len(),
                "file_count": file_count_for_nodes(&community.nodes, &self.nodes),
                "cohesion": community.cohesion,
                "members": members,
            });
        }

        let items = subcommunities
            .iter()
            .take(community_limit)
            .map(|community| {
                if !include_members {
                    return json!({
                        "id": community.id,
                        "label": community.label,
                        "member_count": community.nodes.len(),
                        "file_count": file_count_for_nodes(&community.nodes, &self.nodes),
                        "cohesion": community.cohesion,
                    });
                }
                let mut top_members = community
                    .nodes
                    .iter()
                    .filter_map(|node_id| self.nodes.get(node_id))
                    .map(|node| {
                        node_degree_json_for_community(node, self.degree(&node.id), community.id)
                    })
                    .collect::<Vec<_>>();
                top_members.sort_by(|a, b| {
                    b.get("degree")
                        .and_then(Value::as_u64)
                        .cmp(&a.get("degree").and_then(Value::as_u64))
                });
                top_members.truncate(limit.min(10));
                json!({
                    "id": community.id,
                    "label": community.label,
                    "member_count": community.nodes.len(),
                    "file_count": file_count_for_nodes(&community.nodes, &self.nodes),
                    "cohesion": community.cohesion,
                    "top_members": top_members,
                })
            })
            .collect::<Vec<_>>();
        json!({
            "algorithm": "lazy-louvain-subcommunities",
            "parent_community": parent_json,
            "total_subcommunities": subcommunities.len(),
            "returned_subcommunities": items.len(),
            "community_limit": community_limit,
            "include_members": include_members,
            "subcommunities": items,
        })
    }

    pub fn to_graphml(&self, max_nodes: usize, max_edges: usize) -> String {
        let allowed = self
            .nodes
            .keys()
            .take(max_nodes)
            .cloned()
            .collect::<HashSet<_>>();
        let mut out = String::new();
        out.push_str("<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n");
        out.push_str("<graphml xmlns=\"http://graphml.graphdrawing.org/xmlns\">\n");
        out.push_str(
            "  <key id=\"label\" for=\"node\" attr.name=\"label\" attr.type=\"string\"/>\n",
        );
        out.push_str("  <key id=\"type\" for=\"node\" attr.name=\"type\" attr.type=\"string\"/>\n");
        out.push_str(
            "  <key id=\"community\" for=\"node\" attr.name=\"community\" attr.type=\"int\"/>\n",
        );
        out.push_str(
            "  <key id=\"relation\" for=\"edge\" attr.name=\"relation\" attr.type=\"string\"/>\n",
        );
        out.push_str("  <key id=\"confidence\" for=\"edge\" attr.name=\"confidence\" attr.type=\"string\"/>\n");
        out.push_str("  <graph edgedefault=\"directed\">\n");
        for node in self.nodes.values().take(max_nodes) {
            out.push_str(&format!("    <node id=\"{}\">\n", xml_escape(&node.id)));
            out.push_str(&format!(
                "      <data key=\"label\">{}</data>\n",
                xml_escape(&node.label)
            ));
            out.push_str(&format!(
                "      <data key=\"type\">{}</data>\n",
                xml_escape(&node.node_type)
            ));
            if let Some(community) = node.community {
                out.push_str(&format!(
                    "      <data key=\"community\">{community}</data>\n"
                ));
            }
            out.push_str("    </node>\n");
        }
        for (idx, edge) in self
            .edges
            .iter()
            .filter(|edge| allowed.contains(&edge.source) && allowed.contains(&edge.target))
            .take(max_edges)
            .enumerate()
        {
            out.push_str(&format!(
                "    <edge id=\"e{}\" source=\"{}\" target=\"{}\">\n",
                idx,
                xml_escape(&edge.source),
                xml_escape(&edge.target)
            ));
            out.push_str(&format!(
                "      <data key=\"relation\">{}</data>\n",
                xml_escape(&edge.relation)
            ));
            out.push_str(&format!(
                "      <data key=\"confidence\">{}</data>\n",
                xml_escape(&edge.confidence)
            ));
            out.push_str("    </edge>\n");
        }
        out.push_str("  </graph>\n</graphml>\n");
        out
    }

    pub fn to_cypher(&self, max_nodes: usize, max_edges: usize) -> String {
        let mut out = String::new();
        out.push_str("// codebase-mcp graph export\n");
        out.push_str(&format!(
            "// Nodes: {}, Edges: {}, Communities: {}\n\n",
            self.nodes.len(),
            self.edges.len(),
            self.communities.len()
        ));
        let allowed = self
            .nodes
            .keys()
            .take(max_nodes)
            .cloned()
            .collect::<HashSet<_>>();
        for node in self.nodes.values().take(max_nodes) {
            let label = neo4j_label(&node.node_type);
            out.push_str(&format!(
                "MERGE (n:{label} {{id: '{}'}}) SET n.label = '{}', n.type = '{}'",
                cypher_escape(&node.id),
                cypher_escape(&node.label),
                cypher_escape(&node.node_type)
            ));
            if let Some(file_path) = &node.file_path {
                out.push_str(&format!(", n.file_path = '{}'", cypher_escape(file_path)));
            }
            if let Some(community) = node.community {
                out.push_str(&format!(", n.community = {community}"));
            }
            out.push_str(";\n");
        }
        out.push('\n');
        for edge in self
            .edges
            .iter()
            .filter(|edge| allowed.contains(&edge.source) && allowed.contains(&edge.target))
            .take(max_edges)
        {
            out.push_str(&format!(
                "MATCH (a {{id: '{}'}}), (b {{id: '{}'}}) MERGE (a)-[r:{}]->(b) SET r.weight = {:.3}, r.confidence = '{}';\n",
                cypher_escape(&edge.source),
                cypher_escape(&edge.target),
                neo4j_relation(&edge.relation),
                edge.weight,
                cypher_escape(&edge.confidence)
            ));
        }
        out
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

    fn top_nodes(&self, top_n: usize) -> Vec<NodeDegree> {
        let mut nodes = self
            .nodes
            .values()
            .filter(|node| !is_file_node(node))
            .map(|node| NodeDegree {
                id: node.id.clone(),
                label: node.label.clone(),
                node_type: node.node_type.clone(),
                degree: self.degree(&node.id),
                file_path: node.file_path.clone(),
                community: node.community,
            })
            .collect::<Vec<_>>();
        nodes.sort_by(|a, b| b.degree.cmp(&a.degree).then_with(|| a.id.cmp(&b.id)));
        nodes.truncate(top_n);
        nodes
    }

    fn relation_counts(&self) -> Vec<CountItem> {
        let mut counts = BTreeMap::<String, usize>::new();
        for edge in &self.edges {
            *counts.entry(edge.relation.clone()).or_default() += 1;
        }
        sorted_counts(counts)
    }

    fn type_counts(&self) -> Vec<CountItem> {
        let mut counts = BTreeMap::<String, usize>::new();
        for node in self.nodes.values() {
            *counts.entry(node.node_type.clone()).or_default() += 1;
        }
        sorted_counts(counts)
    }

    fn surprising_connections(&self, top_n: usize) -> Vec<SurprisingConnection> {
        let mut candidates = Vec::new();
        for edge in &self.edges {
            if matches!(
                edge.relation.as_str(),
                "contains" | "declares_namespace" | "depends_on"
            ) {
                continue;
            }
            let Some(source) = self.nodes.get(&edge.source) else {
                continue;
            };
            let Some(target) = self.nodes.get(&edge.target) else {
                continue;
            };
            if is_file_node(source) || is_file_node(target) {
                continue;
            }
            let source_file = source.file_path.clone().unwrap_or_default();
            let target_file = target.file_path.clone().unwrap_or_default();
            if source_file.is_empty() || target_file.is_empty() || source_file == target_file {
                continue;
            }

            let mut score = 0usize;
            let mut reasons = Vec::new();
            if edge.confidence == "INFERRED" {
                score += 2;
                reasons.push("inferred from identifier references".to_string());
            } else {
                score += 1;
            }
            if top_level_dir(&source_file) != top_level_dir(&target_file) {
                score += 2;
                reasons.push("crosses top-level directories".to_string());
            }
            if source.community.is_some()
                && target.community.is_some()
                && source.community != target.community
            {
                score += 1;
                reasons.push("bridges separate communities".to_string());
            }
            if self.degree(&source.id).min(self.degree(&target.id)) <= 2
                && self.degree(&source.id).max(self.degree(&target.id)) >= 5
            {
                score += 1;
                reasons.push("connects a peripheral symbol to a hub".to_string());
            }

            candidates.push((
                score,
                SurprisingConnection {
                    source: source.label.clone(),
                    target: target.label.clone(),
                    relation: edge.relation.clone(),
                    confidence: edge.confidence.clone(),
                    source_files: vec![source_file, target_file],
                    why: if reasons.is_empty() {
                        "cross-file graph connection".to_string()
                    } else {
                        reasons.join("; ")
                    },
                },
            ));
        }
        candidates.sort_by(|a, b| b.0.cmp(&a.0));
        candidates
            .into_iter()
            .take(top_n)
            .map(|(_, connection)| connection)
            .collect()
    }

    fn suggested_questions(&self, top_n: usize) -> Vec<SuggestedQuestion> {
        let mut questions = Vec::new();

        for node in self.top_nodes(5) {
            let cross_community = self
                .undirected_neighbors(&node.id)
                .into_iter()
                .filter_map(|(neighbor, _, _)| self.nodes.get(&neighbor))
                .filter(|neighbor| neighbor.community != node.community)
                .count();
            if cross_community >= 2 {
                questions.push(SuggestedQuestion {
                    question_type: "bridge_node".to_string(),
                    question: format!(
                        "Why does `{}` connect multiple code communities?",
                        node.label
                    ),
                    why: format!(
                        "`{}` has degree {} and crosses {} community boundaries.",
                        node.label, node.degree, cross_community
                    ),
                });
            }
        }

        for community in &self.communities {
            if community.nodes.len() >= 20 && community.cohesion < 0.05 {
                questions.push(SuggestedQuestion {
                    question_type: "low_cohesion".to_string(),
                    question: format!(
                        "Should `{}` be split into smaller architectural areas?",
                        community.label
                    ),
                    why: format!(
                        "Community {} has {} nodes but cohesion is only {:.2}.",
                        community.id,
                        community.nodes.len(),
                        community.cohesion
                    ),
                });
            }
        }

        let weak = self
            .nodes
            .values()
            .filter(|node| !is_file_node(node) && self.degree(&node.id) <= 1)
            .take(3)
            .map(|node| format!("`{}`", node.label))
            .collect::<Vec<_>>();
        if !weak.is_empty() {
            questions.push(SuggestedQuestion {
                question_type: "weakly_connected".to_string(),
                question: format!(
                    "What connects {} to the rest of the project?",
                    weak.join(", ")
                ),
                why: "These symbols have one or zero graph connections.".to_string(),
            });
        }

        if questions.is_empty() {
            questions.push(SuggestedQuestion {
                question_type: "no_signal".to_string(),
                question: "What are the highest-degree symbols and why are they central?"
                    .to_string(),
                why: "The graph did not expose clear low-confidence or bridge-node questions."
                    .to_string(),
            });
        }

        questions.truncate(top_n);
        questions
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
        let lines = file.content.lines().collect::<Vec<_>>();
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

fn detect_louvain_communities_with_label_depth(
    nodes: &BTreeMap<String, GraphNode>,
    edges: &[GraphEdge],
    label_depth: usize,
    max_community_fraction: f64,
) -> Vec<GraphCommunity> {
    if nodes.is_empty() {
        return Vec::new();
    }

    let node_ids = nodes.keys().cloned().collect::<Vec<_>>();
    let node_index = node_ids
        .iter()
        .enumerate()
        .map(|(idx, id)| (id.as_str(), idx))
        .collect::<HashMap<_, _>>();
    let mut adjacency = vec![HashMap::<usize, f64>::new(); node_ids.len()];
    let mut node_degree = vec![0.0f64; node_ids.len()];

    for edge in edges {
        let (Some(&source), Some(&target)) = (
            node_index.get(edge.source.as_str()),
            node_index.get(edge.target.as_str()),
        ) else {
            continue;
        };
        if source == target {
            continue;
        }
        let weight = edge.weight.max(0.1) as f64;
        *adjacency[source].entry(target).or_default() += weight;
        *adjacency[target].entry(source).or_default() += weight;
        node_degree[source] += weight;
        node_degree[target] += weight;
    }

    let total_edge_weight = node_degree.iter().sum::<f64>() / 2.0;
    if total_edge_weight <= f64::EPSILON {
        return node_ids
            .into_iter()
            .enumerate()
            .map(|(id, node_id)| GraphCommunity {
                id,
                label: nodes
                    .get(&node_id)
                    .map(initial_community_label)
                    .unwrap_or_else(|| "Community".to_string()),
                nodes: vec![node_id],
                cohesion: 1.0,
            })
            .collect();
    }

    let mut node_to_community = (0..node_ids.len()).collect::<Vec<_>>();
    let mut community_total_degree = node_degree.clone();
    let m2 = 2.0 * total_edge_weight;
    let m2_sq = m2 * m2;
    let mut edges_to_community = HashMap::<usize, f64>::new();

    for _ in 0..LOUVAIN_MAX_ITERATIONS {
        let mut improved = false;
        for node_idx in 0..node_ids.len() {
            let current_community = node_to_community[node_idx];
            let degree = node_degree[node_idx];
            if degree <= f64::EPSILON {
                continue;
            }

            edges_to_community.clear();
            for (&neighbor, &weight) in &adjacency[node_idx] {
                if neighbor == node_idx {
                    continue;
                }
                let neighbor_community = node_to_community[neighbor];
                *edges_to_community.entry(neighbor_community).or_default() += weight;
            }

            let edges_to_current = edges_to_community
                .get(&current_community)
                .copied()
                .unwrap_or(0.0);
            let current_total = community_total_degree[current_community];
            let mut best_community = current_community;
            let mut best_gain = 0.0f64;

            for (&target_community, &edges_to_target) in &edges_to_community {
                if target_community == current_community {
                    continue;
                }
                let target_total = community_total_degree[target_community];
                let gain = LOUVAIN_RESOLUTION
                    * ((edges_to_target - edges_to_current) / m2
                        + (current_total - target_total - degree) * degree / m2_sq);
                if gain > best_gain {
                    best_gain = gain;
                    best_community = target_community;
                }
            }

            if best_community != current_community {
                community_total_degree[current_community] -= degree;
                community_total_degree[best_community] += degree;
                node_to_community[node_idx] = best_community;
                improved = true;
            }
        }
        if !improved {
            break;
        }
    }

    let mut grouped: BTreeMap<usize, Vec<String>> = BTreeMap::new();
    for (node_idx, community_id) in node_to_community.into_iter().enumerate() {
        grouped
            .entry(community_id)
            .or_default()
            .push(node_ids[node_idx].clone());
    }

    let max_size =
        MIN_COMMUNITY_SPLIT_SIZE.max((nodes.len() as f64 * max_community_fraction) as usize);
    let mut final_groups = Vec::<Vec<String>>::new();
    for (_, members) in grouped {
        if members.len() <= max_size {
            final_groups.push(members);
        } else {
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
                    final_groups.push(bucket);
                } else {
                    for chunk in bucket.chunks(max_size) {
                        final_groups.push(chunk.to_vec());
                    }
                }
            }
        }
    }

    final_groups.sort_by(|a, b| b.len().cmp(&a.len()).then_with(|| a.cmp(b)));
    communities_from_groups(nodes, edges, final_groups, label_depth)
}

fn communities_from_groups(
    nodes: &BTreeMap<String, GraphNode>,
    edges: &[GraphEdge],
    groups: Vec<Vec<String>>,
    label_depth: usize,
) -> Vec<GraphCommunity> {
    let node_to_community = groups
        .iter()
        .enumerate()
        .flat_map(|(community_id, members)| {
            members
                .iter()
                .map(move |node_id| (node_id.as_str(), community_id))
        })
        .collect::<HashMap<_, _>>();
    let mut internal_edge_counts = HashMap::<usize, usize>::new();
    for edge in edges {
        let source = node_to_community.get(edge.source.as_str()).copied();
        let target = node_to_community.get(edge.target.as_str()).copied();
        if let (Some(source), Some(target)) = (source, target) {
            if source == target {
                *internal_edge_counts.entry(source).or_default() += 1;
            }
        }
    }

    let mut communities = groups
        .into_iter()
        .enumerate()
        .map(|(id, members)| GraphCommunity {
            id,
            label: community_label(&members, nodes, label_depth),
            cohesion: cohesion_from_count(
                members.len(),
                internal_edge_counts.get(&id).copied().unwrap_or(0),
            ),
            nodes: members,
        })
        .collect::<Vec<_>>();
    disambiguate_duplicate_community_labels(&mut communities, nodes);
    communities
}

fn sort_communities_by_size_and_renumber(communities: &mut [GraphCommunity]) {
    communities.sort_by(|a, b| {
        b.nodes
            .len()
            .cmp(&a.nodes.len())
            .then_with(|| a.label.cmp(&b.label))
    });
    for (id, community) in communities.iter_mut().enumerate() {
        community.id = id;
    }
}

fn merge_file_communities_by_feature(communities: &mut Vec<GraphCommunity>) {
    let mut grouped = BTreeMap::<String, Vec<GraphCommunity>>::new();
    for community in communities.drain(..) {
        let key = feature_key_for_community(&community)
            .map(|feature| format!("feature:{feature}"))
            .unwrap_or_else(|| format!("community:{}", community.id));
        grouped.entry(key).or_default().push(community);
    }

    let mut merged = Vec::new();
    for (key, group) in grouped {
        if group.len() == 1 {
            merged.push(group.into_iter().next().expect("single community exists"));
            continue;
        }

        let feature = key.strip_prefix("feature:").unwrap_or(&key);
        let mut nodes = group
            .into_iter()
            .flat_map(|community| community.nodes)
            .collect::<Vec<_>>();
        nodes.sort();
        nodes.dedup();
        merged.push(GraphCommunity {
            id: merged.len(),
            label: title_feature(feature),
            nodes,
            cohesion: 0.0,
        });
    }

    sort_communities_by_size_and_renumber(&mut merged);
    *communities = merged;
}

fn feature_key_for_community(community: &GraphCommunity) -> Option<String> {
    let suffix = community
        .label
        .split_once(" :: ")
        .map(|(_, suffix)| suffix)
        .unwrap_or(&community.label);
    ordered_feature_tokens_for_path(suffix)
        .into_iter()
        .next()
        .or_else(|| feature_key_for_file_nodes(&community.nodes))
        .or_else(|| {
            ordered_feature_tokens_for_path(&community.label)
                .into_iter()
                .next()
        })
}

fn title_feature(feature: &str) -> String {
    let mut chars = feature.chars();
    let Some(first) = chars.next() else {
        return "Module".to_string();
    };
    format!("{}{}", first.to_ascii_uppercase(), chars.as_str())
}

fn build_file_reference_edges(
    files: &BTreeMap<String, FileEntry>,
    allowed_files: &BTreeSet<String>,
) -> EdgeAccumulator {
    let mut symbol_files = HashMap::<String, BTreeSet<String>>::new();
    for path in allowed_files {
        let Some(file) = files.get(path) else {
            continue;
        };
        for symbol in &file.symbols {
            symbol_files
                .entry(symbol.name.clone())
                .or_default()
                .insert(file.path.clone());
        }
    }

    let mut edges = EdgeAccumulator::default();
    for source_path in allowed_files {
        let Some(file) = files.get(source_path) else {
            continue;
        };
        let identifiers = raw_identifiers(&file.content)
            .into_iter()
            .filter(|ident| should_consider_reference(ident, ""))
            .collect::<BTreeSet<_>>();
        let mut target_weights = BTreeMap::<String, f32>::new();
        for ident in identifiers {
            let Some(targets) = symbol_files.get(&ident) else {
                continue;
            };
            if targets.len() > MAX_TARGET_FILES_PER_SYMBOL_NAME {
                continue;
            }
            for target_path in targets {
                if target_path == source_path {
                    continue;
                }
                *target_weights.entry(target_path.clone()).or_default() += 1.0;
            }
        }

        let mut ranked = target_weights.into_iter().collect::<Vec<_>>();
        ranked.sort_by(|a, b| b.1.total_cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
        for (target_path, weight) in ranked.into_iter().take(MAX_FILE_REFERENCE_EDGES_PER_FILE) {
            edges.add(
                &file_node_id(source_path),
                &file_node_id(&target_path),
                "module_reference",
                "INFERRED",
                0.65,
                weight.clamp(1.0, 50.0),
                Some(source_path.clone()),
                None,
            );
        }
    }
    edges
}

fn add_feature_affinity_edges(
    files: &BTreeMap<String, FileEntry>,
    allowed_files: &BTreeSet<String>,
    edges: &mut EdgeAccumulator,
) {
    let mut token_files = HashMap::<String, BTreeSet<String>>::new();
    for path in allowed_files {
        if !files.contains_key(path) {
            continue;
        }
        for token in feature_tokens_for_path(path) {
            token_files.entry(token).or_default().insert(path.clone());
        }
    }

    for (token, paths) in token_files {
        if paths.len() < 2 || paths.len() > MAX_FEATURE_AFFINITY_FILES {
            continue;
        }
        let paths = paths.into_iter().collect::<Vec<_>>();
        let anchor = &paths[0];
        for path in paths.iter().skip(1) {
            edges.add(
                &file_node_id(anchor),
                &file_node_id(path),
                "feature_affinity",
                "INFERRED",
                0.45,
                FEATURE_AFFINITY_WEIGHT,
                Some(format!("feature:{token}")),
                None,
            );
        }
    }
}

fn feature_tokens_for_path(path: &str) -> BTreeSet<String> {
    ordered_feature_tokens_for_path(path).into_iter().collect()
}

fn ordered_feature_tokens_for_path(path: &str) -> Vec<String> {
    let mut seen = BTreeSet::new();
    let mut tokens = Vec::new();
    path.split(|ch: char| !ch.is_ascii_alphanumeric() && ch != '_')
        .flat_map(feature_tokens_for_part)
        .filter(|token| is_feature_token(token))
        .for_each(|token| {
            if seen.insert(token.clone()) {
                tokens.push(token);
            }
        });
    tokens
}

fn feature_tokens_for_part(part: &str) -> Vec<String> {
    let mut tokens = split_identifier(part);
    if tokens.len() > 1 {
        tokens.remove(0);
    }
    tokens
}

fn feature_key_for_file_nodes(node_ids: &[String]) -> Option<String> {
    let mut scores = BTreeMap::<String, usize>::new();
    for node_id in node_ids {
        let Some(path) = node_id.strip_prefix("file:") else {
            continue;
        };
        score_feature_tokens_for_path(path, &mut scores);
    }
    scores
        .into_iter()
        .max_by(|a, b| a.1.cmp(&b.1).then_with(|| b.0.cmp(&a.0)))
        .map(|(token, _)| token)
}

fn score_feature_tokens_for_path(path: &str, scores: &mut BTreeMap<String, usize>) {
    for (idx, part) in path
        .split(|ch: char| !ch.is_ascii_alphanumeric() && ch != '_')
        .filter(|part| !part.is_empty())
        .enumerate()
    {
        let part_lower = part.to_ascii_lowercase();
        for token in feature_tokens_for_part(part) {
            if !is_feature_token(&token) {
                continue;
            }
            let mut score = 1 + idx;
            if part_lower == token {
                score += 12;
            } else if part_lower.contains(&token) {
                score += 8;
            }
            *scores.entry(token).or_default() += score;
        }
    }
}

fn is_feature_token(token: &str) -> bool {
    token.len() >= 3
        && token.chars().any(|ch| ch.is_ascii_alphabetic())
        && !token.chars().all(|ch| ch.is_ascii_digit())
        && !CS_KEYWORDS.contains(&token)
        && !FEATURE_STOP_TOKENS.contains(&token)
}

fn module_edge_weight(edge: &GraphEdge) -> Option<f32> {
    match edge.relation.as_str() {
        "references_file" | "module_reference" => Some(edge.weight.max(1.0) * 3.0),
        "feature_affinity" => Some(edge.weight.max(0.1)),
        "references" => Some(edge.weight.max(0.1) * 2.0),
        "depends_on" => Some(edge.weight.max(1.0)),
        _ => None,
    }
}

fn module_cohesion_by_file_community(
    communities: &[GraphCommunity],
    edges: &[GraphEdge],
) -> HashMap<usize, f32> {
    let file_to_community = communities
        .iter()
        .flat_map(|community| {
            community
                .nodes
                .iter()
                .map(move |node_id| (node_id.as_str(), community.id))
        })
        .collect::<HashMap<_, _>>();
    let mut internal = HashMap::<usize, f32>::new();
    let mut total = HashMap::<usize, f32>::new();
    for edge in edges {
        let Some(source_community) = file_to_community.get(edge.source.as_str()).copied() else {
            continue;
        };
        let Some(target_community) = file_to_community.get(edge.target.as_str()).copied() else {
            continue;
        };
        let weight = edge.weight.max(0.1);
        if source_community == target_community {
            *internal.entry(source_community).or_default() += weight;
            *total.entry(source_community).or_default() += weight;
        } else {
            *total.entry(source_community).or_default() += weight;
            *total.entry(target_community).or_default() += weight;
        }
    }
    communities
        .iter()
        .map(|community| {
            let inside = internal.get(&community.id).copied().unwrap_or(0.0);
            let all = total.get(&community.id).copied().unwrap_or(0.0);
            let cohesion = if all <= f32::EPSILON {
                0.0
            } else {
                round2((inside / all).clamp(0.0, 1.0))
            };
            (community.id, cohesion)
        })
        .collect()
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
        language: Some(file.language.clone()),
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
        language: Some("csharp".to_string()),
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
        label: symbol_label(&symbol.kind, &symbol.name),
        node_type: symbol.kind.clone(),
        file_path: Some(file.path.clone()),
        line_start: Some(symbol.line_start),
        line_end: Some(symbol.line_end),
        language: Some(file.language.clone()),
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
    ident != self_name
        && ident.len() > 2
        && !CS_KEYWORDS.contains(&ident)
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

fn disambiguate_duplicate_community_labels(
    communities: &mut [GraphCommunity],
    all_nodes: &BTreeMap<String, GraphNode>,
) {
    let mut counts = BTreeMap::<String, usize>::new();
    for community in communities.iter() {
        *counts.entry(community.label.clone()).or_default() += 1;
    }

    for community in communities.iter_mut() {
        if counts.get(&community.label).copied().unwrap_or(0) <= 1 {
            continue;
        }
        if let Some(suffix) = community_label_suffix(&community.label, &community.nodes, all_nodes)
        {
            community.label = format!("{} :: {}", community.label, suffix);
        }
    }
}

fn file_count_for_nodes(nodes: &[String], all_nodes: &BTreeMap<String, GraphNode>) -> usize {
    nodes
        .iter()
        .filter_map(|node_id| all_nodes.get(node_id))
        .filter_map(|node| node.file_path.as_deref())
        .collect::<BTreeSet<_>>()
        .len()
}

fn community_label_suffix(
    label: &str,
    nodes: &[String],
    all_nodes: &BTreeMap<String, GraphNode>,
) -> Option<String> {
    let mut file_counts = BTreeMap::<String, usize>::new();
    for node_id in nodes {
        let Some(node) = all_nodes.get(node_id) else {
            continue;
        };
        if let Some(file_path) = &node.file_path {
            *file_counts.entry(file_path.clone()).or_default() += 1;
        }
    }
    if let Some((file_path, _)) = file_counts
        .into_iter()
        .max_by(|a, b| a.1.cmp(&b.1).then_with(|| b.0.cmp(&a.0)))
    {
        let suffix = path_suffix_for_label(label, &file_path);
        if !suffix.is_empty() {
            return Some(suffix);
        }
    }

    nodes
        .iter()
        .filter_map(|node_id| all_nodes.get(node_id))
        .find(|node| !is_file_node(node))
        .map(|node| node.label.clone())
}

fn community_scope_prefix(label: &str) -> Option<String> {
    let prefix = label
        .split(" :: ")
        .next()
        .unwrap_or(label)
        .trim()
        .replace('\\', "/");
    if prefix.contains('/') {
        Some(prefix)
    } else {
        None
    }
}

fn path_matches_scope(path: &str, prefix: &str) -> bool {
    path == prefix
        || path
            .strip_prefix(prefix)
            .is_some_and(|suffix| suffix.starts_with('/'))
}

fn path_suffix_for_label(label: &str, file_path: &str) -> String {
    let label_parts = label.split('/').collect::<Vec<_>>();
    let file_parts = file_path.split('/').collect::<Vec<_>>();
    let mut common = 0usize;
    while common < label_parts.len()
        && common < file_parts.len()
        && label_parts[common] == file_parts[common]
    {
        common += 1;
    }
    let suffix = if common < file_parts.len() {
        file_parts[common..].join("/")
    } else {
        file_parts
            .iter()
            .rev()
            .take(2)
            .copied()
            .collect::<Vec<_>>()
            .into_iter()
            .rev()
            .collect::<Vec<_>>()
            .join("/")
    };
    if suffix.len() <= 120 {
        suffix
    } else {
        file_parts
            .iter()
            .rev()
            .take(3)
            .copied()
            .collect::<Vec<_>>()
            .into_iter()
            .rev()
            .collect::<Vec<_>>()
            .join("/")
    }
}

fn is_file_node(node: &GraphNode) -> bool {
    node.node_type == "file"
}

fn sorted_counts(counts: BTreeMap<String, usize>) -> Vec<CountItem> {
    let mut items = counts
        .into_iter()
        .map(|(name, count)| CountItem { name, count })
        .collect::<Vec<_>>();
    items.sort_by(|a, b| b.count.cmp(&a.count).then_with(|| a.name.cmp(&b.name)));
    items
}

fn node_degree_json_for_community(node: &GraphNode, degree: usize, community: usize) -> Value {
    json!({
        "id": node.id,
        "label": node.label,
        "type": node.node_type,
        "degree": degree,
        "file_path": node.file_path,
        "line_start": node.line_start,
        "community": community,
    })
}

fn summarize_node(node: &GraphNode, incoming: usize, outgoing: usize, degree: usize) -> String {
    let connectivity = match degree {
        0 => "isolated",
        1 => "minimally connected",
        2..=4 => "lightly connected",
        5..=19 => "moderately connected",
        _ => "highly connected",
    };
    format!(
        "`{}` is a {} node that is {} ({} total edges: {} incoming, {} outgoing).",
        node.label, node.node_type, connectivity, degree, incoming, outgoing
    )
}

fn top_level_dir(path: &str) -> &str {
    path.split('/').next().unwrap_or(path)
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

fn xml_escape(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}

fn cypher_escape(value: &str) -> String {
    value
        .replace('\\', "\\\\")
        .replace('\'', "\\'")
        .replace('\n', "\\n")
        .replace('\r', "\\r")
}

fn neo4j_label(value: &str) -> String {
    let mut out = value
        .chars()
        .filter_map(|ch| {
            if ch.is_ascii_alphanumeric() {
                Some(ch)
            } else if ch == '_' {
                Some(ch)
            } else {
                None
            }
        })
        .collect::<String>();
    if out.is_empty()
        || !out
            .chars()
            .next()
            .is_some_and(|ch| ch.is_ascii_alphabetic())
    {
        out.insert_str(0, "Node");
    }
    out
}

fn neo4j_relation(value: &str) -> String {
    let mut out = String::new();
    for ch in value.chars() {
        if ch.is_ascii_alphanumeric() {
            out.push(ch.to_ascii_uppercase());
        } else {
            out.push('_');
        }
    }
    while out.contains("__") {
        out = out.replace("__", "_");
    }
    let out = out.trim_matches('_').to_string();
    if out.is_empty() {
        "RELATED_TO".to_string()
    } else {
        out
    }
}

const CS_KEYWORDS: &[&str] = &[
    "abstract",
    "as",
    "base",
    "bool",
    "break",
    "byte",
    "case",
    "catch",
    "char",
    "checked",
    "class",
    "const",
    "continue",
    "decimal",
    "default",
    "delegate",
    "do",
    "double",
    "else",
    "enum",
    "event",
    "explicit",
    "extern",
    "false",
    "finally",
    "fixed",
    "float",
    "for",
    "foreach",
    "goto",
    "if",
    "implicit",
    "in",
    "int",
    "interface",
    "internal",
    "is",
    "lock",
    "long",
    "namespace",
    "new",
    "null",
    "object",
    "operator",
    "out",
    "override",
    "params",
    "private",
    "protected",
    "public",
    "readonly",
    "ref",
    "return",
    "sbyte",
    "sealed",
    "short",
    "sizeof",
    "stackalloc",
    "static",
    "string",
    "struct",
    "switch",
    "this",
    "throw",
    "true",
    "try",
    "typeof",
    "uint",
    "ulong",
    "unchecked",
    "unsafe",
    "ushort",
    "using",
    "var",
    "virtual",
    "void",
    "volatile",
    "while",
    "get",
    "set",
    "init",
    "value",
];

const FEATURE_STOP_TOKENS: &[&str] = &[
    "asset",
    "assets",
    "activity",
    "base",
    "btn",
    "build",
    "button",
    "client",
    "code",
    "com",
    "common",
    "component",
    "components",
    "container",
    "core",
    "csharp",
    "csharpframework",
    "custom",
    "data",
    "fix",
    "framework",
    "game",
    "gameview",
    "gen",
    "generate",
    "generated",
    "group",
    "handler",
    "helper",
    "hot",
    "hotfix",
    "icon",
    "info",
    "item",
    "items",
    "list",
    "logic",
    "lua",
    "lua2csharp",
    "lua2csharpcode",
    "main",
    "manager",
    "module",
    "modules",
    "node",
    "page",
    "pages",
    "panel",
    "popup",
    "runtime",
    "script",
    "scripts",
    "simple",
    "ui",
    "util",
    "utils",
    "view",
    "views",
];

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{FileEntry, Symbol};

    #[test]
    fn builds_graph_with_path_and_explain() {
        let mut files = BTreeMap::new();
        files.insert(
            "Services/UserService.cs".to_string(),
            FileEntry {
                path: "Services/UserService.cs".to_string(),
                language: "csharp".to_string(),
                line_count: 6,
                byte_size: 120,
                modified_unix_ms: 0,
                content_hash: "a".to_string(),
                namespace: Some("Game.Services".to_string()),
                imports: vec![],
                symbols: vec![Symbol {
                    name: "UserService".to_string(),
                    kind: "class".to_string(),
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
                language: "csharp".to_string(),
                line_count: 3,
                byte_size: 80,
                modified_unix_ms: 0,
                content_hash: "b".to_string(),
                namespace: Some("Game.Data".to_string()),
                imports: vec![],
                symbols: vec![Symbol {
                    name: "UserRepository".to_string(),
                    kind: "class".to_string(),
                    line_start: 1,
                    line_end: 3,
                    detail: "class UserRepository".to_string(),
                }],
                content: "class UserRepository {}".to_string(),
            },
        );
        let graph = CodeGraph::build(&files, &HashMap::new());
        assert!(graph.stats().nodes >= 4);
        let explain = graph.explain("UserService", 10).expect("node exists");
        assert_eq!(explain.node.label, "UserService");
        let path = graph.shortest_path("UserService", "UserRepository", 5);
        assert!(path.found);
    }
}
