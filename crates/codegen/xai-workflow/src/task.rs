use serde::{Deserialize, Serialize};

use crate::{
    MAX_TASK_DESCRIPTION_LEN, MAX_TASK_ID_LEN, MAX_TASK_RESULT_SUMMARY_LEN, MAX_WORKFLOW_TASKS,
};

pub const DEFAULT_TASK_MAX_ATTEMPTS: u32 = 1;
pub const MAX_TASK_ATTEMPTS: u32 = 16;

fn default_max_attempts() -> u32 {
    DEFAULT_TASK_MAX_ATTEMPTS
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TaskDefinition {
    pub id: String,
    pub description: String,
    #[serde(default)]
    pub depends_on: Vec<String>,
    #[serde(default = "default_max_attempts")]
    pub max_attempts: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TaskStatus {
    Pending,
    Running,
    Completed,
    Failed,
    Blocked,
}

impl TaskStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Running => "in_progress",
            Self::Completed => "completed",
            Self::Failed => "failed",
            Self::Blocked => "blocked",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TaskState {
    pub id: String,
    pub description: String,
    #[serde(default)]
    pub depends_on: Vec<String>,
    pub status: TaskStatus,
    #[serde(default)]
    pub attempts: u32,
    pub max_attempts: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub result_summary: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TaskQueueState {
    #[serde(default)]
    pub tasks: Vec<TaskState>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub current_task: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum TaskQueueError {
    #[error("task queue contains too many tasks (maximum {MAX_WORKFLOW_TASKS})")]
    TooManyTasks,
    #[error("task id must be non-empty and at most {MAX_TASK_ID_LEN} bytes: {0}")]
    InvalidId(String),
    #[error("task description must be non-empty and at most {MAX_TASK_DESCRIPTION_LEN} bytes: {0}")]
    InvalidDescription(String),
    #[error("task max_attempts must be between 1 and {MAX_TASK_ATTEMPTS}: {task}")]
    InvalidMaxAttempts { task: String },
    #[error("duplicate task id: {0}")]
    DuplicateId(String),
    #[error("task {task} depends on unknown task {dependency}")]
    UnknownDependency { task: String, dependency: String },
    #[error("task dependency graph contains a cycle")]
    DependencyCycle,
    #[error("task not found: {0}")]
    NotFound(String),
    #[error("task {task} is not eligible: dependencies are incomplete")]
    DependenciesIncomplete { task: String },
    #[error("task {task} is already {status}")]
    InvalidTransition { task: String, status: String },
    #[error("task {task} cannot be started after {attempts} attempts")]
    AttemptsExhausted { task: String, attempts: u32 },
    #[error("task {task} is not the current task")]
    NotCurrent { task: String },
    #[error("task result summary exceeds {MAX_TASK_RESULT_SUMMARY_LEN} bytes")]
    ResultSummaryTooLong,
}

impl TaskQueueState {
    pub fn from_definitions(definitions: &[TaskDefinition]) -> Result<Self, TaskQueueError> {
        if definitions.len() > MAX_WORKFLOW_TASKS {
            return Err(TaskQueueError::TooManyTasks);
        }
        let mut ids = std::collections::HashSet::with_capacity(definitions.len());
        for definition in definitions {
            validate_definition(definition)?;
            if !ids.insert(definition.id.as_str()) {
                return Err(TaskQueueError::DuplicateId(definition.id.clone()));
            }
        }
        for definition in definitions {
            for dependency in &definition.depends_on {
                if !ids.contains(dependency.as_str()) {
                    return Err(TaskQueueError::UnknownDependency {
                        task: definition.id.clone(),
                        dependency: dependency.clone(),
                    });
                }
            }
        }
        let mut visiting = std::collections::HashSet::new();
        let mut visited = std::collections::HashSet::new();
        for definition in definitions {
            visit_definition(
                definition.id.as_str(),
                definitions,
                &mut visiting,
                &mut visited,
            )?;
        }
        Ok(Self {
            tasks: definitions
                .iter()
                .map(|definition| TaskState {
                    id: definition.id.clone(),
                    description: definition.description.clone(),
                    depends_on: definition.depends_on.clone(),
                    status: TaskStatus::Pending,
                    attempts: 0,
                    max_attempts: definition.max_attempts,
                    result_summary: None,
                })
                .collect(),
            current_task: None,
        })
    }

    /// Validate a queue restored from durable state before making it visible
    /// to a workflow executor.
    pub fn validate(&self) -> Result<(), TaskQueueError> {
        if self.tasks.len() > MAX_WORKFLOW_TASKS {
            return Err(TaskQueueError::TooManyTasks);
        }
        let definitions = self
            .tasks
            .iter()
            .map(|task| TaskDefinition {
                id: task.id.clone(),
                description: task.description.clone(),
                depends_on: task.depends_on.clone(),
                max_attempts: task.max_attempts,
            })
            .collect::<Vec<_>>();
        Self::from_definitions(&definitions)?;

        let running: Vec<&TaskState> = self
            .tasks
            .iter()
            .filter(|task| task.status == TaskStatus::Running)
            .collect();
        if running.len() > 1 {
            return Err(TaskQueueError::InvalidTransition {
                task: running[1].id.clone(),
                status: "multiple tasks are running".into(),
            });
        }
        match self.current_task.as_deref() {
            Some(task_id) => {
                let task = self
                    .tasks
                    .iter()
                    .find(|task| task.id == task_id)
                    .ok_or_else(|| TaskQueueError::NotFound(task_id.to_string()))?;
                if task.status != TaskStatus::Running {
                    return Err(TaskQueueError::InvalidTransition {
                        task: task_id.to_string(),
                        status: task.status.as_str().to_string(),
                    });
                }
            }
            None if !running.is_empty() => {
                return Err(TaskQueueError::InvalidTransition {
                    task: running[0].id.clone(),
                    status: "running task has no current_task".into(),
                });
            }
            None => {}
        }
        for (index, task) in self.tasks.iter().enumerate() {
            if task.attempts > task.max_attempts {
                return Err(TaskQueueError::AttemptsExhausted {
                    task: task.id.clone(),
                    attempts: task.attempts,
                });
            }
            let dependencies_completed = self.dependencies_completed(index);
            let dependency_failed = task.depends_on.iter().any(|dependency| {
                self.tasks
                    .iter()
                    .find(|candidate| candidate.id == *dependency)
                    .is_some_and(|candidate| {
                        matches!(candidate.status, TaskStatus::Failed | TaskStatus::Blocked)
                    })
            });
            let invalid = match task.status {
                TaskStatus::Pending => task.attempts >= task.max_attempts || dependency_failed,
                TaskStatus::Running | TaskStatus::Completed => {
                    task.attempts == 0 || !dependencies_completed
                }
                TaskStatus::Failed => task.attempts != task.max_attempts,
                TaskStatus::Blocked => !dependency_failed,
            };
            if invalid {
                return Err(TaskQueueError::InvalidTransition {
                    task: task.id.clone(),
                    status: task.status.as_str().to_string(),
                });
            }
            validate_summary(&task.result_summary)?;
        }
        Ok(())
    }

    pub fn matches_definitions(&self, definitions: &[TaskDefinition]) -> bool {
        let Ok(expected) = Self::from_definitions(definitions) else {
            return false;
        };
        self.tasks.len() == expected.tasks.len()
            && expected.tasks.iter().all(|expected| {
                self.tasks.iter().any(|actual| {
                    actual.id == expected.id
                        && actual.description == expected.description
                        && actual.depends_on == expected.depends_on
                        && actual.max_attempts == expected.max_attempts
                })
            })
    }

    pub fn start(&mut self, task_id: &str) -> Result<Self, TaskQueueError> {
        let index = self.index(task_id)?;
        let task = &self.tasks[index];
        // Task transitions are deliberately idempotent across workflow resume.
        // Check terminal/current states before the single-cursor guard because
        // a completed earlier task is replayed while the next task is current.
        if matches!(
            task.status,
            TaskStatus::Completed | TaskStatus::Failed | TaskStatus::Blocked
        ) || (self.current_task.as_deref() == Some(task_id)
            && task.status == TaskStatus::Running)
        {
            return Ok(self.clone());
        }
        if let Some(current) = self.current_task.as_deref()
            && current != task_id
        {
            return Err(TaskQueueError::InvalidTransition {
                task: task_id.to_string(),
                status: format!("task {current} is running"),
            });
        }
        if task.status != TaskStatus::Pending {
            return Err(TaskQueueError::InvalidTransition {
                task: task_id.to_string(),
                status: task.status.as_str().to_string(),
            });
        }
        if task.attempts >= task.max_attempts {
            return Err(TaskQueueError::AttemptsExhausted {
                task: task_id.to_string(),
                attempts: task.attempts,
            });
        }
        if !self.dependencies_completed(index) {
            return Err(TaskQueueError::DependenciesIncomplete {
                task: task_id.to_string(),
            });
        }
        let task = &mut self.tasks[index];
        task.attempts = task.attempts.saturating_add(1);
        task.status = TaskStatus::Running;
        task.result_summary = None;
        self.current_task = Some(task_id.to_string());
        Ok(self.clone())
    }

    pub fn complete(
        &mut self,
        task_id: &str,
        result_summary: Option<String>,
    ) -> Result<Self, TaskQueueError> {
        validate_summary(&result_summary)?;
        let index = self.index(task_id)?;
        if self.tasks[index].status == TaskStatus::Completed {
            return Ok(self.clone());
        }
        self.require_current(task_id)?;
        if self.tasks[index].status != TaskStatus::Running {
            return Err(TaskQueueError::InvalidTransition {
                task: task_id.to_string(),
                status: self.tasks[index].status.as_str().to_string(),
            });
        }
        self.tasks[index].status = TaskStatus::Completed;
        self.tasks[index].result_summary = result_summary;
        self.current_task = None;
        Ok(self.clone())
    }

    pub fn fail(
        &mut self,
        task_id: &str,
        result_summary: Option<String>,
    ) -> Result<Self, TaskQueueError> {
        validate_summary(&result_summary)?;
        let index = self.index(task_id)?;
        if matches!(
            self.tasks[index].status,
            TaskStatus::Completed | TaskStatus::Failed | TaskStatus::Blocked
        ) || (self.tasks[index].status == TaskStatus::Pending
            && self.current_task.is_none()
            && self.tasks[index].attempts > 0)
        {
            return Ok(self.clone());
        }
        self.require_current(task_id)?;
        if self.tasks[index].status != TaskStatus::Running {
            return Err(TaskQueueError::InvalidTransition {
                task: task_id.to_string(),
                status: self.tasks[index].status.as_str().to_string(),
            });
        }
        self.tasks[index].result_summary = result_summary;
        self.tasks[index].status = if self.tasks[index].attempts >= self.tasks[index].max_attempts {
            TaskStatus::Failed
        } else {
            TaskStatus::Pending
        };
        self.current_task = None;
        if self.tasks[index].status == TaskStatus::Failed {
            self.block_dependents();
        }
        Ok(self.clone())
    }

    /// Make an interrupted running attempt safe to resume after a process
    /// restart. The attempt remains counted, so retry limits cannot be bypassed.
    pub fn recover_after_interruption(&mut self) -> bool {
        let mut changed = false;
        for task in &mut self.tasks {
            if task.status == TaskStatus::Running {
                task.result_summary = None;
                task.status = if task.attempts >= task.max_attempts {
                    TaskStatus::Failed
                } else {
                    TaskStatus::Pending
                };
                changed = true;
            }
        }
        if self.current_task.take().is_some() {
            changed = true;
        }
        if changed {
            self.block_dependents();
        }
        changed
    }

    pub fn eligible_tasks(&self) -> Vec<String> {
        self.tasks
            .iter()
            .enumerate()
            .filter(|(index, task)| {
                task.status == TaskStatus::Pending
                    && task.attempts < task.max_attempts
                    && self.dependencies_completed(*index)
            })
            .map(|(_, task)| task.id.clone())
            .collect()
    }

    fn block_dependents(&mut self) {
        loop {
            let mut changed = false;
            for index in 0..self.tasks.len() {
                if self.tasks[index].status != TaskStatus::Pending {
                    continue;
                }
                let blocked = self.tasks[index].depends_on.iter().any(|dependency| {
                    self.tasks
                        .iter()
                        .find(|task| task.id == *dependency)
                        .is_some_and(|task| {
                            matches!(task.status, TaskStatus::Failed | TaskStatus::Blocked)
                        })
                });
                if blocked {
                    self.tasks[index].status = TaskStatus::Blocked;
                    changed = true;
                }
            }
            if !changed {
                break;
            }
        }
    }

    fn require_current(&self, task_id: &str) -> Result<(), TaskQueueError> {
        if self.current_task.as_deref() != Some(task_id) {
            return Err(TaskQueueError::NotCurrent {
                task: task_id.to_string(),
            });
        }
        Ok(())
    }

    fn index(&self, task_id: &str) -> Result<usize, TaskQueueError> {
        self.tasks
            .iter()
            .position(|task| task.id == task_id)
            .ok_or_else(|| TaskQueueError::NotFound(task_id.to_string()))
    }

    fn dependencies_completed(&self, index: usize) -> bool {
        self.tasks[index].depends_on.iter().all(|dependency| {
            self.tasks
                .iter()
                .find(|task| task.id == *dependency)
                .is_some_and(|task| task.status == TaskStatus::Completed)
        })
    }
}

fn validate_summary(summary: &Option<String>) -> Result<(), TaskQueueError> {
    if summary
        .as_deref()
        .is_some_and(|summary| summary.len() > crate::MAX_TASK_RESULT_SUMMARY_LEN)
    {
        return Err(TaskQueueError::ResultSummaryTooLong);
    }
    Ok(())
}

fn validate_definition(definition: &TaskDefinition) -> Result<(), TaskQueueError> {
    if definition.id.trim().is_empty() || definition.id.len() > MAX_TASK_ID_LEN {
        return Err(TaskQueueError::InvalidId(definition.id.clone()));
    }
    if !definition
        .id
        .bytes()
        .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
    {
        return Err(TaskQueueError::InvalidId(definition.id.clone()));
    }
    if definition.description.trim().is_empty()
        || definition.description.len() > MAX_TASK_DESCRIPTION_LEN
    {
        return Err(TaskQueueError::InvalidDescription(definition.id.clone()));
    }
    if !(1..=MAX_TASK_ATTEMPTS).contains(&definition.max_attempts) {
        return Err(TaskQueueError::InvalidMaxAttempts {
            task: definition.id.clone(),
        });
    }
    Ok(())
}

fn visit_definition(
    id: &str,
    definitions: &[TaskDefinition],
    visiting: &mut std::collections::HashSet<String>,
    visited: &mut std::collections::HashSet<String>,
) -> Result<(), TaskQueueError> {
    if visited.contains(id) {
        return Ok(());
    }
    if !visiting.insert(id.to_string()) {
        return Err(TaskQueueError::DependencyCycle);
    }
    let definition = definitions
        .iter()
        .find(|definition| definition.id == id)
        .expect("dependencies were validated before cycle detection");
    for dependency in &definition.depends_on {
        visit_definition(dependency, definitions, visiting, visited)?;
    }
    visiting.remove(id);
    visited.insert(id.to_string());
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn definitions() -> Vec<TaskDefinition> {
        vec![
            TaskDefinition {
                id: "plan".into(),
                description: "Plan the work".into(),
                depends_on: vec![],
                max_attempts: 1,
            },
            TaskDefinition {
                id: "ship".into(),
                description: "Ship the work".into(),
                depends_on: vec!["plan".into()],
                max_attempts: 2,
            },
        ]
    }

    #[test]
    fn validates_dependencies_and_cycles() {
        let mut invalid = definitions();
        invalid[1].depends_on = vec!["missing".into()];
        assert!(matches!(
            TaskQueueState::from_definitions(&invalid),
            Err(TaskQueueError::UnknownDependency { .. })
        ));
        let mut cyclic = definitions();
        cyclic[0].depends_on = vec!["ship".into()];
        assert_eq!(
            TaskQueueState::from_definitions(&cyclic),
            Err(TaskQueueError::DependencyCycle)
        );
    }

    #[test]
    fn completion_leaves_next_task_pending_until_script_starts_it() {
        let mut queue = TaskQueueState::from_definitions(&definitions()).unwrap();
        queue.start("plan").unwrap();
        let mut queue = queue.complete("plan", Some("planned".into())).unwrap();
        assert_eq!(queue.current_task, None);
        assert_eq!(queue.tasks[0].status, TaskStatus::Completed);
        assert_eq!(queue.tasks[1].status, TaskStatus::Pending);
        assert_eq!(queue.tasks[1].attempts, 0);
        queue.start("ship").unwrap();
        assert_eq!(queue.current_task.as_deref(), Some("ship"));
    }

    #[test]
    fn failure_allows_retry_then_blocks_dependents_when_exhausted() {
        let mut queue = TaskQueueState::from_definitions(&definitions()).unwrap();
        queue.start("plan").unwrap();
        let mut queue = queue.complete("plan", None).unwrap();
        queue.start("ship").unwrap();
        let mut queue = queue.fail("ship", Some("retry".into())).unwrap();
        assert_eq!(queue.tasks[1].status, TaskStatus::Pending);
        let mut queue = queue.start("ship").unwrap();
        let queue = queue.fail("ship", Some("permanent".into())).unwrap();
        assert_eq!(queue.tasks[1].status, TaskStatus::Failed);
    }

    #[test]
    fn snapshot_round_trip_defaults_missing_queue_fields() {
        let json = serde_json::json!({"tasks": [{"id":"one", "description":"One", "status":"pending", "max_attempts":1}]});
        let queue: TaskQueueState = serde_json::from_value(json).unwrap();
        assert_eq!(queue.current_task, None);
        assert_eq!(queue.tasks[0].attempts, 0);
        assert!(queue.tasks[0].depends_on.is_empty());
    }

    #[test]
    fn replayed_start_of_completed_task_is_idempotent_while_next_task_runs() {
        let mut queue = TaskQueueState::from_definitions(&definitions()).unwrap();
        queue.start("plan").unwrap();
        queue = queue.complete("plan", None).unwrap();
        queue.start("ship").unwrap();
        let replay = queue.start("plan").unwrap();
        assert_eq!(replay, queue);
        assert_eq!(replay.current_task.as_deref(), Some("ship"));
    }

    #[test]
    fn failed_task_blocks_transitive_dependents() {
        let mut defs = definitions();
        defs.push(TaskDefinition {
            id: "publish".into(),
            description: "Publish the work".into(),
            depends_on: vec!["ship".into()],
            max_attempts: 1,
        });
        let mut queue = TaskQueueState::from_definitions(&defs).unwrap();
        queue.start("plan").unwrap();
        queue = queue.fail("plan", Some("failed".into())).unwrap();
        assert_eq!(queue.tasks[1].status, TaskStatus::Blocked);
        assert_eq!(queue.tasks[2].status, TaskStatus::Blocked);
        assert!(queue.eligible_tasks().is_empty());
    }

    #[test]
    fn interrupted_running_task_becomes_retryable_pending() {
        let mut queue = TaskQueueState::from_definitions(&definitions()).unwrap();
        queue.start("plan").unwrap();
        queue = queue.complete("plan", None).unwrap();
        queue.start("ship").unwrap();
        assert!(queue.recover_after_interruption());
        assert_eq!(queue.current_task, None);
        assert_eq!(queue.tasks[1].status, TaskStatus::Pending);
        assert_eq!(queue.tasks[1].attempts, 1);
        queue.validate().unwrap();
    }

    #[test]
    fn restored_definition_matching_is_order_insensitive() {
        let queue = TaskQueueState::from_definitions(&definitions()).unwrap();
        let mut reordered = definitions();
        reordered.swap(0, 1);
        assert!(queue.matches_definitions(&reordered));
    }

    #[test]
    fn restored_queue_validation_requires_a_single_current_running_task() {
        let mut queue = TaskQueueState::from_definitions(&definitions()).unwrap();
        queue.tasks[0].status = TaskStatus::Running;
        assert!(matches!(
            queue.validate(),
            Err(TaskQueueError::InvalidTransition { .. })
        ));
        queue.tasks[0].status = TaskStatus::Pending;
        assert!(queue.validate().is_ok());
        assert!(queue.matches_definitions(&definitions()));
    }
}
