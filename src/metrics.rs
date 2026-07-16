use std::sync::LazyLock;

use prometheus::{
    register_counter, register_counter_vec, register_gauge, Counter, CounterVec, Gauge,
};

pub static PIXELS_PAINTED: LazyLock<Counter> = LazyLock::new(|| {
    register_counter!(
        "gambas_pixels_painted_total",
        "Pixels committed through Raft"
    )
    .unwrap()
});

pub static PAINT_REJECTED: LazyLock<CounterVec> = LazyLock::new(|| {
    register_counter_vec!(
        "gambas_paint_rejected_total",
        "Paint requests rejected before commit",
        &["reason"]
    )
    .unwrap()
});

pub static WS_CLIENTS: LazyLock<Gauge> = LazyLock::new(|| {
    register_gauge!("gambas_ws_clients", "Currently connected WebSocket clients").unwrap()
});
