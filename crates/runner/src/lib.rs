use ace_http::send_request;
use model::Scenario;
use std::collections::HashMap;
use std::fmt;
use std::time::Duration;
use tokio::time::sleep;

#[derive(Debug)]
pub enum RunError {
    InvalidTransition {
        step: String,
        expected: String,
        actual: String,
    },
    HttpError {
        step: String,
        message: String,
    },
}

impl fmt::Display for RunError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            RunError::InvalidTransition {
                step,
                expected,
                actual,
            } => write!(
                f,
                "Step '{}': expected state '{}', but current state is '{}'",
                step, expected, actual
            ),
            RunError::HttpError { step, message } => {
                write!(f, "Step '{}': HTTP error: {}", step, message)
            }
        }
    }
}

#[derive(Debug, serde::Serialize, serde::Deserialize)]
pub struct ExecutionLog {
    pub steps: Vec<StepLog>,
}

#[derive(Debug, serde::Serialize, serde::Deserialize)]
pub struct StepLog {
    pub step_name: String,
    pub state_before: String,
    pub state_after: String,
    pub status: u16,
}

async fn run_once(scenario: &Scenario, task_id: usize) -> Result<(String, ExecutionLog), RunError> {
    let mut current_state = scenario.initial_state.clone();
    let mut context: HashMap<String, String> = HashMap::new();
    let mut log = ExecutionLog { steps: Vec::new() };

    for step in &scenario.steps {
        if step.transition.from != current_state {
            return Err(RunError::InvalidTransition {
                step: step.name.clone(),
                expected: step.transition.from.clone(),
                actual: current_state,
            });
        }

        let mut url = step.url.clone();
        for (key, value) in &context {
            url = url.replace(&format!("{{{{{}}}}}", key), value);
        }

        let max_attempts = step.retry.as_ref().map_or(1, |r| r.attempts);
        let delay_ms = step.retry.as_ref().map_or(0, |r| r.delay_ms);
        let mut last_err = None;

        for attempt in 1..=max_attempts {
            if attempt > 1 {
                println!("[User {}]     retrying... [attempt {}/{}]", task_id, attempt, max_attempts);
                sleep(Duration::from_millis(delay_ms)).await;
            }

            match send_request(&step.method, &url).await {
                Ok((status, body)) => {
                    if status == 200 || max_attempts == 1 {
                        println!(
                            "[User {}] [{}] --{}--> [{}] ✅ ({}) [attempt {}]",
                            task_id, step.transition.from, step.name, step.transition.to, status, attempt
                        );

                        if let Some(extract) = &step.extract {
                            let json: serde_json::Value = serde_json::from_str(&body)
                                .map_err(|e| RunError::HttpError {
                                    step: step.name.clone(),
                                    message: format!("Failed to parse JSON: {}", e),
                                })?;

                            for (context_key, json_key) in extract {
                                if let Some(value) = json.get(json_key) {
                                    let extracted = match value {
                                        serde_json::Value::String(s) => s.clone(),
                                        other => other.to_string(),
                                    };
                                    println!("[User {}]     extracted: {} = {}", task_id, context_key, extracted);
                                    context.insert(context_key.clone(), extracted);
                                }
                            }
                        }

                        log.steps.push(StepLog {
                            step_name: step.name.clone(),
                            state_before: step.transition.from.clone(),
                            state_after: step.transition.to.clone(),
                            status,
                        });

                        last_err = None;
                        break;
                    } else {
                        println!(
                            "[User {}] [{}] --{}--> [{}] ❌ ({}) [attempt {}]",
                            task_id, step.transition.from, step.name, step.transition.to, status, attempt
                        );
                        last_err = Some(format!("status {}", status));
                    }
                }
                Err(e) => {
                    println!(
                        "[User {}] [{}] --{}--> [{}] ❌ ({}) [attempt {}]",
                        task_id, step.transition.from, step.name, step.transition.to, e, attempt
                    );
                    last_err = Some(e);
                }
            }
        }

        if let Some(e) = last_err {
            return Err(RunError::HttpError {
                step: step.name.clone(),
                message: e,
            });
        }

        current_state = step.transition.to.clone();
    }

    Ok((current_state, log))
}

pub async fn run(scenario: &Scenario) -> Result<Vec<Result<(String, ExecutionLog), RunError>>, RunError> {
    let concurrency = scenario.concurrency.unwrap_or(1);

    let mut handles = Vec::new();
    for i in 1..=concurrency {
        let scenario = scenario.clone();
        handles.push(tokio::spawn(async move {
            run_once(&scenario, i).await
        }));
    }

    let mut results = Vec::new();
    for handle in handles {
        let result = handle.await.unwrap();
        results.push(result);
    }

    Ok(results)
}

#[cfg(test)]
mod tests {
    use super::*;
    use model::{load_scenario, Scenario, Step, Transition};

    #[tokio::test]
    async fn valid_transitions() {
        let scenario = Scenario {
            name: "test".into(),
            initial_state: "start".into(),
            concurrency: None,
            steps: vec![
                Step {
                    name: "step1".into(),
                    method: "GET".into(),
                    url: "http://example.com".into(),
                    transition: Transition {
                        from: "start".into(),
                        to: "middle".into(),
                    },
                    extract: None,
                    retry: None,
                },
                Step {
                    name: "step2".into(),
                    method: "POST".into(),
                    url: "http://example.com".into(),
                    transition: Transition {
                        from: "middle".into(),
                        to: "done".into(),
                    },
                    extract: None,
                    retry: None,
                },
            ],
        };

        let results = run(&scenario).await.unwrap();
        assert_eq!(results.len(), 1);
        let (state, log) = results[0].as_ref().unwrap();
        assert_eq!(state, "done");
        assert_eq!(log.steps.len(), 2);
        assert_eq!(log.steps[0].step_name, "step1");
        assert_eq!(log.steps[0].state_before, "start");
        assert_eq!(log.steps[0].state_after, "middle");
    }

    #[tokio::test]
    async fn invalid_transition() {
        let scenario = Scenario {
            name: "test".into(),
            initial_state: "start".into(),
            concurrency: None,
            steps: vec![
                Step {
                    name: "bad step".into(),
                    method: "GET".into(),
                    url: "http://example.com".into(),
                    transition: Transition {
                        from: "wrong".into(),
                        to: "done".into(),
                    },
                    extract: None,
                    retry: None,
                },
            ],
        };

        let results = run(&scenario).await.unwrap();
        assert_eq!(results.len(), 1);
        assert!(matches!(results[0], Err(RunError::InvalidTransition { .. })));
    }

    #[tokio::test]
    async fn roundtrip_yaml() {
        let yaml = r#"
name: flow
initial_state: init
steps:
  - name: fetch
    method: GET
    url: http://example.com
    transition:
      from: init
      to: fetched
"#;
        let scenario = load_scenario(yaml).unwrap();
        let results = run(&scenario).await.unwrap();
        assert_eq!(results.len(), 1);
    }
}
