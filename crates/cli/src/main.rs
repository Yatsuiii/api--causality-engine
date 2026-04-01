use model::load_scenario;
use runner::{run, ExecutionLog};
use std::fs;

fn replay(file: &str) {
    let json = fs::read_to_string(file).expect("Failed to read log file");
    let logs: Vec<ExecutionLog> = serde_json::from_str(&json).expect("Failed to parse log file");

    println!("[REPLAY] Replaying {} execution(s)...\n", logs.len());
    for (i, log) in logs.iter().enumerate() {
        for step in &log.steps {
            println!(
                "[REPLAY] [User {}] [{}] --{}--> [{}] ✅ ({})",
                i + 1, step.state_before, step.step_name, step.state_after, step.status
            );
        }
    }
    println!("\n[REPLAY] Replay complete.");
}

#[tokio::main]
async fn main() {
    let args: Vec<String> = std::env::args().collect();

    if args.len() >= 3 && args[1] == "--replay" {
        replay(&args[2]);
        return;
    }

    let path = args.get(1).expect("Usage: cli <scenario.yaml> | cli --replay <file>");
    let yaml = fs::read_to_string(path).expect("Failed to read file");
    let scenario = load_scenario(&yaml).expect("Failed to parse YAML");

    println!("Scenario: {}", scenario.name);
    let concurrency = scenario.concurrency.unwrap_or(1);
    println!("Concurrency: {}", concurrency);
    for (i, step) in scenario.steps.iter().enumerate() {
        println!(
            "  Step {}: {} {} {} [{} -> {}]",
            i + 1,
            step.method,
            step.url,
            step.name,
            step.transition.from,
            step.transition.to
        );
    }

    println!("\nRunning...");
    match run(&scenario).await {
        Ok(results) => {
            println!("\nResults:");
            let mut failed = false;
            for (i, result) in results.iter().enumerate() {
                match result {
                    Ok((state, log)) => {
                        println!("  User {}: Final state: {} ({} steps)", i + 1, state, log.steps.len());
                    }
                    Err(e) => {
                        println!("  User {}: Failed: {}", i + 1, e);
                        failed = true;
                    }
                }
            }
            // Collect logs and save to file
            let logs: Vec<_> = results
                .iter()
                .filter_map(|r| r.as_ref().ok().map(|(_, log)| log))
                .collect();
            let json = serde_json::to_string_pretty(&logs).expect("Failed to serialize logs");
            fs::write("execution_log.json", &json).expect("Failed to write execution_log.json");
            println!("\nLogs saved to execution_log.json");

            if failed {
                println!("Some executions failed.");
            } else {
                println!("All executions succeeded.");
            }
        }
        Err(e) => eprintln!("Error: {}", e),
    }
}
