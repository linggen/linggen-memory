//! Background storage maintenance — the scheduler half.
//!
//! `memory::maintenance` decides *whether* a table needs work; this decides
//! *when* to ask, and runs the pass. It is deliberately part of the daemon
//! rather than of any host: `ling-mem` also serves Claude Code, Codex and
//! OpenClaw, where nobody ever runs a dream and nothing would ever call in.
//! A store that only stays healthy when some other process remembers to
//! maintain it is not maintained.
//!
//! It is also deliberately **silent**. Compaction is storage plumbing, like
//! a WAL checkpoint — there is no decision for the user in it and nothing
//! they could act on, so it never reaches a report, a status line, or a
//! prompt. Silent is not untraceable: every pass logs what it moved, and
//! the disk figure `stats` already publishes simply gets smaller.
//!
//! ## Why a condition and not a calendar
//!
//! Bloat is driven by writes, not by time, so a pure schedule is wrong at
//! both ends: a bulk import can shred the store within an hour of a run,
//! while a quiet month triggers a full rewrite that reclaims nothing.
//! Checking is nearly free — a few thousand `stat` calls — so we check
//! often and act only on the measured condition. Upstream guidance says
//! the same thing, loosely: run "after large writes or on a schedule".

use crate::http::state::SharedState;
use crate::memory::maintenance::{self, Footprint, Report, PRUNE_OLDER_THAN_DAYS};
use crate::memory::MemoryStore;
use std::sync::Arc;
use std::time::Duration;

/// How often to measure. Cheap enough to do hourly; frequent enough that a
/// heavy import is picked up the same day rather than weeks later.
const CHECK_EVERY: Duration = Duration::from_secs(60 * 60);

/// Wait this long after startup before the first check, so a daemon that
/// was launched to serve one urgent request answers it first.
const FIRST_CHECK_DELAY: Duration = Duration::from_secs(90);

/// Require this much quiet before rewriting a table. Compaction holds the
/// table's write lock, so running it under load would stall a recall — and
/// a rewrite that lands mid-turn is exactly the surprise this design is
/// supposed to avoid.
const IDLE_FOR: Duration = Duration::from_secs(5 * 60);

/// Spawn the maintenance loop. Returns immediately; the task lives as long
/// as the daemon does and is dropped with it on shutdown.
pub fn spawn(state: SharedState) {
    tokio::spawn(async move {
        tokio::time::sleep(FIRST_CHECK_DELAY).await;
        let mut ticker = tokio::time::interval(CHECK_EVERY);
        // The first tick fires instantly; we already slept for it.
        ticker.tick().await;
        loop {
            run_if_due(&state).await;
            ticker.tick().await;
        }
    });
}

/// One evaluation: measure both tables, maintain whichever is due.
///
/// Never returns an error — a failed pass logs and leaves the store exactly
/// as it was. Maintenance falling behind is a slow disk problem; a daemon
/// that dies because compaction failed is an outage.
async fn run_if_due(state: &SharedState) {
    if !idle_long_enough(state) {
        tracing::debug!("maintenance: skipped, daemon busy");
        return;
    }

    for store in [&state.store, &state.episodic] {
        maintain_table(state, store).await;
    }
}

/// Whether the daemon has been quiet long enough to rewrite tables.
fn idle_long_enough(state: &SharedState) -> bool {
    state.idle_for() >= IDLE_FOR
}

/// Measure one table, and if it is due, compact + prune it.
async fn maintain_table(state: &SharedState, store: &Arc<MemoryStore>) {
    let table = store.table_name().to_string();

    let before = match footprint(state, store).await {
        Some(f) => f,
        None => return,
    };
    if !before.maintenance_due() {
        tracing::debug!(
            table = %table,
            rows = before.rows,
            fragments = before.fragments,
            "maintenance: not due"
        );
        return;
    }

    tracing::info!(
        table = %table,
        rows = before.rows,
        fragments = before.fragments,
        versions = before.versions,
        mb = before.total_bytes() / 1_048_576,
        "maintenance: starting"
    );

    let prune_older_than = chrono::Duration::days(PRUNE_OLDER_THAN_DAYS);
    if let Err(e) = store.optimize_storage(prune_older_than).await {
        tracing::warn!(table = %table, error = %e, "maintenance: failed");
        return;
    }

    let Some(after) = footprint(state, store).await else {
        tracing::info!(table = %table, "maintenance: done");
        return;
    };
    let report = Report { before, after };
    tracing::info!(table = %table, "maintenance: done — {report}");
}

/// Current footprint of a table, or `None` if it can't be read — in which
/// case we decline to act rather than guess at whether a rewrite is safe.
async fn footprint(state: &SharedState, store: &Arc<MemoryStore>) -> Option<Footprint> {
    let rows = match store.count().await {
        Ok(n) => n,
        Err(e) => {
            tracing::warn!(error = %e, "maintenance: could not count rows");
            return None;
        }
    };
    let (fragments, small) = match store.fragment_stats().await {
        Ok(s) => s,
        Err(e) => {
            tracing::warn!(error = %e, "maintenance: could not read fragment stats");
            return None;
        }
    };
    Some(maintenance::measure(
        &state.data_dir,
        store.table_name(),
        rows as u64,
        fragments as u64,
        small as u64,
    ))
}
