use ionconnect_protocol::{
    Authentication, ClipboardMime, ClipboardSync, Disconnect, DisplayGeometry, Heartbeat,
    KeyboardPress, KeyboardRelease, Message, MouseButton, MouseClick, MouseMove, Reconnect,
    UdpHello, Version, decode_message, encode_message,
};
use ionconnect_shared::{DeviceId, KeyModifiers};

fn assert_round_trips(message: &Message) {
    let encoded = encode_message(message).expect("encode no debería fallar");
    let decoded = decode_message(&encoded).expect("decode no debería fallar");
    assert_eq!(&decoded, message);
}

#[test]
fn heartbeat_round_trips() {
    assert_round_trips(&Message::Heartbeat(Heartbeat { sequence: 42 }));
}

#[test]
fn mouse_move_round_trips_with_negative_coordinates() {
    assert_round_trips(&Message::MouseMove(MouseMove { x: -120, y: 800 }));
}

#[test]
fn mouse_click_round_trips() {
    assert_round_trips(&Message::MouseClick(MouseClick {
        button: MouseButton::Forward,
        pressed: true,
        x: 10,
        y: 20,
    }));
}

#[test]
fn keyboard_press_and_release_round_trip() {
    let modifiers = KeyModifiers::CTRL | KeyModifiers::ALT_GR;
    assert_round_trips(&Message::KeyboardPress(KeyboardPress {
        keycode: 65,
        modifiers,
    }));
    assert_round_trips(&Message::KeyboardRelease(KeyboardRelease {
        keycode: 65,
        modifiers,
    }));
}

#[test]
fn clipboard_sync_round_trips() {
    assert_round_trips(&Message::ClipboardSync(ClipboardSync {
        mime: ClipboardMime::Text,
        data: b"hola mundo".to_vec(),
    }));
}

#[test]
fn authentication_round_trips() {
    assert_round_trips(&Message::Authentication(Authentication {
        device_id: DeviceId::new(),
        device_name: "escritorio-oficina".to_string(),
        protocol_version: 1,
        cert_fingerprint: [7u8; 32],
    }));
}

#[test]
fn disconnect_round_trips() {
    assert_round_trips(&Message::Disconnect(Disconnect {
        code: 3,
        reason: "fingerprint no coincide".to_string(),
    }));
}

#[test]
fn reconnect_round_trips() {
    assert_round_trips(&Message::Reconnect(Reconnect {
        session_nonce: 0xdead_beef_cafe_1234,
    }));
}

#[test]
fn version_round_trips() {
    assert_round_trips(&Message::Version(Version {
        major: 1,
        minor: 2,
        patch: 3,
    }));
}

#[test]
fn display_geometry_round_trips() {
    assert_round_trips(&Message::DisplayGeometry(DisplayGeometry {
        width: 1366,
        height: 768,
    }));
}

#[test]
fn udp_hello_round_trips() {
    assert_round_trips(&Message::UdpHello(UdpHello { port: 51820 }));
}

#[test]
fn decode_rejects_empty_payload() {
    let err = decode_message(&[]).unwrap_err();
    assert!(matches!(err, ionconnect_protocol::ProtocolError::Empty));
}

#[test]
fn decode_rejects_unknown_message_type() {
    let err = decode_message(&[250]).unwrap_err();
    assert!(matches!(
        err,
        ionconnect_protocol::ProtocolError::UnknownMessageType(250)
    ));
}

#[test]
fn decode_rejects_truncated_mouse_move() {
    // Tipo MouseMove (1) pero solo 2 de los 8 bytes de payload esperados.
    let err = decode_message(&[1, 0, 0]).unwrap_err();
    assert!(matches!(
        err,
        ionconnect_protocol::ProtocolError::Truncated {
            expected: 8,
            remaining: 2
        }
    ));
}
