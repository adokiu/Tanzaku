mod frame;
mod session;
mod stream;

pub use frame::{decode_open, encode_open, FrameType, MuxFrame};
pub use session::{MuxSession, Role};
pub use stream::MuxStream;

/// 单条 mux 数据帧 payload 上限（读写各侧队列深度 × 本值 ≈ 单流缓冲预算）。
pub const MAX_MUX_PAYLOAD: usize = 16 * 1024;
/// 每个 mux 流上未读 DATA 帧数量上限（背压：满则暂停读 TLS/QUIC）。
pub const MUX_STREAM_READ_QUEUE: usize = 4;
/// 会话级 outbound 命令队列深度（Open/Data/Fin/Dgram 共享）。
pub const MUX_COMMAND_QUEUE: usize = 16;
/// 待 accept 的 Open 流数量上限。
pub const MUX_ACCEPT_QUEUE: usize = 16;
/// 未读 datagram 数量上限。
pub const MUX_DATAGRAM_QUEUE: usize = 8;
/// 解码缓冲上限，防止半包攻击撑爆内存。
pub const MUX_DECODE_BUFFER: usize = 64 * 1024;
/// 单次 read(2)  scratch 大小。
pub const MUX_IO_SCRATCH: usize = 16 * 1024;
