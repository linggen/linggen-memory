//! `GET /api/health` — cheap probe used by `ling-mem status` and by
//! Linggen core's daemon-supervisor to confirm the daemon is responsive
//! after autostart. Returns the standard envelope (`{ok, data}`).

use axum::Json;
use serde_json::{json, Value};

pub async fn handler() -> Json<Value> {
    Json(json!({
        "ok": true,
        "data": {
            "status": "healthy",
            "version": env!("CARGO_PKG_VERSION"),
        }
    }))
}
