use std::collections::{HashMap, HashSet, VecDeque};

/// Type of sub-agent to use for a task.
#[derive(Debug, Clone, PartialEq)]
pub enum AgentType {
    Explore,
    Plan,
    General,
}

impl AgentType {
    pub fn from_str(s: &str) -> Self {
        match s.to_lowercase().as_str() {
            "explore" => Self::Explore,
            "plan" => Self::Plan,
            _ => Self::General,
        }
    }

    pub fn as_str(&self) -> &str {
        match self {
            Self::Explore => "explore",
            Self::Plan => "plan",
            Self::General => "general",
        }
    }
}

/// Status of a task node in the graph.
#[derive(Debug, Clone, PartialEq)]
pub enum TaskStatus {
    Pending,
    Running,
    Completed,
    Failed,
    Cancelled,
}

/// A single task node in the DAG.
#[derive(Debug, Clone)]
pub struct TaskNode {
    pub id: String,
    pub prompt: String,
    pub description: String,
    pub agent_type: AgentType,
    pub model_override: String,
    pub depends_on: Vec<String>,
    pub status: TaskStatus,
    pub result: Option<String>,
    pub error: Option<String>,
}

/// Directed Acyclic Graph of task nodes with dependency tracking.
pub struct TaskGraph {
    pub nodes: HashMap<String, TaskNode>,
    /// Reverse index: task_id -> list of task IDs that depend on it.
    dependents: HashMap<String, Vec<String>>,
}

impl TaskGraph {
    /// Build a new task graph from a list of nodes.
    /// Automatically constructs the reverse dependency index.
    pub fn new(nodes: Vec<TaskNode>) -> Self {
        let mut node_map = HashMap::new();
        let mut dependents: HashMap<String, Vec<String>> = HashMap::new();

        for node in nodes {
            // Build reverse index: for each dependency, record this node as a dependent
            for dep_id in &node.depends_on {
                dependents
                    .entry(dep_id.clone())
                    .or_default()
                    .push(node.id.clone());
            }
            node_map.insert(node.id.clone(), node);
        }

        Self {
            nodes: node_map,
            dependents,
        }
    }

    /// Get IDs of tasks that are ready to run:
    /// status == Pending AND all dependencies are Completed.
    pub fn get_ready_tasks(&self) -> Vec<String> {
        self.nodes
            .values()
            .filter(|node| {
                node.status == TaskStatus::Pending
                    && node.depends_on.iter().all(|dep_id| {
                        self.nodes
                            .get(dep_id)
                            .map(|d| d.status == TaskStatus::Completed)
                            .unwrap_or(false)
                    })
            })
            .map(|n| n.id.clone())
            .collect()
    }

    /// Mark a task as running.
    pub fn mark_running(&mut self, id: &str) {
        if let Some(node) = self.nodes.get_mut(id) {
            node.status = TaskStatus::Running;
        }
    }

    /// Mark a task as completed with its result.
    pub fn mark_completed(&mut self, id: &str, result: String) {
        if let Some(node) = self.nodes.get_mut(id) {
            node.status = TaskStatus::Completed;
            node.result = Some(result);
        }
    }

    /// Mark a task as failed with an error.
    pub fn mark_failed(&mut self, id: &str, error: String) {
        if let Some(node) = self.nodes.get_mut(id) {
            node.status = TaskStatus::Failed;
            node.error = Some(error);
        }
    }

    /// Mark a task as cancelled.
    pub fn mark_cancelled(&mut self, id: &str) {
        if let Some(node) = self.nodes.get_mut(id) {
            node.status = TaskStatus::Cancelled;
        }
    }

    /// Collect results from all completed dependencies of a task.
    /// Returns (description, result) pairs.
    pub fn collect_dependency_results(&self, id: &str) -> Vec<(String, String)> {
        let Some(node) = self.nodes.get(id) else {
            return Vec::new();
        };

        node.depends_on
            .iter()
            .filter_map(|dep_id| {
                let dep = self.nodes.get(dep_id)?;
                let result = dep.result.as_ref()?;
                Some((dep.description.clone(), result.clone()))
            })
            .collect()
    }

    /// True when no Pending or Running tasks remain.
    pub fn all_done(&self) -> bool {
        self.nodes
            .values()
            .all(|n| matches!(n.status, TaskStatus::Completed | TaskStatus::Failed | TaskStatus::Cancelled))
    }

    /// True if any task has failed.
    pub fn has_failures(&self) -> bool {
        self.nodes.values().any(|n| n.status == TaskStatus::Failed)
    }

    /// Synthesize a summary from all completed leaf node results.
    /// Leaf nodes are those that no other task depends on.
    pub fn synthesize_results(&self) -> String {
        // Find leaf nodes: nodes that are not in the dependents map (nobody depends on them)
        let depended_on: HashSet<&str> = self.dependents.keys().map(|s| s.as_str()).collect();

        let mut parts = Vec::new();
        // Collect results in sorted order for determinism
        let mut sorted_ids: Vec<&String> = self.nodes.keys().collect();
        sorted_ids.sort();

        for id in sorted_ids {
            let node = &self.nodes[id];
            if !depended_on.contains(id.as_str()) {
                if let Some(result) = &node.result {
                    parts.push(format!(
                        "## {} (Task {})\n\n{}",
                        node.description, node.id, result
                    ));
                } else if let Some(error) = &node.error {
                    parts.push(format!(
                        "## {} (Task {}) — FAILED\n\nError: {}",
                        node.description, node.id, error
                    ));
                }
            }
        }

        if parts.is_empty() {
            // Fall back to all results
            let mut sorted_ids: Vec<&String> = self.nodes.keys().collect();
            sorted_ids.sort();
            for id in sorted_ids {
                let node = &self.nodes[id];
                if let Some(result) = &node.result {
                    parts.push(format!(
                        "## {} (Task {})\n\n{}",
                        node.description, node.id, result
                    ));
                }
            }
        }

        parts.join("\n\n---\n\n")
    }

    /// Validate the graph: check for cycles and missing references.
    /// Uses Kahn's algorithm for topological sort.
    pub fn validate(&self) -> Result<(), String> {
        let node_ids: HashSet<&str> = self.nodes.keys().map(|s| s.as_str()).collect();

        // Check for missing dependency references
        for node in self.nodes.values() {
            for dep_id in &node.depends_on {
                if !node_ids.contains(dep_id.as_str()) {
                    return Err(format!(
                        "Task '{}' depends on '{}' which does not exist",
                        node.id, dep_id
                    ));
                }
            }
        }

        // Kahn's algorithm for cycle detection
        let mut in_degree: HashMap<&str, usize> = HashMap::new();
        for id in &node_ids {
            in_degree.insert(id, 0);
        }
        for node in self.nodes.values() {
            for _dep_id in &node.depends_on {
                *in_degree.entry(node.id.as_str()).or_insert(0) += 1;
            }
        }

        let mut queue: VecDeque<&str> = in_degree
            .iter()
            .filter(|(_, deg)| **deg == 0)
            .map(|(&id, _)| id)
            .collect();

        let mut visited = 0usize;
        while let Some(id) = queue.pop_front() {
            visited += 1;
            if let Some(deps) = self.dependents.get(id) {
                for dep_id in deps {
                    if let Some(deg) = in_degree.get_mut(dep_id.as_str()) {
                        *deg -= 1;
                        if *deg == 0 {
                            queue.push_back(dep_id.as_str());
                        }
                    }
                }
            }
        }

        if visited != node_ids.len() {
            return Err("Task graph contains a cycle".to_string());
        }

        Ok(())
    }

    /// Get the number of nodes in the graph.
    pub fn len(&self) -> usize {
        self.nodes.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_node(id: &str, desc: &str, deps: Vec<&str>) -> TaskNode {
        TaskNode {
            id: id.to_string(),
            prompt: format!("Do {}", desc),
            description: desc.to_string(),
            agent_type: AgentType::General,
            model_override: String::new(),
            depends_on: deps.into_iter().map(|s| s.to_string()).collect(),
            status: TaskStatus::Pending,
            result: None,
            error: None,
        }
    }

    #[test]
    fn test_basic_dag() {
        let nodes = vec![
            make_node("1", "Research X", vec![]),
            make_node("2", "Research Y", vec![]),
            make_node("3", "Compare", vec!["1", "2"]),
        ];
        let graph = TaskGraph::new(nodes);

        assert_eq!(graph.len(), 3);
        assert!(graph.validate().is_ok());

        let ready = graph.get_ready_tasks();
        assert_eq!(ready.len(), 2);
        assert!(ready.contains(&"1".to_string()));
        assert!(ready.contains(&"2".to_string()));
    }

    #[test]
    fn test_ready_tasks_after_completion() {
        let nodes = vec![
            make_node("1", "Research X", vec![]),
            make_node("2", "Research Y", vec![]),
            make_node("3", "Compare", vec!["1", "2"]),
        ];
        let mut graph = TaskGraph::new(nodes);

        graph.mark_running("1");
        graph.mark_completed("1", "Result X".to_string());

        // Task 3 still blocked on task 2
        let ready = graph.get_ready_tasks();
        assert_eq!(ready.len(), 1);
        assert!(ready.contains(&"2".to_string()));

        graph.mark_running("2");
        graph.mark_completed("2", "Result Y".to_string());

        // Now task 3 is ready
        let ready = graph.get_ready_tasks();
        assert_eq!(ready.len(), 1);
        assert!(ready.contains(&"3".to_string()));
    }

    #[test]
    fn test_cycle_detection() {
        let nodes = vec![
            make_node("1", "A", vec!["3"]),
            make_node("2", "B", vec!["1"]),
            make_node("3", "C", vec!["2"]),
        ];
        let graph = TaskGraph::new(nodes);

        let result = graph.validate();
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("cycle"));
    }

    #[test]
    fn test_missing_dependency() {
        let nodes = vec![make_node("1", "A", vec!["99"])];
        let graph = TaskGraph::new(nodes);

        let result = graph.validate();
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("does not exist"));
    }

    #[test]
    fn test_dependency_results() {
        let nodes = vec![
            make_node("1", "Research X", vec![]),
            make_node("2", "Research Y", vec![]),
            make_node("3", "Compare", vec!["1", "2"]),
        ];
        let mut graph = TaskGraph::new(nodes);

        graph.mark_completed("1", "X is great".to_string());
        graph.mark_completed("2", "Y is good".to_string());

        let results = graph.collect_dependency_results("3");
        assert_eq!(results.len(), 2);
    }

    #[test]
    fn test_all_done() {
        let nodes = vec![
            make_node("1", "A", vec![]),
            make_node("2", "B", vec!["1"]),
        ];
        let mut graph = TaskGraph::new(nodes);

        assert!(!graph.all_done());

        graph.mark_completed("1", "done".to_string());
        assert!(!graph.all_done());

        graph.mark_completed("2", "done".to_string());
        assert!(graph.all_done());
    }

    #[test]
    fn test_has_failures() {
        let nodes = vec![
            make_node("1", "A", vec![]),
            make_node("2", "B", vec![]),
        ];
        let mut graph = TaskGraph::new(nodes);

        assert!(!graph.has_failures());

        graph.mark_failed("1", "oops".to_string());
        assert!(graph.has_failures());
    }

    #[test]
    fn test_synthesize_results() {
        let nodes = vec![
            make_node("1", "Research X", vec![]),
            make_node("2", "Research Y", vec![]),
            make_node("3", "Compare", vec!["1", "2"]),
        ];
        let mut graph = TaskGraph::new(nodes);

        graph.mark_completed("1", "X findings".to_string());
        graph.mark_completed("2", "Y findings".to_string());
        graph.mark_completed("3", "Comparison result".to_string());

        let output = graph.synthesize_results();
        // Task 3 is the leaf (nothing depends on it)
        assert!(output.contains("Comparison result"));
        assert!(output.contains("Compare"));
    }

    #[test]
    fn test_single_task() {
        let nodes = vec![make_node("1", "Simple task", vec![])];
        let graph = TaskGraph::new(nodes);

        assert!(graph.validate().is_ok());
        let ready = graph.get_ready_tasks();
        assert_eq!(ready.len(), 1);
    }

    #[test]
    fn test_diamond_dag() {
        // A -> B, A -> C, B -> D, C -> D
        let nodes = vec![
            make_node("a", "Start", vec![]),
            make_node("b", "Branch 1", vec!["a"]),
            make_node("c", "Branch 2", vec!["a"]),
            make_node("d", "Merge", vec!["b", "c"]),
        ];
        let graph = TaskGraph::new(nodes);

        assert!(graph.validate().is_ok());

        let ready = graph.get_ready_tasks();
        assert_eq!(ready.len(), 1);
        assert!(ready.contains(&"a".to_string()));
    }
}
