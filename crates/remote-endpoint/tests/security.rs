use dogcodes_endpoint::{
    Session, keypair,
    pairing::{PairingAttempt, PairingBook},
};

fn keys() -> ([u8; 32], [u8; 32]) {
    let pair = keypair().unwrap();
    (
        pair.private.try_into().unwrap(),
        pair.public.try_into().unwrap(),
    )
}
fn handshake(phone: &mut Session, host: &mut Session) {
    host.read_handshake(&phone.write_handshake().unwrap())
        .unwrap();
    phone
        .read_handshake(&host.write_handshake().unwrap())
        .unwrap();
    host.read_handshake(&phone.write_handshake().unwrap())
        .unwrap();
}
fn connected() -> (Session, Session) {
    let (ps, _) = keys();
    let (hs, hp) = keys();
    let mut phone = Session::new(
        true,
        &ps,
        Some(hp),
        Some(&[7; 32]),
        b"fixture-pairing-context",
    )
    .unwrap();
    let mut host =
        Session::new(false, &hs, None, Some(&[7; 32]), b"fixture-pairing-context").unwrap();
    handshake(&mut phone, &mut host);
    (phone, host)
}
#[test]
fn paired_endpoints_encrypt_and_agree_on_channel_binding() {
    let (mut phone, mut host) = connected();
    assert_eq!(phone.binding().unwrap(), host.binding().unwrap());
    let frame = phone.seal(b"task fixture").unwrap();
    assert!(!frame.windows(12).any(|v| v == b"task fixture"));
    assert_eq!(host.open(&frame).unwrap(), b"task fixture");
    assert_eq!(
        phone.open(&host.seal(b"response").unwrap()).unwrap(),
        b"response"
    );
}
#[test]
fn replay_tampering_and_out_of_order_fail_closed() {
    for mode in 0..3 {
        let (mut phone, mut host) = connected();
        let mut first = phone.seal(b"one").unwrap();
        let second = phone.seal(b"two").unwrap();
        match mode {
            0 => {
                host.open(&first).unwrap();
            }
            1 => {
                first[0] ^= 1;
            }
            _ => {
                first = second.clone();
            }
        }
        assert!(host.open(&first).is_err());
        assert!(host.open(&second).is_err());
        assert!(host.authenticated_peer().is_err());
    }
}
#[test]
fn wrong_host_pin_and_psk_and_context_are_rejected() {
    for mode in 0..3 {
        let (ps, _) = keys();
        let (hs, hp) = keys();
        let (_, wrong) = keys();
        let mut phone = Session::new(
            true,
            &ps,
            Some(if mode == 0 { wrong } else { hp }),
            Some(&[7; 32]),
            b"context-a",
        )
        .unwrap();
        let mut host = Session::new(
            false,
            &hs,
            None,
            Some(&[if mode == 1 { 8 } else { 7 }; 32]),
            if mode == 2 {
                b"context-b"
            } else {
                b"context-a"
            },
        )
        .unwrap();
        let result = (|| -> dogcodes_endpoint::Result<()> {
            host.read_handshake(&phone.write_handshake()?)?;
            phone.read_handshake(&host.write_handshake()?)?;
            host.read_handshake(&phone.write_handshake()?)?;
            Ok(())
        })();
        assert!(result.is_err());
        assert!(host.authenticated_peer().is_err());
    }
}
fn attempt(
    book: &PairingBook,
    hs: &[u8; 32],
    hp: [u8; 32],
    ps: &[u8; 32],
    id: &str,
    psk: [u8; 32],
) -> PairingAttempt {
    let mut attempt = book.begin(id, hs, 100).unwrap();
    let mut phone = Session::new(true, ps, Some(hp), Some(&psk), b"context").unwrap();
    handshake(&mut phone, attempt.session());
    attempt
}
#[test]
fn concurrent_claim_first_wins_receipt_survives_restart_revocation_fences_existing_lease() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("pairs.json");
    let mut book = PairingBook::load(&path).unwrap();
    book.issue("invite", [7; 32], b"context".to_vec(), 200, 100)
        .unwrap();
    let (hs, hp) = keys();
    let (ps, _) = keys();
    let (other, _) = keys();
    let mut first = attempt(&book, &hs, hp, &ps, "invite", [7; 32]);
    let second = attempt(&book, &hs, hp, &other, "invite", [7; 32]);
    let grant = book.complete(&first, 110).unwrap();
    assert!(book.complete(&second, 110).is_err());
    assert_eq!(book.complete(&first, 110).unwrap(), grant);
    let mut reopened = PairingBook::load(&path).unwrap();
    assert_eq!(reopened.receipt("invite", first.session()).unwrap(), grant);
    let lease = reopened.authorize(first.session()).unwrap();
    reopened.revoke(&grant.device_id).unwrap();
    assert!(reopened.check(&lease).is_err());
    assert!(reopened.receipt("invite", first.session()).is_err());
    assert!(
        PairingBook::load(&path)
            .unwrap()
            .authorize(first.session())
            .is_err()
    );
}
#[test]
fn expiration_cancel_and_regeneration_reject_inflight_handshake() {
    for mode in 0..3 {
        let directory = tempfile::tempdir().unwrap();
        let mut book = PairingBook::load(directory.path().join("pairs.json")).unwrap();
        book.issue("invite", [7; 32], b"context".to_vec(), 200, 100)
            .unwrap();
        let (hs, hp) = keys();
        let (ps, _) = keys();
        let old = attempt(&book, &hs, hp, &ps, "invite", [7; 32]);
        if mode == 1 {
            book.cancel_invitation();
        }
        if mode == 2 {
            book.issue("invite", [8; 32], b"context".to_vec(), 200, 100)
                .unwrap();
        }
        assert!(
            book.complete(&old, if mode == 0 { 200 } else { 110 })
                .is_err()
        );
    }
}
#[test]
fn reconnect_requires_both_pins_and_regrant_does_not_revive_old_lease_or_receipt() {
    let directory = tempfile::tempdir().unwrap();
    let mut book = PairingBook::load(directory.path().join("pairs.json")).unwrap();
    let (hs, hp) = keys();
    let (ps, pp) = keys();
    book.issue("one", [7; 32], b"context".to_vec(), 200, 100)
        .unwrap();
    let mut first = attempt(&book, &hs, hp, &ps, "one", [7; 32]);
    let grant = book.complete(&first, 110).unwrap();
    let lease = book.authorize(first.session()).unwrap();
    book.revoke(&grant.device_id).unwrap();
    book.issue("two", [8; 32], b"context".to_vec(), 200, 100)
        .unwrap();
    let second = attempt(&book, &hs, hp, &ps, "two", [8; 32]);
    book.complete(&second, 110).unwrap();
    assert!(book.check(&lease).is_err());
    assert!(book.receipt("one", first.session()).is_err());
    let mut phone = Session::new(true, &ps, Some(hp), None, b"reconnect").unwrap();
    let mut host = Session::new(false, &hs, Some(pp), None, b"reconnect").unwrap();
    handshake(&mut phone, &mut host);
    assert!(book.authorize(&host).is_ok());
    assert!(Session::new(false, &hs, None, None, b"reconnect").is_err());
}
#[test]
fn journal_corruption_and_incomplete_write_fail_closed() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("pairs.json");
    std::fs::write(&path, b"bad json").unwrap();
    assert!(PairingBook::load(&path).is_err());
    std::fs::remove_file(&path).unwrap();
    std::fs::write(path.with_extension("pending"), b"partial").unwrap();
    assert!(PairingBook::load(&path).is_err());
}

#[test]
fn failed_revoke_write_disables_existing_authorization() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("pairs.json");
    let mut book = PairingBook::load(&path).unwrap();
    let (hs, hp) = keys();
    let (ps, _) = keys();
    book.issue("invite", [7; 32], b"context".to_vec(), 200, 100)
        .unwrap();
    let mut candidate = attempt(&book, &hs, hp, &ps, "invite", [7; 32]);
    let grant = book.complete(&candidate, 110).unwrap();
    let lease = book.authorize(candidate.session()).unwrap();
    std::fs::write(
        path.with_extension("pending"),
        b"simulated interrupted write",
    )
    .unwrap();
    assert!(book.revoke(&grant.device_id).is_err());
    assert!(book.check(&lease).is_err());
    assert!(book.authorize(candidate.session()).is_err());
    assert!(PairingBook::load(&path).is_err());
}
