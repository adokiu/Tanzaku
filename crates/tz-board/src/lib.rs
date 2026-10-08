pub mod api;
pub mod page;
mod carrier_secret;
pub mod config;
mod pki;
mod quota;
pub mod port_cache;
pub mod setup;
pub mod theme;
mod client_traffic;
mod tunnel_traffic;
mod geoip;
mod node_metrics;
mod scheduler;
mod stats;
mod subscription_period;
mod store;
mod system_settings;
mod guard_policy;
mod guard_events;
pub mod ws;

pub fn start_background_tasks(state: std::sync::Arc<setup::AppState>) {
    stats::spawn_ingress_worker(state.clone());
    stats::spawn_settlement_task(state.clone());
    scheduler::spawn(state);
}
