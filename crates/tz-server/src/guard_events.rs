use dashmap::DashMap;
use std::{
    sync::{
        atomic::{AtomicU32, Ordering},
        Arc, Mutex,
    },
    time::Duration,
};
use tz_agent::{emit_guard_event, NodeHandle};
use tz_guard::{GuardEvent, GuardEventHook, GuardPipeline};
use tz_proto::GuardEventReport;
use uuid::Uuid;

#[derive(Hash, Eq, PartialEq, Clone)]
struct EventKey {
    rule: String,
    peer: String,
    tunnel_id: Option<Uuid>,
}

struct Pending {
    detail: Mutex<String>,
    hits: AtomicU32,
}

/// 短时聚合防护触发，降低控制面上报频率。
pub fn install_guard_event_hook(handle: NodeHandle, guard: &GuardPipeline) {
    let pending: Arc<DashMap<EventKey, Arc<Pending>>> = Arc::new(DashMap::new());
    let pending_flush = pending.clone();
    let handle_flush = handle.clone();

    tokio::spawn(async move {
        let mut ticker = tokio::time::interval(Duration::from_secs(2));
        loop {
            ticker.tick().await;
            flush_pending(&handle_flush, &pending_flush).await;
        }
    });

    let hook: GuardEventHook = Arc::new(move |event: GuardEvent| {
        let key = EventKey {
            rule: event.rule,
            peer: event.peer.unwrap_or_default(),
            tunnel_id: event.tunnel_id,
        };
        let entry = pending.entry(key).or_insert_with(|| {
            Arc::new(Pending {
                detail: Mutex::new(String::new()),
                hits: AtomicU32::new(0),
            })
        });
        if !event.detail.is_empty() {
            if let Ok(mut slot) = entry.detail.lock() {
                *slot = event.detail;
            }
        }
        entry.hits.fetch_add(1, Ordering::Relaxed);
    });
    guard.set_event_hook(Some(hook));
}

async fn flush_pending(handle: &NodeHandle, pending: &DashMap<EventKey, Arc<Pending>>) {
    let keys: Vec<EventKey> = pending.iter().map(|entry| entry.key().clone()).collect();
    for key in keys {
        let Some((_, entry)) = pending.remove(&key) else {
            continue;
        };
        let hits = entry.hits.swap(0, Ordering::Relaxed);
        if hits == 0 {
            continue;
        }
        let detail = entry
            .detail
            .lock()
            .map(|value| value.clone())
            .unwrap_or_default();
        emit_guard_event(
            handle,
            GuardEventReport {
                rule: key.rule,
                peer: if key.peer.is_empty() {
                    None
                } else {
                    Some(key.peer)
                },
                tunnel_id: key.tunnel_id,
                detail,
                hit_count: hits,
            },
        )
        .await;
    }
}
