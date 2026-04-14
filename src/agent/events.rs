use anyhow::Result;
use serde_json::Value;

use super::poll::reconcile_and_store_run;
use crate::config::AppConfig;
use crate::runstore::get_run;

pub fn get_agent_events(config: &AppConfig, run_id: &str) -> Result<Option<Vec<Value>>> {
    let Some(record) = get_run(config, run_id)? else {
        return Ok(None);
    };
    Ok(Some(reconcile_and_store_run(config, record).events))
}
