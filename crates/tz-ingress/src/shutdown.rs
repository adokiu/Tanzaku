use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};

use tokio::sync::Notify;

#[derive(Clone)]
pub struct IngressShutdown {
    inner: Arc<ShutdownInner>,
}

struct ShutdownInner {
    stopped: AtomicBool,
    notify: Notify,
}

impl IngressShutdown {
    pub fn new_pair() -> (Self, IngressShutdownHandle) {
        let inner = Arc::new(ShutdownInner {
            stopped: AtomicBool::new(false),
            notify: Notify::new(),
        });
        (
            Self {
                inner: inner.clone(),
            },
            IngressShutdownHandle { inner },
        )
    }

    pub fn is_stopped(&self) -> bool {
        self.inner.stopped.load(Ordering::SeqCst)
    }

    pub async fn cancelled(&self) {
        let notified = self.inner.notify.notified();
        tokio::pin!(notified);
        notified.as_mut().enable();
        if self.is_stopped() {
            return;
        }
        notified.await;
    }
}

#[derive(Clone)]
pub struct IngressShutdownHandle {
    inner: Arc<ShutdownInner>,
}

impl IngressShutdownHandle {
    pub fn stop(&self) {
        if self.inner.stopped.swap(true, Ordering::SeqCst) {
            return;
        }
        self.inner.notify.notify_waiters();
    }
}
