//! Run on a disposable `SQLite` backup: opening installs additive query indexes.
use agent_core::{Core, CoreRepository};
use agent_store::SqliteStore;
use std::{sync::Arc, time::Instant};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let path = std::env::args()
        .nth(1)
        .ok_or("provide a disposable database backup")?;
    let store = Arc::new(SqliteStore::open(path)?);
    let core = Core::new(store.clone());
    let snapshot = core.bootstrap().await?;
    let task = &snapshot.tasks.last().ok_or("no tasks")?.task_id;
    for method in [
        "bootstrap",
        "timeline",
        "timeline_snapshot",
        "pending_approvals",
        "event_head",
        "task_execution_state",
        "turn_reviews",
        "refresh_round",
    ] {
        let mut samples = vec![];
        for _ in 0..10 {
            let start = Instant::now();
            match method {
                "bootstrap" => {
                    core.bootstrap().await?;
                }
                "event_head" => {
                    store.event_head().await?;
                }
                "task_execution_state" => {
                    store.task_execution_state(task).await?;
                }
                "turn_reviews" => {
                    core.turn_reviews(task).await?;
                }
                "timeline" => {
                    core.timeline(task).await?;
                }
                "timeline_snapshot" => {
                    core.timeline_snapshot(task).await?;
                }
                "pending_approvals" => {
                    core.pending_approvals(task, 50).await?;
                }
                _ => {
                    core.bootstrap().await?;
                    core.timeline_snapshot(task).await?;
                    core.pending_approvals(task, 50).await?;
                }
            }
            samples.push(start.elapsed().as_secs_f64() * 1000.0);
        }
        samples.sort_by(f64::total_cmp);
        println!(
            "{method}: median={:.3}ms max={:.3}ms",
            samples[5], samples[9]
        );
    }
    Ok(())
}
