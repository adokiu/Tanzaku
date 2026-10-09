use dashmap::DashMap;
use std::{
    sync::{
        atomic::{AtomicU32, Ordering},
        Arc,
    },
    time::Duration,
};
use tz_agent::{emit_guard_event, NodeHandle};
use tz_guard::{GuardEvent, GuardEventHook, GuardPipeline};
use tz_proto::GuardEventReport;
use uuid::Uuid;

/// 攻击类：新建聚合键后尽快上报，便于 board 立刻暂停。
const ATTACK_RULES: &[&str] = &["udp_amplify", "per_ip_limit", "per_tunnel_ip_limit"];

#[derive(Hash, Eq, PartialEq, Clone)]
struct EventKey {
    rule: String,
    tunnel_id: Option<Uuid>,
}

/// 短时聚合防护状态：控制面只上报类型 + 强度，不携带 peer / detail 等业务原始数据。
pub fn install_guard_event_hook(handle: NodeHandle, guard: &GuardPipeline) {
    let pending: Arc<DashMap<EventKey, AtomicU32>> = Arc::new(DashMap::new());
    let pending_flush = pending.clone();
    let handle_flush = handle.clone();

    tokio::spawn(async move {
        let mut ticker = tokio::time::interval(Duration::from_secs(2));
        loop {
            ticker.tick().await;
            flush_all(&handle_flush, &pending_flush).await;
        }
    });

    let pending_hook = pending.clone();
    let handle_early = handle.clone();
    let hook: GuardEventHook = Arc::new(move |event: GuardEvent| {
        let key = EventKey {
            rule: event.rule,
            tunnel_id: event.tunnel_id,
        };
        let is_attack = ATTACK_RULES.iter().any(|name| *name == key.rule.as_str());
        let is_new = !pending_hook.contains_key(&key);
        pending_hook
            .entry(key.clone())
            .or_insert_with(|| AtomicU32::new(0))
            .fetch_add(1, Ordering::Relaxed);
        // 攻击首次出现：短延迟后立刻冲刷该键，避免等满 2s 窗口。
        if is_attack && is_new {
            let pending = pending_hook.clone();
            let handle = handle_early.clone();
            tokio::spawn(async move {
                tokio::time::sleep(Duration::from_millis(50)).await;
                flush_one(&handle, &pending, key).await;
            });
        }
    });
    guard.set_event_hook(Some(hook));
}

async fn flush_all(handle: &NodeHandle, pending: &DashMap<EventKey, AtomicU32>) {
    let keys: Vec<EventKey> = pending.iter().map(|entry| entry.key().clone()).collect();
    for key in keys {
        flush_one(handle, pending, key).await;
    }
}

async fn flush_one(handle: &NodeHandle, pending: &DashMap<EventKey, AtomicU32>, key: EventKey) {
    let Some((_, counter)) = pending.remove(&key) else {
        return;
    };
    let intensity = counter.swap(0, Ordering::Relaxed);
    if intensity == 0 {
        return;
    }
    emit_guard_event(
        handle,
        GuardEventReport {
            rule: key.rule,
            tunnel_id: key.tunnel_id,
            intensity,
        },
    )
    .await;
}
