//! Known-answer fixtures from Cacophony (via Snow 0.10), NOT production keys.
use serde_json::Value;
use snow::Builder;
fn bytes(v: &Value, field: &str) -> Vec<u8> {
    hex::decode(v[field].as_str().unwrap()).unwrap()
}
#[test]
fn selected_patterns_match_independent_known_answers() {
    let fixture: Value = serde_json::from_str(include_str!("fixtures/noise-vectors.json")).unwrap();
    for vector in fixture["vectors"].as_array().unwrap() {
        let pattern = vector["protocol_name"].as_str().unwrap();
        let is = bytes(vector, "init_static");
        let ie = bytes(vector, "init_ephemeral");
        let ip = bytes(vector, "init_prologue");
        let rs = bytes(vector, "resp_static");
        let re = bytes(vector, "resp_ephemeral");
        let rp = bytes(vector, "resp_prologue");
        let mut initiator = Builder::new(pattern.parse().unwrap())
            .local_private_key(&is)
            .unwrap()
            .prologue(&ip)
            .unwrap()
            .fixed_ephemeral_key_for_testing_only(&ie);
        let mut responder = Builder::new(pattern.parse().unwrap())
            .local_private_key(&rs)
            .unwrap()
            .prologue(&rp)
            .unwrap()
            .fixed_ephemeral_key_for_testing_only(&re);
        let psk: Option<[u8; 32]> =
            vector["init_psks"]
                .as_array()
                .and_then(|p| p.first())
                .map(|p| {
                    hex::decode(p.as_str().unwrap())
                        .unwrap()
                        .try_into()
                        .unwrap()
                });
        if let Some(psk) = &psk {
            initiator = initiator.psk(3, psk).unwrap();
            responder = responder.psk(3, psk).unwrap();
        }
        let mut initiator = initiator.build_initiator().unwrap();
        let mut responder = responder.build_responder().unwrap();
        let messages = vector["messages"].as_array().unwrap();
        let mut output = vec![0; 65535];
        let mut decrypted = vec![0; 65535];
        for (index, message) in messages.iter().take(3).enumerate() {
            let (send, receive) = if index % 2 == 0 {
                (&mut initiator, &mut responder)
            } else {
                (&mut responder, &mut initiator)
            };
            let plain = bytes(message, "payload");
            let expected = bytes(message, "ciphertext");
            let n = send.write_message(&plain, &mut output).unwrap();
            assert_eq!(&output[..n], expected);
            let n = receive.read_message(&expected, &mut decrypted).unwrap();
            assert_eq!(&decrypted[..n], plain);
        }
        assert_eq!(
            hex::encode(initiator.get_handshake_hash()),
            vector["handshake_hash"].as_str().unwrap()
        );
        let mut initiator = initiator.into_transport_mode().unwrap();
        let mut responder = responder.into_transport_mode().unwrap();
        for (index, message) in messages.iter().enumerate().skip(3) {
            let (send, receive) = if index % 2 == 0 {
                (&mut initiator, &mut responder)
            } else {
                (&mut responder, &mut initiator)
            };
            let plain = bytes(message, "payload");
            let expected = bytes(message, "ciphertext");
            let n = send.write_message(&plain, &mut output).unwrap();
            assert_eq!(&output[..n], expected);
            let n = receive.read_message(&expected, &mut decrypted).unwrap();
            assert_eq!(&decrypted[..n], plain);
        }
    }
}
