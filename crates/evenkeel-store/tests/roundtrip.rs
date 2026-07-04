//! Live-database round-trip tests. They run only when `DATABASE_URL` is set
//! (local dev / CI with a Postgres service); without it they no-op so the
//! workspace stays green offline.

#![allow(clippy::unwrap_used)]

use evenkeel_core::{Asset, ChannelSnapshot};
use evenkeel_store::Store;

fn snap(channel_id: &str, local: u128, at_ms: u64) -> ChannelSnapshot {
    ChannelSnapshot {
        channel_id: channel_id.into(),
        peer: "02aa".into(),
        asset: Asset::Ckb,
        local_balance: local,
        remote_balance: 1_000,
        offered_tlc_balance: 7,
        received_tlc_balance: 3,
        ready: true,
        at_ms,
    }
}

async fn test_store() -> Option<Store> {
    let url = std::env::var("DATABASE_URL").ok()?;
    Some(Store::connect(&url).await.unwrap())
}

fn unique_node_id(tag: &str) -> String {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    format!("test-{tag}-{nanos}")
}

#[tokio::test]
async fn snapshots_round_trip_including_u128_extremes() {
    let Some(store) = test_store().await else { return };
    let node = unique_node_id("roundtrip");

    let written = vec![
        snap("0xch1", u128::MAX, 1_000),
        snap("0xch1", 42, 2_000),
        snap("0xch2", 0, 1_500),
    ];
    store.insert_snapshots(&node, &written).await.unwrap();

    let read = store.window_since(&node, 0).await.unwrap();
    assert_eq!(read.len(), 3);
    // Ordered by channel then time; u128::MAX survives NUMERIC(39,0) exactly.
    assert_eq!(read[0].local_balance, u128::MAX);
    assert_eq!(read[0].at_ms, 1_000);
    assert_eq!(read[1].local_balance, 42);
    assert_eq!(read[2].channel_id, "0xch2");
    assert_eq!(read[0].offered_tlc_balance, 7);

    assert_eq!(store.latest_at_ms(&node).await.unwrap(), Some(2_000));
}

#[tokio::test]
async fn window_since_filters_by_time() {
    let Some(store) = test_store().await else { return };
    let node = unique_node_id("window");

    store
        .insert_snapshots(&node, &[snap("0xch", 1, 1_000), snap("0xch", 2, 5_000)])
        .await
        .unwrap();
    let recent = store.window_since(&node, 2_000).await.unwrap();
    assert_eq!(recent.len(), 1);
    assert_eq!(recent[0].local_balance, 2);
}

#[tokio::test]
async fn empty_node_has_no_latest() {
    let Some(store) = test_store().await else { return };
    let node = unique_node_id("empty");
    assert_eq!(store.latest_at_ms(&node).await.unwrap(), None);
}
