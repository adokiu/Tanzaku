pub mod pipeline;
pub mod types;

include!(concat!(env!("OUT_DIR"), "/guards.inc.rs"));

pub use pipeline::{GuardEvent, GuardEventHook, GuardPipeline};
pub use types::{Reason, Verdict};

pub fn registered_modules() -> Vec<&'static str> {
    pipeline::registered_modules()
}
