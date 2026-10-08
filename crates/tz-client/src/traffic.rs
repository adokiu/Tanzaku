use dashmap::DashMap;
use std::collections::HashSet;
use std::sync::Arc;
use tz_net::meter::TrafficMeter;
use uuid::Uuid;

#[derive(Debug)]
pub struct TunnelTraffic {
    pub meter: TrafficMeter,
}

#[derive(Debug, Default)]
pub struct ClientTrafficRegistry {
    by_tunnel: DashMap<Uuid, Arc<TunnelTraffic>>,
}

impl ClientTrafficRegistry {
    pub fn ensure(&self, tunnel_id: Uuid) -> Arc<TunnelTraffic> {
        self.by_tunnel
            .entry(tunnel_id)
            .or_insert_with(|| {
                Arc::new(TunnelTraffic {
                    meter: TrafficMeter::default(),
                })
            })
            .clone()
    }

    pub fn retain_active(&self, active: &HashSet<Uuid>) {
        self.by_tunnel.retain(|tunnel_id, _| active.contains(tunnel_id));
    }

    pub fn totals(&self) -> (u64, u64) {
        let mut bytes_in = 0_u64;
        let mut bytes_out = 0_u64;
        for entry in self.by_tunnel.iter() {
            let snap = entry.meter.snapshot();
            bytes_in = bytes_in.saturating_add(snap.left_to_right);
            bytes_out = bytes_out.saturating_add(snap.right_to_left);
        }
        (bytes_in, bytes_out)
    }

    pub fn take_interval_delta(&self) -> (u64, u64) {
        let mut bytes_in = 0_u64;
        let mut bytes_out = 0_u64;
        for entry in self.by_tunnel.iter() {
            let delta = entry.meter.take_delta();
            bytes_in = bytes_in.saturating_add(delta.left_to_right);
            bytes_out = bytes_out.saturating_add(delta.right_to_left);
        }
        (bytes_in, bytes_out)
    }
}
