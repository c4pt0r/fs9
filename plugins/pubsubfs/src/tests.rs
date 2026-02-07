use super::*;
use fs9_sdk::OpenFlags;
use fs9_sdk_ffi::FS9_SDK_VERSION;
use std::ptr;

#[test]
fn version_matches_sdk() {
    assert_eq!(ffi::fs9_plugin_version(), FS9_SDK_VERSION);
}

#[test]
fn vtable_not_null() {
    let vtable = ffi::fs9_plugin_vtable();
    assert!(!vtable.is_null());
    unsafe {
        assert_eq!((*vtable).sdk_version, FS9_SDK_VERSION);
    }
}

#[test]
fn provider_lifecycle() {
    unsafe {
        let provider = ffi::create_provider_for_test(ptr::null(), 0);
        assert!(!provider.is_null());
        ffi::destroy_provider_for_test(provider);
    }
}

#[test]
fn create_topic_auto() {
    let provider = PubSubFsProvider::new(PubSubFsConfig::default());

    let handle = provider
        .open(
            "/test_topic",
            OpenFlags {
                write: true,
                ..Default::default()
            },
        )
        .unwrap();

    let topics = provider.topics.read().unwrap();
    assert!(topics.contains_key("test_topic"));

    provider.close(handle.id()).unwrap();
}

#[test]
fn delete_topic() {
    let provider = PubSubFsProvider::new(PubSubFsConfig::default());

    let h = provider
        .open(
            "/test",
            OpenFlags {
                write: true,
                ..Default::default()
            },
        )
        .unwrap();
    provider.close(h.id()).unwrap();

    provider.remove("/test").unwrap();

    let topics = provider.topics.read().unwrap();
    assert!(!topics.contains_key("test"));
}

#[test]
fn publish_and_subscribe() {
    let provider = PubSubFsProvider::new(PubSubFsConfig::default());

    let pub_h = provider
        .open(
            "/chat",
            OpenFlags {
                write: true,
                ..Default::default()
            },
        )
        .unwrap();

    let sub_h = provider
        .open(
            "/chat",
            OpenFlags {
                read: true,
                ..Default::default()
            },
        )
        .unwrap();

    provider.write(pub_h.id(), b"hello world").unwrap();

    let data = provider.read(sub_h.id(), 0, 4096).unwrap();
    assert!(String::from_utf8_lossy(&data).contains("hello world"));

    provider.close(pub_h.id()).unwrap();
    provider.close(sub_h.id()).unwrap();
}

#[test]
fn multiple_subscribers() {
    let provider = PubSubFsProvider::new(PubSubFsConfig::default());

    let pub_h = provider
        .open(
            "/broadcast",
            OpenFlags {
                write: true,
                ..Default::default()
            },
        )
        .unwrap();

    let sub1 = provider
        .open(
            "/broadcast",
            OpenFlags {
                read: true,
                ..Default::default()
            },
        )
        .unwrap();

    let sub2 = provider
        .open(
            "/broadcast",
            OpenFlags {
                read: true,
                ..Default::default()
            },
        )
        .unwrap();

    provider.write(pub_h.id(), b"broadcast message").unwrap();

    let data1 = provider.read(sub1.id(), 0, 4096).unwrap();
    let data2 = provider.read(sub2.id(), 0, 4096).unwrap();

    assert!(String::from_utf8_lossy(&data1).contains("broadcast message"));
    assert!(String::from_utf8_lossy(&data2).contains("broadcast message"));

    provider.close(pub_h.id()).unwrap();
    provider.close(sub1.id()).unwrap();
    provider.close(sub2.id()).unwrap();
}

#[test]
fn topic_info() {
    let provider = PubSubFsProvider::new(PubSubFsConfig::default());

    let h = provider
        .open(
            "/test",
            OpenFlags {
                write: true,
                ..Default::default()
            },
        )
        .unwrap();
    provider.close(h.id()).unwrap();

    let info_h = provider
        .open(
            "/test.info",
            OpenFlags {
                read: true,
                ..Default::default()
            },
        )
        .unwrap();

    let data = provider.read(info_h.id(), 0, 4096).unwrap();
    let info = String::from_utf8_lossy(&data);

    assert!(info.contains("name: test"));
    assert!(info.contains("subscribers:"));
    assert!(info.contains("messages:"));

    provider.close(info_h.id()).unwrap();
}

#[test]
fn list_topics() {
    let provider = PubSubFsProvider::new(PubSubFsConfig::default());

    let h1 = provider
        .open(
            "/topic1",
            OpenFlags {
                write: true,
                ..Default::default()
            },
        )
        .unwrap();
    let h2 = provider
        .open(
            "/topic2",
            OpenFlags {
                write: true,
                ..Default::default()
            },
        )
        .unwrap();
    provider.close(h1.id()).unwrap();
    provider.close(h2.id()).unwrap();

    let entries = provider.readdir("/").unwrap();

    assert!(entries.iter().any(|e| e.path == "/topic1"));
    assert!(entries.iter().any(|e| e.path == "/topic2"));
    assert!(entries.iter().any(|e| e.path == "/topic1.info"));
    assert!(entries.iter().any(|e| e.path == "/topic2.info"));
}

#[test]
fn readdir_root() {
    let provider = PubSubFsProvider::new(PubSubFsConfig::default());

    let h1 = provider
        .open(
            "/chat",
            OpenFlags {
                write: true,
                ..Default::default()
            },
        )
        .unwrap();
    let h2 = provider
        .open(
            "/logs",
            OpenFlags {
                write: true,
                ..Default::default()
            },
        )
        .unwrap();
    provider.close(h1.id()).unwrap();
    provider.close(h2.id()).unwrap();

    let entries = provider.readdir("/").unwrap();

    assert!(entries.iter().any(|e| e.path == "/README"));
    assert!(entries.iter().any(|e| e.path == "/chat"));
    assert!(entries.iter().any(|e| e.path == "/chat.info"));
    assert!(entries.iter().any(|e| e.path == "/logs"));
    assert!(entries.iter().any(|e| e.path == "/logs.info"));
}

#[test]
fn ring_buffer_historical_messages() {
    let config = PubSubFsConfig {
        default_ring_size: 3,
        default_channel_size: 10,
    };
    let provider = PubSubFsProvider::new(config);

    let pub_h = provider
        .open(
            "/test",
            OpenFlags {
                write: true,
                ..Default::default()
            },
        )
        .unwrap();

    provider.write(pub_h.id(), b"msg1").unwrap();
    provider.write(pub_h.id(), b"msg2").unwrap();
    provider.write(pub_h.id(), b"msg3").unwrap();
    provider.write(pub_h.id(), b"msg4").unwrap();

    let sub_h = provider
        .open(
            "/test",
            OpenFlags {
                read: true,
                ..Default::default()
            },
        )
        .unwrap();

    let data = provider.read(sub_h.id(), 0, 4096).unwrap();
    let content = String::from_utf8_lossy(&data);

    assert!(!content.contains("msg1"));
    assert!(content.contains("msg2"));
    assert!(content.contains("msg3"));
    assert!(content.contains("msg4"));

    provider.close(pub_h.id()).unwrap();
    provider.close(sub_h.id()).unwrap();
}

#[test]
fn cannot_open_read_write_simultaneously() {
    let provider = PubSubFsProvider::new(PubSubFsConfig::default());

    let result = provider.open(
        "/test",
        OpenFlags {
            read: true,
            write: true,
            ..Default::default()
        },
    );

    assert!(result.is_err());
}
