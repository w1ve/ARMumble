//! Unit tests for the video track plumbing.

#![allow(clippy::unwrap_used, reason = "unwrap is acceptable in test code")]
#![allow(clippy::expect_used, reason = "expect is acceptable in test code")]

use super::*;

#[test]
fn sender_exposes_its_track() {
    let sender = VideoSender::new(12345).expect("create video sender");
    // Two calls must hand out the same underlying track, not clones of
    // different tracks, or samples would go to a track nobody is sending.
    let a = sender.track();
    let b = sender.track();
    assert!(Arc::ptr_eq(&a, &b));
}

#[test]
fn payload_type_is_in_the_dynamic_range() {
    // Values outside 96-127 are not valid dynamic payload types and would be
    // rejected or misinterpreted by the peer.
    assert!(
        (96..=127).contains(&H264_PAYLOAD_TYPE),
        "H264 payload type {H264_PAYLOAD_TYPE} must be a dynamic type"
    );
}

#[tokio::test]
async fn receiver_queues_and_drains_in_order() {
    let rx = VideoReceiver::new();
    assert_eq!(rx.pending_count().await, 0);

    rx.push_access_unit(vec![1, 2, 3]).await;
    rx.push_access_unit(vec![4, 5]).await;
    assert_eq!(rx.pending_count().await, 2);

    let taken = rx.take_access_units().await;
    assert_eq!(taken, vec![vec![1, 2, 3], vec![4, 5]]);
    assert_eq!(
        rx.pending_count().await,
        0,
        "taking must drain the queue so frames are not decoded twice"
    );
}

#[tokio::test]
async fn draining_an_empty_receiver_is_harmless() {
    let rx = VideoReceiver::new();
    assert!(rx.take_access_units().await.is_empty());
}
