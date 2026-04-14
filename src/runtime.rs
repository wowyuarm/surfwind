pub mod discovery;
pub mod headless;
pub mod rpc;

pub use discovery::{
    discover_runtime, now_iso, resolve_workspace_root, runtime_diagnostics, workspace_id_for_path,
};
pub use headless::{
    cleanup_idle_managed_runtimes, prepare_active_runtime_context, touch_managed_runtime,
};
pub use rpc::{choose_active_port, discover_models, rpc_call, sample_outbound_targets, ActiveRuntimeContext};
