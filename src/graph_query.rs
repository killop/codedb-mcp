use anyhow::{Result, anyhow, bail};
use serde::Serialize;
use serde_json::{Map, Value, json};
use std::cmp::Ordering as CmpOrdering;
use std::collections::{BTreeMap, BTreeSet};

#[derive(Debug, Clone, PartialEq)]
pub struct Query {
    pub patterns: Vec<PathPattern>,
    pub predicates: Vec<Predicate>,
    pub projections: Vec<Projection>,
    pub order_by: Vec<Ordering>,
    pub limit: Option<usize>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct PathPattern {
    pub shortest: bool,
    pub variable: Option<String>,
    pub nodes: Vec<NodePattern>,
    pub relationships: Vec<RelationshipPattern>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct NodePattern {
    pub variable: String,
    pub label: Option<String>,
    pub properties: BTreeMap<String, Scalar>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct RelationshipPattern {
    pub variable: Option<String>,
    pub types: Vec<String>,
    pub direction: Direction,
    pub min_hops: usize,
    pub max_hops: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Direction {
    Outgoing,
    Incoming,
    Either,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Predicate {
    pub variable: String,
    pub property: String,
    pub operator: PredicateOperator,
    pub value: Operand,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PredicateOperator {
    Equal,
    NotEqual,
    LessThan,
    LessThanOrEqual,
    GreaterThan,
    GreaterThanOrEqual,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Operand {
    Scalar(Scalar),
    Property { variable: String, property: String },
}

#[derive(Debug, Clone, PartialEq)]
pub struct Projection {
    pub variable: String,
    pub property: Option<String>,
    pub alias: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Ordering {
    pub variable: String,
    pub property: String,
    pub direction: SortDirection,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SortDirection {
    Ascending,
    Descending,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(untagged)]
pub enum Scalar {
    String(String),
    Integer(i64),
    Boolean(bool),
    Null,
}

impl Scalar {
    fn as_json(&self) -> Value {
        match self {
            Self::String(value) => Value::String(value.clone()),
            Self::Integer(value) => json!(value),
            Self::Boolean(value) => json!(value),
            Self::Null => Value::Null,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct QueryNode {
    pub id: String,
    pub labels: Vec<String>,
    pub properties: BTreeMap<String, Scalar>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct QueryEdge {
    pub id: String,
    pub source: String,
    pub target: String,
    #[serde(rename = "type")]
    pub edge_type: String,
    pub properties: BTreeMap<String, Scalar>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct QueryPath {
    pub nodes: Vec<QueryNode>,
    pub relationships: Vec<QueryEdge>,
}

pub trait QueryProvider {
    fn seed_nodes(&mut self, pattern: &NodePattern) -> Result<Vec<QueryNode>>;

    fn expand(
        &mut self,
        node: &QueryNode,
        relationship: &RelationshipPattern,
    ) -> Result<Vec<(QueryEdge, QueryNode)>>;
}

#[derive(Debug, Clone)]
enum BindingValue {
    Node(QueryNode),
    Edge(QueryEdge),
    Path(QueryPath),
    Edges(Vec<QueryEdge>),
}

type Bindings = BTreeMap<String, BindingValue>;

pub fn parse(input: &str) -> Result<Query> {
    Parser::new(tokenize(input)?)?.parse_query()
}

pub fn execute(provider: &mut impl QueryProvider, query: &Query) -> Result<Value> {
    let mut bindings = vec![Bindings::new()];
    for pattern in &query.patterns {
        let mut next = Vec::new();
        for binding in &bindings {
            match_pattern(provider, query, pattern, binding, &mut next)?;
        }
        bindings = next;
        if bindings.is_empty() {
            break;
        }
    }

    bindings.retain(|binding| predicates_match(binding, &query.predicates));
    let mut rows = bindings
        .iter()
        .map(|binding| {
            Ok((
                query
                    .order_by
                    .iter()
                    .map(|ordering| {
                        binding_property(binding.get(&ordering.variable), &ordering.property)
                            .cloned()
                    })
                    .collect::<Vec<_>>(),
                project(binding, &query.projections)?,
            ))
        })
        .collect::<Result<Vec<_>>>()?;
    rows.sort_by(|left, right| {
        compare_order_values(&left.0, &right.0, &query.order_by)
            .then_with(|| canonical_json(&left.1).cmp(&canonical_json(&right.1)))
    });
    let mut rows = rows.into_iter().map(|(_, row)| row).collect::<Vec<_>>();
    rows.dedup_by(|left, right| canonical_json(left) == canonical_json(right));
    if let Some(limit) = query.limit {
        rows.truncate(limit);
    }
    Ok(json!({
        "columns": query.projections.iter().map(projection_name).collect::<Vec<_>>(),
        "rows": rows,
        "count": rows.len(),
    }))
}

fn match_pattern(
    provider: &mut impl QueryProvider,
    query: &Query,
    pattern: &PathPattern,
    existing: &Bindings,
    out: &mut Vec<Bindings>,
) -> Result<()> {
    let (planned, reverse_output) = plan_pattern(query, pattern, existing);
    let pattern = &planned;
    let first = pattern
        .nodes
        .first()
        .ok_or_else(|| anyhow!("MATCH path must contain a node"))?;
    let starts = match existing.get(&first.variable) {
        Some(BindingValue::Node(node)) if node_matches(node, first) => vec![node.clone()],
        Some(_) => Vec::new(),
        None => {
            let mut pushed = first.clone();
            for predicate in query.predicates.iter().filter(|predicate| {
                predicate.variable == first.variable
                    && predicate.operator == PredicateOperator::Equal
            }) {
                if let Operand::Scalar(value) = &predicate.value {
                    pushed
                        .properties
                        .entry(predicate.property.clone())
                        .or_insert_with(|| value.clone());
                }
            }
            provider.seed_nodes(&pushed)?
        }
    };
    for start in starts {
        if !node_matches(&start, first) {
            continue;
        }
        let mut binding = existing.clone();
        if !bind(
            &mut binding,
            &first.variable,
            BindingValue::Node(start.clone()),
        ) {
            continue;
        }
        let path = QueryPath {
            nodes: vec![start.clone()],
            relationships: Vec::new(),
        };
        match_path_step(
            provider,
            query,
            pattern,
            0,
            start,
            binding,
            path,
            reverse_output,
            out,
        )?;
    }
    Ok(())
}

fn plan_pattern(query: &Query, pattern: &PathPattern, existing: &Bindings) -> (PathPattern, bool) {
    if pattern.nodes.len() < 2 {
        return (pattern.clone(), false);
    }
    let first = pattern.nodes.first().expect("checked non-empty pattern");
    let last = pattern.nodes.last().expect("checked non-empty pattern");
    let first_score = node_plan_score(query, first, existing);
    let last_score = node_plan_score(query, last, existing);
    if last_score <= first_score {
        return (pattern.clone(), false);
    }

    let mut reversed = pattern.clone();
    reversed.nodes.reverse();
    reversed.relationships.reverse();
    for relationship in &mut reversed.relationships {
        relationship.direction = match relationship.direction {
            Direction::Outgoing => Direction::Incoming,
            Direction::Incoming => Direction::Outgoing,
            Direction::Either => Direction::Either,
        };
    }
    (reversed, true)
}

fn node_plan_score(query: &Query, node: &NodePattern, existing: &Bindings) -> usize {
    if existing.contains_key(&node.variable) {
        return usize::MAX;
    }
    let property_score = |property: &str| match property {
        "id" | "path" => 100,
        "name" | "owner_name" | "line_start" => 60,
        _ => 10,
    };
    let inline = node
        .properties
        .keys()
        .map(|property| property_score(property))
        .sum::<usize>();
    let predicates = query
        .predicates
        .iter()
        .filter(|predicate| {
            predicate.variable == node.variable
                && predicate.operator == PredicateOperator::Equal
                && matches!(&predicate.value, Operand::Scalar(_))
        })
        .map(|predicate| property_score(&predicate.property))
        .sum::<usize>();
    inline + predicates
}

fn match_path_step(
    provider: &mut impl QueryProvider,
    query: &Query,
    pattern: &PathPattern,
    relationship_index: usize,
    current: QueryNode,
    binding: Bindings,
    path: QueryPath,
    reverse_output: bool,
    out: &mut Vec<Bindings>,
) -> Result<()> {
    if relationship_index == pattern.relationships.len() {
        let mut binding = binding;
        if let Some(variable) = &pattern.variable
            && !bind(
                &mut binding,
                variable,
                BindingValue::Path(if reverse_output {
                    reverse_query_path(path)
                } else {
                    path
                }),
            )
        {
            return Ok(());
        }
        out.push(binding);
        return Ok(());
    }

    let relationship = &pattern.relationships[relationship_index];
    let target_pattern = &pattern.nodes[relationship_index + 1];
    let mut pushed_target = target_pattern.clone();
    for predicate in query.predicates.iter().filter(|predicate| {
        predicate.variable == target_pattern.variable
            && predicate.operator == PredicateOperator::Equal
    }) {
        if let Operand::Scalar(value) = &predicate.value {
            pushed_target
                .properties
                .entry(predicate.property.clone())
                .or_insert_with(|| value.clone());
        }
    }
    let expansions = variable_length_expansions(
        provider,
        &current,
        relationship,
        (!pushed_target.properties.is_empty()).then_some(&pushed_target),
        pattern.shortest,
    )?;
    for expansion in expansions {
        let Some(target) = expansion.nodes.last().cloned() else {
            continue;
        };
        if !node_matches(&target, target_pattern) {
            continue;
        }
        let mut next_binding = binding.clone();
        if !bind(
            &mut next_binding,
            &target_pattern.variable,
            BindingValue::Node(target.clone()),
        ) {
            continue;
        }
        if let Some(variable) = &relationship.variable {
            let value = if expansion.relationships.len() == 1 {
                BindingValue::Edge(expansion.relationships[0].clone())
            } else {
                BindingValue::Edges(expansion.relationships.clone())
            };
            if !bind(&mut next_binding, variable, value) {
                continue;
            }
        }
        let mut next_path = path.clone();
        next_path.nodes.extend(expansion.nodes.into_iter().skip(1));
        next_path.relationships.extend(expansion.relationships);
        match_path_step(
            provider,
            query,
            pattern,
            relationship_index + 1,
            target,
            next_binding,
            next_path,
            reverse_output,
            out,
        )?;
    }
    Ok(())
}

fn reverse_query_path(mut path: QueryPath) -> QueryPath {
    path.nodes.reverse();
    path.relationships.reverse();
    path
}

fn variable_length_expansions(
    provider: &mut impl QueryProvider,
    start: &QueryNode,
    relationship: &RelationshipPattern,
    target: Option<&NodePattern>,
    shortest: bool,
) -> Result<Vec<QueryPath>> {
    if shortest && let Some(target) = target {
        return shortest_connectors(provider, start, target, relationship);
    }
    let mut out = Vec::new();
    let use_reverse = target.is_some() && relationship.max_hops > 1;
    let reverse_distance = if use_reverse {
        let target = target.expect("reverse pruning requires a target");
        reverse_reachable_nodes(provider, target, relationship)?
    } else {
        BTreeMap::new()
    };
    if use_reverse && !reverse_distance.is_empty() && !reverse_distance.contains_key(&start.id) {
        return Ok(out);
    }
    let mut visited = BTreeSet::from([start.id.clone()]);
    let path = QueryPath {
        nodes: vec![start.clone()],
        relationships: Vec::new(),
    };
    expand_depth_first(
        provider,
        relationship,
        start,
        0,
        (!reverse_distance.is_empty()).then_some(&reverse_distance),
        &mut visited,
        path,
        &mut out,
    )?;
    out.sort_by_key(|path| {
        path.nodes
            .iter()
            .map(|node| node.id.as_str())
            .collect::<Vec<_>>()
            .join("\u{0}")
    });
    Ok(out)
}

fn shortest_connectors(
    provider: &mut impl QueryProvider,
    start: &QueryNode,
    target: &NodePattern,
    relationship: &RelationshipPattern,
) -> Result<Vec<QueryPath>> {
    let mut targets = provider.seed_nodes(target)?;
    targets.sort_by(|left, right| left.id.cmp(&right.id));
    targets.dedup_by(|left, right| left.id == right.id);
    if targets.is_empty() {
        return Ok(Vec::new());
    }
    if relationship.min_hops == 0 && targets.iter().any(|target| target.id == start.id) {
        return Ok(vec![QueryPath {
            nodes: vec![start.clone()],
            relationships: Vec::new(),
        }]);
    }

    let reverse = RelationshipPattern {
        variable: None,
        types: relationship.types.clone(),
        direction: match relationship.direction {
            Direction::Outgoing => Direction::Incoming,
            Direction::Incoming => Direction::Outgoing,
            Direction::Either => Direction::Either,
        },
        min_hops: 1,
        max_hops: 1,
    };
    let step = RelationshipPattern {
        variable: None,
        types: relationship.types.clone(),
        direction: relationship.direction,
        min_hops: 1,
        max_hops: 1,
    };

    let mut nodes = BTreeMap::<String, QueryNode>::new();
    nodes.insert(start.id.clone(), start.clone());
    let mut forward_distance = BTreeMap::from([(start.id.clone(), 0usize)]);
    let mut backward_distance = BTreeMap::<String, usize>::new();
    let mut forward_parent = BTreeMap::<String, (String, QueryEdge)>::new();
    let mut backward_next = BTreeMap::<String, (String, QueryEdge)>::new();
    let mut forward_frontier = vec![start.clone()];
    let mut backward_frontier = targets;
    for target in &backward_frontier {
        nodes.insert(target.id.clone(), target.clone());
        backward_distance.insert(target.id.clone(), 0);
    }
    let mut forward_depth = 0usize;
    let mut backward_depth = 0usize;
    let mut best_hops = None::<usize>;
    let mut meetings = BTreeSet::<String>::new();

    while !forward_frontier.is_empty() && !backward_frontier.is_empty() {
        if forward_depth + backward_depth >= relationship.max_hops {
            break;
        }
        if best_hops.is_some_and(|best| forward_depth + backward_depth >= best) {
            break;
        }
        let expand_forward = forward_frontier.len() <= backward_frontier.len();
        if expand_forward {
            let next_depth = forward_depth + 1;
            let mut next = BTreeMap::<String, QueryNode>::new();
            for current in std::mem::take(&mut forward_frontier) {
                let mut neighbors = provider.expand(&current, &step)?;
                neighbors.sort_by(|left, right| {
                    left.1
                        .id
                        .cmp(&right.1.id)
                        .then_with(|| left.0.id.cmp(&right.0.id))
                });
                for (edge, neighbor) in neighbors {
                    if !forward_distance.contains_key(&neighbor.id) {
                        forward_distance.insert(neighbor.id.clone(), next_depth);
                        forward_parent.insert(neighbor.id.clone(), (current.id.clone(), edge));
                        nodes.insert(neighbor.id.clone(), neighbor.clone());
                        next.insert(neighbor.id.clone(), neighbor.clone());
                    }
                    if let Some(backward) = backward_distance.get(&neighbor.id) {
                        record_shortest_meeting(
                            &mut best_hops,
                            &mut meetings,
                            next_depth + *backward,
                            &neighbor.id,
                            relationship,
                        );
                    }
                }
            }
            forward_frontier = next.into_values().collect();
            forward_depth = next_depth;
        } else {
            let next_depth = backward_depth + 1;
            let mut next = BTreeMap::<String, QueryNode>::new();
            for current in std::mem::take(&mut backward_frontier) {
                let mut neighbors = provider.expand(&current, &reverse)?;
                neighbors.sort_by(|left, right| {
                    left.1
                        .id
                        .cmp(&right.1.id)
                        .then_with(|| left.0.id.cmp(&right.0.id))
                });
                for (edge, neighbor) in neighbors {
                    if !backward_distance.contains_key(&neighbor.id) {
                        backward_distance.insert(neighbor.id.clone(), next_depth);
                        backward_next.insert(neighbor.id.clone(), (current.id.clone(), edge));
                        nodes.insert(neighbor.id.clone(), neighbor.clone());
                        next.insert(neighbor.id.clone(), neighbor.clone());
                    }
                    if let Some(forward) = forward_distance.get(&neighbor.id) {
                        record_shortest_meeting(
                            &mut best_hops,
                            &mut meetings,
                            next_depth + *forward,
                            &neighbor.id,
                            relationship,
                        );
                    }
                }
            }
            backward_frontier = next.into_values().collect();
            backward_depth = next_depth;
        }
    }

    let mut paths = meetings
        .into_iter()
        .filter_map(|meeting| {
            reconstruct_shortest_path(&meeting, &nodes, &forward_parent, &backward_next)
        })
        .collect::<Vec<_>>();
    paths.sort_by_key(|path| {
        path.nodes
            .iter()
            .map(|node| node.id.as_str())
            .collect::<Vec<_>>()
            .join("\u{0}")
    });
    paths.dedup_by(|left, right| {
        left.nodes
            .iter()
            .map(|node| &node.id)
            .eq(right.nodes.iter().map(|node| &node.id))
    });
    Ok(paths)
}

fn record_shortest_meeting(
    best_hops: &mut Option<usize>,
    meetings: &mut BTreeSet<String>,
    hops: usize,
    meeting: &str,
    relationship: &RelationshipPattern,
) {
    if hops < relationship.min_hops || hops > relationship.max_hops {
        return;
    }
    match best_hops {
        Some(best) if *best < hops => {}
        Some(best) if *best == hops => {
            meetings.insert(meeting.to_string());
        }
        _ => {
            *best_hops = Some(hops);
            meetings.clear();
            meetings.insert(meeting.to_string());
        }
    }
}

fn reconstruct_shortest_path(
    meeting: &str,
    nodes: &BTreeMap<String, QueryNode>,
    forward_parent: &BTreeMap<String, (String, QueryEdge)>,
    backward_next: &BTreeMap<String, (String, QueryEdge)>,
) -> Option<QueryPath> {
    let mut left_nodes = vec![meeting.to_string()];
    let mut left_edges = Vec::<QueryEdge>::new();
    let mut current = meeting.to_string();
    while let Some((previous, edge)) = forward_parent.get(&current) {
        left_edges.push(edge.clone());
        current = previous.clone();
        left_nodes.push(current.clone());
    }
    left_nodes.reverse();
    left_edges.reverse();

    let mut node_ids = left_nodes;
    let mut edges = left_edges;
    current = meeting.to_string();
    while let Some((next, edge)) = backward_next.get(&current) {
        edges.push(edge.clone());
        current = next.clone();
        node_ids.push(current.clone());
    }
    let path_nodes = node_ids
        .into_iter()
        .map(|id| nodes.get(&id).cloned())
        .collect::<Option<Vec<_>>>()?;
    Some(QueryPath {
        nodes: path_nodes,
        relationships: edges,
    })
}

fn expand_depth_first(
    provider: &mut impl QueryProvider,
    relationship: &RelationshipPattern,
    current: &QueryNode,
    depth: usize,
    reverse_distance: Option<&BTreeMap<String, usize>>,
    visited: &mut BTreeSet<String>,
    path: QueryPath,
    out: &mut Vec<QueryPath>,
) -> Result<()> {
    if depth >= relationship.min_hops {
        out.push(path.clone());
    }
    if depth >= relationship.max_hops {
        return Ok(());
    }
    let mut neighbors = provider.expand(current, relationship)?;
    neighbors.sort_by(|left, right| {
        left.1
            .id
            .cmp(&right.1.id)
            .then_with(|| left.0.edge_type.cmp(&right.0.edge_type))
            .then_with(|| left.0.id.cmp(&right.0.id))
    });
    for (edge, node) in neighbors {
        if reverse_distance.is_some_and(|distance| {
            distance
                .get(&node.id)
                .is_none_or(|remaining| depth + 1 + *remaining > relationship.max_hops)
        }) {
            continue;
        }
        if !visited.insert(node.id.clone()) {
            continue;
        }
        let mut next_path = path.clone();
        next_path.relationships.push(edge);
        next_path.nodes.push(node.clone());
        expand_depth_first(
            provider,
            relationship,
            &node,
            depth + 1,
            reverse_distance,
            visited,
            next_path,
            out,
        )?;
        visited.remove(&node.id);
    }
    Ok(())
}

fn reverse_reachable_nodes(
    provider: &mut impl QueryProvider,
    target: &NodePattern,
    relationship: &RelationshipPattern,
) -> Result<BTreeMap<String, usize>> {
    let mut distance = BTreeMap::<String, usize>::new();
    let mut frontier = provider.seed_nodes(target)?;
    frontier.sort_by(|left, right| left.id.cmp(&right.id));
    frontier.dedup_by(|left, right| left.id == right.id);
    for node in &frontier {
        distance.insert(node.id.clone(), 0);
    }
    let reverse = RelationshipPattern {
        variable: None,
        types: relationship.types.clone(),
        direction: match relationship.direction {
            Direction::Outgoing => Direction::Incoming,
            Direction::Incoming => Direction::Outgoing,
            Direction::Either => Direction::Either,
        },
        min_hops: 1,
        max_hops: 1,
    };
    for depth in 1..=relationship.max_hops {
        let mut next = BTreeMap::<String, QueryNode>::new();
        for node in frontier {
            for (_, neighbor) in provider.expand(&node, &reverse)? {
                if distance.contains_key(&neighbor.id) {
                    continue;
                }
                next.entry(neighbor.id.clone()).or_insert(neighbor);
            }
        }
        if next.is_empty() {
            break;
        }
        for id in next.keys() {
            distance.insert(id.clone(), depth);
        }
        frontier = next.into_values().collect();
    }
    Ok(distance)
}

fn bind(binding: &mut Bindings, variable: &str, value: BindingValue) -> bool {
    match binding.get(variable) {
        Some(existing) => binding_values_equal(existing, &value),
        None => {
            binding.insert(variable.to_string(), value);
            true
        }
    }
}

fn binding_values_equal(left: &BindingValue, right: &BindingValue) -> bool {
    match (left, right) {
        (BindingValue::Node(left), BindingValue::Node(right)) => left.id == right.id,
        (BindingValue::Edge(left), BindingValue::Edge(right)) => left.id == right.id,
        (BindingValue::Path(left), BindingValue::Path(right)) => {
            left.nodes
                .iter()
                .map(|node| &node.id)
                .eq(right.nodes.iter().map(|node| &node.id))
                && left
                    .relationships
                    .iter()
                    .map(|edge| &edge.id)
                    .eq(right.relationships.iter().map(|edge| &edge.id))
        }
        (BindingValue::Edges(left), BindingValue::Edges(right)) => left
            .iter()
            .map(|edge| &edge.id)
            .eq(right.iter().map(|edge| &edge.id)),
        _ => false,
    }
}

fn node_matches(node: &QueryNode, pattern: &NodePattern) -> bool {
    pattern
        .label
        .as_ref()
        .is_none_or(|label| node.labels.iter().any(|candidate| candidate == label))
        && pattern
            .properties
            .iter()
            .all(|(property, value)| node.properties.get(property) == Some(value))
}

fn predicates_match(binding: &Bindings, predicates: &[Predicate]) -> bool {
    predicates.iter().all(|predicate| {
        let Some(actual) = binding_property(binding.get(&predicate.variable), &predicate.property)
        else {
            return false;
        };
        let expected = match &predicate.value {
            Operand::Scalar(value) => Some(value),
            Operand::Property { variable, property } => {
                binding_property(binding.get(variable), property)
            }
        };
        let Some(expected) = expected else {
            return false;
        };
        match predicate.operator {
            PredicateOperator::Equal => actual == expected,
            PredicateOperator::NotEqual => actual != expected,
            PredicateOperator::LessThan => scalar_cmp(actual, expected) == Some(CmpOrdering::Less),
            PredicateOperator::LessThanOrEqual => matches!(
                scalar_cmp(actual, expected),
                Some(CmpOrdering::Less | CmpOrdering::Equal)
            ),
            PredicateOperator::GreaterThan => {
                scalar_cmp(actual, expected) == Some(CmpOrdering::Greater)
            }
            PredicateOperator::GreaterThanOrEqual => matches!(
                scalar_cmp(actual, expected),
                Some(CmpOrdering::Greater | CmpOrdering::Equal)
            ),
        }
    })
}

fn scalar_cmp(left: &Scalar, right: &Scalar) -> Option<CmpOrdering> {
    match (left, right) {
        (Scalar::String(left), Scalar::String(right)) => Some(left.cmp(right)),
        (Scalar::Integer(left), Scalar::Integer(right)) => Some(left.cmp(right)),
        (Scalar::Boolean(left), Scalar::Boolean(right)) => Some(left.cmp(right)),
        (Scalar::Null, Scalar::Null) => Some(CmpOrdering::Equal),
        _ => None,
    }
}

fn compare_order_values(
    left: &[Option<Scalar>],
    right: &[Option<Scalar>],
    order_by: &[Ordering],
) -> CmpOrdering {
    for ((left, right), ordering) in left.iter().zip(right).zip(order_by) {
        let comparison = match (left, right) {
            (Some(left), Some(right)) => scalar_cmp(left, right).unwrap_or(CmpOrdering::Equal),
            (Some(_), None) => CmpOrdering::Less,
            (None, Some(_)) => CmpOrdering::Greater,
            (None, None) => CmpOrdering::Equal,
        };
        let comparison = match ordering.direction {
            SortDirection::Ascending => comparison,
            SortDirection::Descending => comparison.reverse(),
        };
        if comparison != CmpOrdering::Equal {
            return comparison;
        }
    }
    CmpOrdering::Equal
}

fn binding_property<'a>(value: Option<&'a BindingValue>, property: &str) -> Option<&'a Scalar> {
    match value? {
        BindingValue::Node(node) => node.properties.get(property),
        BindingValue::Edge(edge) => edge.properties.get(property),
        BindingValue::Path(_) | BindingValue::Edges(_) => None,
    }
}

fn project(binding: &Bindings, projections: &[Projection]) -> Result<Value> {
    let mut row = Map::new();
    for projection in projections {
        let name = projection_name(projection);
        let value = binding.get(&projection.variable).ok_or_else(|| {
            anyhow!(
                "RETURN references unbound variable '{}'",
                projection.variable
            )
        })?;
        let value = if let Some(property) = &projection.property {
            binding_property(Some(value), property)
                .map(Scalar::as_json)
                .unwrap_or(Value::Null)
        } else {
            match value {
                BindingValue::Node(node) => serde_json::to_value(node)?,
                BindingValue::Edge(edge) => serde_json::to_value(edge)?,
                BindingValue::Path(path) => serde_json::to_value(path)?,
                BindingValue::Edges(edges) => serde_json::to_value(edges)?,
            }
        };
        row.insert(name, value);
    }
    Ok(Value::Object(row))
}

fn projection_name(projection: &Projection) -> String {
    projection.alias.clone().unwrap_or_else(|| {
        projection
            .property
            .as_ref()
            .map(|property| format!("{}.{}", projection.variable, property))
            .unwrap_or_else(|| projection.variable.clone())
    })
}

fn canonical_json(value: &Value) -> String {
    serde_json::to_string(value).unwrap_or_default()
}

#[derive(Debug, Clone, PartialEq)]
enum Token {
    Ident(String),
    String(String),
    Number(i64),
    LParen,
    RParen,
    LBracket,
    RBracket,
    LBrace,
    RBrace,
    Colon,
    Dot,
    Comma,
    Equal,
    NotEqual,
    LessThan,
    LessThanOrEqual,
    GreaterThan,
    GreaterThanOrEqual,
    Dash,
    ArrowRight,
    ArrowLeft,
    Star,
    Range,
    Pipe,
}

fn tokenize(input: &str) -> Result<Vec<Token>> {
    let chars = input.chars().collect::<Vec<_>>();
    let mut tokens = Vec::new();
    let mut index = 0usize;
    while index < chars.len() {
        let ch = chars[index];
        if ch.is_whitespace() || ch == ';' {
            index += 1;
            continue;
        }
        if ch == '\'' || ch == '"' {
            let quote = ch;
            index += 1;
            let mut value = String::new();
            let mut closed = false;
            while index < chars.len() {
                let current = chars[index];
                index += 1;
                if current == quote {
                    closed = true;
                    break;
                }
                if current == '\\' && index < chars.len() {
                    let escaped = chars[index];
                    index += 1;
                    value.push(match escaped {
                        'n' => '\n',
                        'r' => '\r',
                        't' => '\t',
                        other => other,
                    });
                } else {
                    value.push(current);
                }
            }
            if !closed {
                bail!("unterminated string literal");
            }
            tokens.push(Token::String(value));
            continue;
        }
        if ch.is_ascii_digit() {
            let start = index;
            index += 1;
            while index < chars.len() && chars[index].is_ascii_digit() {
                index += 1;
            }
            let value = chars[start..index].iter().collect::<String>().parse()?;
            tokens.push(Token::Number(value));
            continue;
        }
        if is_ident_start(ch) {
            let start = index;
            index += 1;
            while index < chars.len() && is_ident_continue(chars[index]) {
                index += 1;
            }
            tokens.push(Token::Ident(chars[start..index].iter().collect()));
            continue;
        }
        let token = match ch {
            '(' => Token::LParen,
            ')' => Token::RParen,
            '[' => Token::LBracket,
            ']' => Token::RBracket,
            '{' => Token::LBrace,
            '}' => Token::RBrace,
            ':' => Token::Colon,
            ',' => Token::Comma,
            '|' => Token::Pipe,
            '*' => Token::Star,
            '=' => Token::Equal,
            '!' if chars.get(index + 1) == Some(&'=') => {
                index += 1;
                Token::NotEqual
            }
            '<' if chars.get(index + 1) == Some(&'>') => {
                index += 1;
                Token::NotEqual
            }
            '<' if chars.get(index + 1) == Some(&'=') => {
                index += 1;
                Token::LessThanOrEqual
            }
            '<' if chars.get(index + 1) == Some(&'-') => {
                index += 1;
                Token::ArrowLeft
            }
            '>' if chars.get(index + 1) == Some(&'=') => {
                index += 1;
                Token::GreaterThanOrEqual
            }
            '<' => Token::LessThan,
            '>' => Token::GreaterThan,
            '-' if chars.get(index + 1) == Some(&'>') => {
                index += 1;
                Token::ArrowRight
            }
            '-' => Token::Dash,
            '.' if chars.get(index + 1) == Some(&'.') => {
                index += 1;
                Token::Range
            }
            '.' => Token::Dot,
            _ => bail!("unexpected character '{ch}'"),
        };
        tokens.push(token);
        index += 1;
    }
    Ok(tokens)
}

fn is_ident_start(ch: char) -> bool {
    ch.is_ascii_alphabetic() || ch == '_'
}

fn is_ident_continue(ch: char) -> bool {
    ch.is_ascii_alphanumeric() || ch == '_'
}

struct Parser {
    tokens: Vec<Token>,
    position: usize,
    anonymous: usize,
}

impl Parser {
    fn new(tokens: Vec<Token>) -> Result<Self> {
        if tokens.is_empty() {
            bail!("empty graph query");
        }
        Ok(Self {
            tokens,
            position: 0,
            anonymous: 0,
        })
    }

    fn parse_query(mut self) -> Result<Query> {
        self.expect_keyword("MATCH")?;
        let mut patterns = vec![self.parse_path_pattern()?];
        while self.consume(&Token::Comma) {
            patterns.push(self.parse_path_pattern()?);
        }
        let mut predicates = Vec::new();
        if self.consume_keyword("WHERE") {
            predicates.push(self.parse_predicate()?);
            while self.consume_keyword("AND") {
                predicates.push(self.parse_predicate()?);
            }
        }
        self.expect_keyword("RETURN")?;
        let mut projections = vec![self.parse_projection()?];
        while self.consume(&Token::Comma) {
            projections.push(self.parse_projection()?);
        }
        let mut order_by = Vec::new();
        if self.consume_keyword("ORDER") {
            self.expect_keyword("BY")?;
            order_by.push(self.parse_ordering()?);
            while self.consume(&Token::Comma) {
                order_by.push(self.parse_ordering()?);
            }
        }
        let limit = if self.consume_keyword("LIMIT") {
            Some(self.expect_usize("LIMIT")?)
        } else {
            None
        };
        if self.position != self.tokens.len() {
            bail!("unexpected token after query: {:?}", self.peek());
        }
        Ok(Query {
            patterns,
            predicates,
            projections,
            order_by,
            limit,
        })
    }

    fn parse_path_pattern(&mut self) -> Result<PathPattern> {
        let shortest = self.consume_keyword("SHORTEST");
        let variable = if matches!(self.peek(), Some(Token::Ident(_)))
            && self.tokens.get(self.position + 1) == Some(&Token::Equal)
        {
            let variable = self.expect_ident("path variable")?;
            self.expect(&Token::Equal)?;
            Some(variable)
        } else {
            None
        };
        let mut nodes = vec![self.parse_node_pattern()?];
        let mut relationships = Vec::new();
        while matches!(self.peek(), Some(Token::Dash | Token::ArrowLeft)) {
            relationships.push(self.parse_relationship_pattern()?);
            nodes.push(self.parse_node_pattern()?);
        }
        Ok(PathPattern {
            shortest,
            variable,
            nodes,
            relationships,
        })
    }

    fn parse_node_pattern(&mut self) -> Result<NodePattern> {
        self.expect(&Token::LParen)?;
        let variable = if matches!(self.peek(), Some(Token::Ident(_))) {
            self.expect_ident("node variable")?
        } else {
            self.anonymous += 1;
            format!("__node{}", self.anonymous)
        };
        let label = if self.consume(&Token::Colon) {
            Some(self.expect_ident("node label")?)
        } else {
            None
        };
        let properties = if self.consume(&Token::LBrace) {
            let mut properties = BTreeMap::new();
            if !self.consume(&Token::RBrace) {
                loop {
                    let name = self.expect_ident("property name")?;
                    self.expect(&Token::Colon)?;
                    let value = self.parse_scalar()?;
                    properties.insert(name, value);
                    if self.consume(&Token::RBrace) {
                        break;
                    }
                    self.expect(&Token::Comma)?;
                }
            }
            properties
        } else {
            BTreeMap::new()
        };
        self.expect(&Token::RParen)?;
        Ok(NodePattern {
            variable,
            label,
            properties,
        })
    }

    fn parse_relationship_pattern(&mut self) -> Result<RelationshipPattern> {
        let incoming = self.consume(&Token::ArrowLeft);
        if !incoming {
            self.expect(&Token::Dash)?;
        }
        self.expect(&Token::LBracket)?;
        let variable = if matches!(self.peek(), Some(Token::Ident(_))) {
            Some(self.expect_ident("relationship variable")?)
        } else {
            None
        };
        let mut types = Vec::new();
        if self.consume(&Token::Colon) {
            types.push(self.expect_ident("relationship type")?);
            while self.consume(&Token::Pipe) {
                self.consume(&Token::Colon);
                types.push(self.expect_ident("relationship type")?);
            }
        }
        let (min_hops, max_hops) = if self.consume(&Token::Star) {
            let min = if matches!(self.peek(), Some(Token::Number(_))) {
                self.expect_usize("lower path bound")?
            } else {
                1
            };
            if self.consume(&Token::Range) {
                let max = self.expect_usize("upper path bound")?;
                if max < min {
                    bail!("upper path bound must be >= lower bound");
                }
                (min, max)
            } else {
                (min, min)
            }
        } else {
            (1, 1)
        };
        self.expect(&Token::RBracket)?;
        let direction = if incoming {
            self.expect(&Token::Dash)?;
            Direction::Incoming
        } else if self.consume(&Token::ArrowRight) {
            Direction::Outgoing
        } else {
            self.expect(&Token::Dash)?;
            Direction::Either
        };
        Ok(RelationshipPattern {
            variable,
            types,
            direction,
            min_hops,
            max_hops,
        })
    }

    fn parse_predicate(&mut self) -> Result<Predicate> {
        let variable = self.expect_ident("predicate variable")?;
        self.expect(&Token::Dot)?;
        let property = self.expect_ident("predicate property")?;
        let operator = if self.consume(&Token::Equal) {
            PredicateOperator::Equal
        } else if self.consume(&Token::NotEqual) {
            PredicateOperator::NotEqual
        } else if self.consume(&Token::LessThan) {
            PredicateOperator::LessThan
        } else if self.consume(&Token::LessThanOrEqual) {
            PredicateOperator::LessThanOrEqual
        } else if self.consume(&Token::GreaterThan) {
            PredicateOperator::GreaterThan
        } else if self.consume(&Token::GreaterThanOrEqual) {
            PredicateOperator::GreaterThanOrEqual
        } else {
            bail!("WHERE supports =, !=/<>, <, <=, >, and >=");
        };
        let value = if matches!(self.peek(), Some(Token::Ident(_)))
            && self.tokens.get(self.position + 1) == Some(&Token::Dot)
        {
            let variable = self.expect_ident("predicate value variable")?;
            self.expect(&Token::Dot)?;
            let property = self.expect_ident("predicate value property")?;
            Operand::Property { variable, property }
        } else {
            Operand::Scalar(self.parse_scalar()?)
        };
        Ok(Predicate {
            variable,
            property,
            operator,
            value,
        })
    }

    fn parse_projection(&mut self) -> Result<Projection> {
        let variable = self.expect_ident("RETURN variable")?;
        let property = if self.consume(&Token::Dot) {
            Some(self.expect_ident("RETURN property")?)
        } else {
            None
        };
        let alias = if self.consume_keyword("AS") {
            Some(self.expect_ident("RETURN alias")?)
        } else {
            None
        };
        Ok(Projection {
            variable,
            property,
            alias,
        })
    }

    fn parse_ordering(&mut self) -> Result<Ordering> {
        let variable = self.expect_ident("ORDER BY variable")?;
        self.expect(&Token::Dot)?;
        let property = self.expect_ident("ORDER BY property")?;
        let direction = if self.consume_keyword("DESC") {
            SortDirection::Descending
        } else {
            self.consume_keyword("ASC");
            SortDirection::Ascending
        };
        Ok(Ordering {
            variable,
            property,
            direction,
        })
    }

    fn parse_scalar(&mut self) -> Result<Scalar> {
        match self.next() {
            Some(Token::String(value)) => Ok(Scalar::String(value)),
            Some(Token::Number(value)) => Ok(Scalar::Integer(value)),
            Some(Token::Ident(value)) if value.eq_ignore_ascii_case("true") => {
                Ok(Scalar::Boolean(true))
            }
            Some(Token::Ident(value)) if value.eq_ignore_ascii_case("false") => {
                Ok(Scalar::Boolean(false))
            }
            Some(Token::Ident(value)) if value.eq_ignore_ascii_case("null") => Ok(Scalar::Null),
            other => bail!("expected scalar literal, found {other:?}"),
        }
    }

    fn expect_keyword(&mut self, keyword: &str) -> Result<()> {
        if self.consume_keyword(keyword) {
            Ok(())
        } else {
            bail!("expected {keyword}")
        }
    }

    fn consume_keyword(&mut self, keyword: &str) -> bool {
        if matches!(self.peek(), Some(Token::Ident(value)) if value.eq_ignore_ascii_case(keyword)) {
            self.position += 1;
            true
        } else {
            false
        }
    }

    fn expect_ident(&mut self, context: &str) -> Result<String> {
        match self.next() {
            Some(Token::Ident(value)) => Ok(value),
            other => bail!("expected {context}, found {other:?}"),
        }
    }

    fn expect_usize(&mut self, context: &str) -> Result<usize> {
        match self.next() {
            Some(Token::Number(value)) if value >= 0 => Ok(value as usize),
            other => bail!("expected non-negative {context}, found {other:?}"),
        }
    }

    fn expect(&mut self, token: &Token) -> Result<()> {
        if self.consume(token) {
            Ok(())
        } else {
            bail!("expected {token:?}, found {:?}", self.peek())
        }
    }

    fn consume(&mut self, token: &Token) -> bool {
        if self.peek() == Some(token) {
            self.position += 1;
            true
        } else {
            false
        }
    }

    fn peek(&self) -> Option<&Token> {
        self.tokens.get(self.position)
    }

    fn next(&mut self) -> Option<Token> {
        let token = self.tokens.get(self.position).cloned();
        self.position += usize::from(token.is_some());
        token
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Default)]
    struct MemoryProvider {
        nodes: BTreeMap<String, QueryNode>,
        edges: Vec<QueryEdge>,
        seed_properties: Vec<BTreeMap<String, Scalar>>,
    }

    impl QueryProvider for MemoryProvider {
        fn seed_nodes(&mut self, pattern: &NodePattern) -> Result<Vec<QueryNode>> {
            self.seed_properties.push(pattern.properties.clone());
            Ok(self
                .nodes
                .values()
                .filter(|node| node_matches(node, pattern))
                .cloned()
                .collect())
        }

        fn expand(
            &mut self,
            node: &QueryNode,
            relationship: &RelationshipPattern,
        ) -> Result<Vec<(QueryEdge, QueryNode)>> {
            let type_matches = |edge: &&QueryEdge| {
                relationship.types.is_empty()
                    || relationship
                        .types
                        .iter()
                        .any(|kind| kind == &edge.edge_type)
            };
            let mut result = Vec::new();
            for edge in self.edges.iter().filter(type_matches) {
                if matches!(
                    relationship.direction,
                    Direction::Outgoing | Direction::Either
                ) && edge.source == node.id
                    && let Some(target) = self.nodes.get(&edge.target)
                {
                    result.push((edge.clone(), target.clone()));
                }
                if matches!(
                    relationship.direction,
                    Direction::Incoming | Direction::Either
                ) && edge.target == node.id
                    && let Some(source) = self.nodes.get(&edge.source)
                {
                    result.push((edge.clone(), source.clone()));
                }
            }
            Ok(result)
        }
    }

    fn node(id: &str, name: &str) -> QueryNode {
        QueryNode {
            id: id.to_string(),
            labels: vec!["Symbol".to_string()],
            properties: BTreeMap::from([("name".to_string(), Scalar::String(name.to_string()))]),
        }
    }

    fn edge(source: &str, target: &str, kind: &str) -> QueryEdge {
        QueryEdge {
            id: format!("{source}:{kind}:{target}"),
            source: source.to_string(),
            target: target.to_string(),
            edge_type: kind.to_string(),
            properties: BTreeMap::new(),
        }
    }

    fn metric_node(id: &str, name: &str, score: i64, community: i64) -> QueryNode {
        let mut node = node(id, name);
        node.properties
            .insert("score".to_string(), Scalar::Integer(score));
        node.properties
            .insert("community".to_string(), Scalar::Integer(community));
        node
    }

    #[test]
    fn parses_recursive_typed_path() {
        let query = parse(
            "MATCH p=(caller:Symbol)-[:CALLS|DISPATCHES_TO*1..8]->(leaf:Symbol) \
             WHERE caller.name = 'LoadReady' RETURN p, leaf.name AS terminal LIMIT 5",
        )
        .unwrap();
        assert_eq!(query.patterns[0].relationships[0].min_hops, 1);
        assert_eq!(query.patterns[0].relationships[0].max_hops, 8);
        assert_eq!(
            query.patterns[0].relationships[0].types,
            ["CALLS", "DISPATCHES_TO"]
        );
        assert_eq!(query.limit, Some(5));
    }

    #[test]
    fn shortest_path_uses_bidirectional_connector_plan() {
        let mut provider = MemoryProvider::default();
        for item in [
            node("a", "Start"),
            node("b", "Middle"),
            node("c", "End"),
            node("x", "Noise"),
        ] {
            provider.nodes.insert(item.id.clone(), item);
        }
        provider.edges = vec![
            edge("a", "b", "CALLS"),
            edge("b", "c", "CALLS"),
            edge("a", "x", "CALLS"),
        ];
        let query = parse(
            "MATCH SHORTEST p=(start:Symbol)-[:CALLS*1..5]->(end:Symbol) \
             WHERE start.name='Start' AND end.name='End' RETURN p",
        )
        .unwrap();
        assert!(query.patterns[0].shortest);
        let result = execute(&mut provider, &query).unwrap();
        assert_eq!(result["count"], 1);
        assert_eq!(result["rows"][0]["p"]["nodes"].as_array().unwrap().len(), 3);
    }

    #[test]
    fn executes_recursive_path_deterministically() {
        let mut provider = MemoryProvider::default();
        for item in [
            node("a", "LoadReady"),
            node("b", "Wrapper"),
            node("c", "Leaf"),
        ] {
            provider.nodes.insert(item.id.clone(), item);
        }
        provider.edges = vec![edge("a", "b", "CALLS"), edge("b", "c", "DISPATCHES_TO")];
        let query = parse(
            "MATCH p=(caller:Symbol)-[:CALLS|DISPATCHES_TO*1..2]->(leaf:Symbol) \
             WHERE caller.name='LoadReady' AND leaf.name='Leaf' RETURN p, leaf.name",
        )
        .unwrap();
        let result = execute(&mut provider, &query).unwrap();
        assert_eq!(result["count"], 1);
        assert_eq!(result["rows"][0]["leaf.name"], "Leaf");
        assert_eq!(
            result["rows"][0]["p"]["relationships"]
                .as_array()
                .unwrap()
                .len(),
            2
        );
    }

    #[test]
    fn joins_shared_state_patterns() {
        let state = QueryNode {
            id: "state".to_string(),
            labels: vec!["SharedState".to_string()],
            properties: BTreeMap::from([("name".to_string(), Scalar::String("task".to_string()))]),
        };
        let mut provider = MemoryProvider::default();
        for item in [
            node("consumer", "Consume"),
            node("producer", "Produce"),
            state,
        ] {
            provider.nodes.insert(item.id.clone(), item);
        }
        provider.edges = vec![
            edge("consumer", "state", "READS"),
            edge("producer", "state", "WRITES"),
        ];
        let query = parse(
            "MATCH (consumer:Symbol)-[:READS]->(state:SharedState)<-[:WRITES]-(producer:Symbol) \
             WHERE consumer.name='Consume' RETURN consumer.name, state.name, producer.name",
        )
        .unwrap();
        let result = execute(&mut provider, &query).unwrap();
        assert_eq!(result["count"], 1);
        assert_eq!(result["rows"][0]["producer.name"], "Produce");
    }

    #[test]
    fn compares_properties_and_orders_numeric_results() {
        let mut provider = MemoryProvider::default();
        for item in [
            metric_node("a", "Alpha", 2, 1),
            metric_node("b", "Beta", 5, 2),
            metric_node("c", "Gamma", 3, 1),
        ] {
            provider.nodes.insert(item.id.clone(), item);
        }
        provider.edges = vec![edge("a", "b", "DEPENDS_ON"), edge("c", "a", "DEPENDS_ON")];

        let ranked = parse(
            "MATCH (item:Symbol) WHERE item.score >= 3 \
             RETURN item.name, item.score ORDER BY item.score DESC",
        )
        .unwrap();
        let ranked = execute(&mut provider, &ranked).unwrap();
        assert_eq!(ranked["rows"][0]["item.name"], "Beta");
        assert_eq!(ranked["rows"][1]["item.name"], "Gamma");

        let bridge = parse(
            "MATCH (source:Symbol)-[:DEPENDS_ON]->(target:Symbol) \
             WHERE source.community != target.community RETURN source.name, target.name",
        )
        .unwrap();
        let bridge = execute(&mut provider, &bridge).unwrap();
        assert_eq!(bridge["count"], 1);
        assert_eq!(bridge["rows"][0]["source.name"], "Alpha");
        assert_eq!(bridge["rows"][0]["target.name"], "Beta");
    }

    #[test]
    fn plans_from_the_selective_path_endpoint_and_preserves_path_direction() {
        let mut provider = MemoryProvider::default();
        for item in [node("source", "Source"), node("target", "Target")] {
            provider.nodes.insert(item.id.clone(), item);
        }
        provider.edges = vec![edge("source", "target", "REFERENCES")];
        let query = parse(
            "MATCH p=(source:Symbol)-[:REFERENCES]->(target:Symbol) \
             WHERE target.name='Target' RETURN source.name, p",
        )
        .unwrap();
        let result = execute(&mut provider, &query).unwrap();

        assert_eq!(
            provider.seed_properties[0]["name"],
            Scalar::String("Target".to_string())
        );
        assert_eq!(result["rows"][0]["source.name"], "Source");
        assert_eq!(
            result["rows"][0]["p"]["nodes"][0]["properties"]["name"],
            "Source"
        );
        assert_eq!(
            result["rows"][0]["p"]["nodes"][1]["properties"]["name"],
            "Target"
        );
    }
}
