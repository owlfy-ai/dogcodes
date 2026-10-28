//! Ephemeral desktop lease migration. The capture/controller and input sequence
//! remain authoritative on PC; only the authenticated physical owner changes.
use super::{Arc, Connection, Duration, Instant, Lease, Ordering, Value, json};
use std::sync::{Mutex, Weak};

#[derive(Clone)]
pub(super) struct Liveness(Arc<Mutex<Owners>>);
struct Owners {
    current: Weak<Connection>,
    staged: Option<(Weak<Connection>, Instant)>,
}
impl Liveness {
    pub fn new(owner: &Arc<Connection>) -> Self {
        Self(Arc::new(Mutex::new(Owners {
            current: Arc::downgrade(owner),
            staged: None,
        })))
    }
    fn owners(&self) -> std::sync::MutexGuard<'_, Owners> {
        self.0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
    pub fn alive(&self) -> bool {
        let owners = self.owners();
        owners.current.upgrade().is_some_and(|c| c.alive())
            || owners.staged.as_ref().is_some_and(|(owner, until)| {
                Instant::now() < *until && owner.upgrade().is_some_and(|c| c.alive())
            })
    }
    fn stage(&self, owner: &Arc<Connection>, until: Instant) {
        self.owners().staged = Some((Arc::downgrade(owner), until));
    }
    fn cancel(&self) {
        self.owners().staged = None;
    }
    fn adopt(&self, owner: &Arc<Connection>) {
        let mut owners = self.owners();
        owners.current = Arc::downgrade(owner);
        owners.staged = None;
    }
}

pub(super) struct Ticket {
    id: String,
    offered_by: String,
    from_revision: u64,
    source_instance: Value,
    source_revision: Value,
    peer: [u8; 32],
    until: Instant,
    staged: Option<Arc<Connection>>,
    applied: bool,
    transport: Option<String>,
}
fn ready(lease: &Lease) -> Result<(), String> {
    if !lease
        .video
        .as_ref()
        .is_some_and(super::super::video::Session::migration_ready)
    {
        return Err("当前视频正在调整或不支持原地迁移".into());
    }
    #[cfg(target_os = "linux")]
    if let Some(portal) = &lease.portal {
        portal.status()?;
    }
    Ok(())
}
fn describe(lease: &Lease, state: &str) -> Value {
    let mut video = lease
        .video
        .as_ref()
        .map_or(Value::Null, super::super::video::Session::describe);
    if !video.is_null() {
        video["transport"] = json!(if lease.relay.is_some() {
            "binary"
        } else {
            "webrtc"
        });
        video["clientReadyRequired"] = json!(true);
        if let Some(target) = lease
            .handoff
            .as_ref()
            .filter(|t| !t.applied)
            .and_then(|t| t.transport.as_deref())
        {
            let profile = if target == "webrtc" {
                "cb31"
            } else {
                "cb-auto"
            };
            video["switchingCodec"] = json!(video["profile"] != profile);
            video["profile"] = json!(profile);
            video["transport"] = json!(target);
        }
    }
    json!({"state":state,"leaseId":lease.id,"monitorId":lease.display,
        "inputSequence":lease.sequence.to_string(),"revision":lease.handoff_revision.to_string(),"inputBatch":super::MAX_INPUT_BATCH,"video":video})
}
/// Expiry is bounded even when the candidate stays connected but never claims.
pub(super) fn expire(lease: &mut Lease) {
    if lease
        .handoff
        .as_ref()
        .is_some_and(|t| !t.applied && Instant::now() >= t.until)
    {
        lease.handoff = None;
        lease.liveness.cancel();
        if let Some(video) = &lease.video {
            let _ = video.hold(None);
        }
        if let Some(relay) = &lease.relay {
            relay.hold(None);
        }
    }
}
pub(super) fn call(
    lease: &mut Lease,
    connection: Arc<Connection>,
    args: Value,
) -> Result<Value, String> {
    let physical = connection.id.clone();
    inner(lease, connection, args).map(|mut value| {
        value["owner"] = json!(lease.owner.id == physical);
        value
    })
}
fn inner(lease: &mut Lease, connection: Arc<Connection>, mut args: Value) -> Result<Value, String> {
    expire(lease);
    if args["leaseId"] != lease.id || !connection.alive() {
        return Err("远控租约已结束，请重新连接".into());
    }
    let id = args["ticket"]
        .as_str()
        .ok_or("缺少迁移请求身份")?
        .to_owned();
    let parsed = uuid::Uuid::parse_str(&id).map_err(|_| "迁移请求身份无效")?;
    if parsed.get_version_num() != 4 {
        return Err("迁移请求身份无效".into());
    }
    let peer = connection.peer.ok_or("迁移需要已认证的手机连接")?;
    let phase = args["phase"].as_str().ok_or("缺少迁移阶段")?.to_owned();
    if !["offer", "stage", "claim", "status", "cancel"].contains(&phase.as_str()) {
        return Err("迁移阶段无效".into());
    }
    if phase == "offer" {
        if lease.owner.id != connection.id {
            return Err("远控租约不属于当前连接".into());
        }
        super::super::parse_sequence(args.get_mut("expectedRevision"))?;
        let expected = args["expectedRevision"].as_u64().ok_or("缺少迁移版本")?;
        if let Some(ticket) = &lease.handoff
            && ticket.id == id
        {
            if ticket.from_revision != expected {
                return Err("迁移请求身份冲突".into());
            }
            return Ok(describe(
                lease,
                if ticket.applied {
                    "applied"
                } else if ticket.staged.is_some() {
                    "staged"
                } else {
                    "offered"
                },
            ));
        }
        if expected != lease.handoff_revision {
            return Err("迁移版本已变化，旧请求不能重新生效".into());
        }
        ready(lease)?;
        if lease.handoff.as_ref().is_some_and(|t| !t.applied) {
            return Err("已有视频连接正在迁移".into());
        }
        let revision = lease
            .handoff_revision
            .checked_add(1)
            .ok_or("请重新开启远控租约")?;
        lease.handoff = Some(Ticket {
            id,
            offered_by: connection.id.clone(),
            from_revision: expected,
            source_instance:
                lease.video.as_ref().ok_or("视频画面未开启")?.describe()["resize"]["instanceId"]
                    .clone(),
            source_revision:
                lease.video.as_ref().ok_or("视频画面未开启")?.describe()["resize"]["applied"]
                    .clone(),
            peer,
            until: Instant::now() + Duration::from_secs(20),
            staged: None,
            applied: false,
            transport: None,
        });
        lease.handoff_revision = revision;
        return Ok(describe(lease, "offered"));
    }
    // Absence is not a claim that the historical operation never ran. The
    // counter fences late offers; this snapshot proves only whether this
    // physical connection owns the current lease and whether the token retired.
    if matches!(phase.as_str(), "status" | "cancel")
        && connection.peer == lease.owner.peer
        && lease.handoff.as_ref().is_none_or(|t| t.id != id)
    {
        if phase == "cancel" {
            super::super::parse_sequence(args.get_mut("expectedRevision"))?;
            let expected = args["expectedRevision"].as_u64().ok_or("缺少迁移版本")?;
            if expected > lease.handoff_revision {
                return Err("迁移版本无效".into());
            }
            if expected == lease.handoff_revision {
                if lease.owner.id != connection.id {
                    return Err("请从当前 owner 结束尚未受理的迁移".into());
                }
                lease.handoff_revision = expected.checked_add(1).ok_or("请重新开启远控租约")?;
            }
        }
        return Ok(describe(lease, "retired"));
    }
    let ticket = lease
        .handoff
        .as_ref()
        .filter(|t| t.id == id && t.peer == peer)
        .ok_or("迁移请求已结束或不属于当前手机")?;
    let offered_by = ticket.offered_by == connection.id;
    let candidate = ticket
        .staged
        .as_ref()
        .is_some_and(|c| c.id == connection.id);
    if ticket.applied {
        return if offered_by || candidate {
            Ok(describe(lease, "applied"))
        } else {
            Err("迁移请求不属于当前连接".into())
        };
    }
    if matches!(phase.as_str(), "stage" | "claim") {
        let description = lease.video.as_ref().ok_or("视频画面未开启")?.describe();
        if description["resize"]["instanceId"] != ticket.source_instance
            || description["resize"]["applied"] != ticket.source_revision
        {
            return Err("屏幕或视频配置已变化，请重新准备迁移".into());
        }
    }
    if phase == "stage" {
        if offered_by {
            return Err("迁移需要新的认证连接".into());
        }
        if ticket.staged.is_some() && !candidate {
            return Err("另一条连接已准备本次迁移".into());
        }
        if args["chunkedMessages"] != true {
            return Err("新连接未声明分片接收能力".into());
        }
        let transport = args["videoTransport"]
            .as_str()
            .unwrap_or(if lease.relay.is_some() {
                "binary"
            } else {
                "webrtc"
            });
        if !matches!(transport, "binary" | "webrtc") {
            return Err("不支持的视频传输类型".into());
        }
        if ticket
            .transport
            .as_deref()
            .is_some_and(|saved| saved != transport)
        {
            return Err("迁移目标类型不可更换".into());
        }
        if transport == "webrtc" && connection.video.is_none() {
            return Err("新连接未协商兼容的视频轨道".into());
        }
        let target_transport = transport.to_owned();
        ready(lease)?;
        let until = ticket.until;
        connection.chunked.store(true, Ordering::Release);
        lease.liveness.stage(&connection, until);
        let ticket = lease.handoff.as_mut().ok_or("迁移请求已结束")?;
        ticket.staged = Some(connection);
        ticket.transport = Some(target_transport);
        if let Some(relay) = &lease.relay {
            relay.hold(Some(until));
        }
        if let Some(video) = &lease.video {
            video.hold(Some(until))?;
        }
        lease.expires = until;
        return Ok(describe(lease, "staged"));
    }
    if !offered_by && !candidate {
        return Err("迁移请求不属于当前连接".into());
    }
    match phase.as_str() {
        "status" => Ok(describe(
            lease,
            if ticket.staged.is_some() {
                "staged"
            } else {
                "offered"
            },
        )),
        "cancel" => {
            lease.handoff = None;
            lease.liveness.cancel();
            if let Some(video) = &lease.video {
                let _ = video.hold(None);
            }
            if let Some(relay) = &lease.relay {
                relay.hold(None);
            }
            Ok(describe(lease, "canceled"))
        }
        "claim" => {
            if !candidate {
                return Err("请先在新连接准备迁移".into());
            }
            super::super::parse_sequence(args.get_mut("expectedInputSequence"))?;
            if args["expectedInputSequence"].as_u64() != Some(lease.sequence) {
                return Err("旧连接仍有输入尚未确认，不能迁移".into());
            }
            ready(lease)?;
            // All validation precedes effects. Native worker serializes this
            // swap against every input and all later requests on the old wire.
            let video = lease.video.as_ref().ok_or("视频画面未开启")?;
            let target = lease
                .handoff
                .as_ref()
                .and_then(|t| t.transport.as_deref())
                .ok_or("迁移目标尚未准备")?;
            if target == "binary" && lease.relay.is_some() {
                let relay = lease.relay.as_ref().ok_or("视频输出不可用")?;
                let generation =
                    relay.rebind(connection.binary.clone(), connection.video_epoch.clone());
                let _ = video.hold(None);
                video.migrated(generation);
            } else if target == "binary" {
                let generation = video.describe()["generation"]
                    .as_u64()
                    .ok_or("视频代次不可用")?;
                connection
                    .video_epoch
                    .fetch_max(generation, Ordering::AcqRel);
                let relay = Arc::new(super::super::video::RelayVideo::with_epoch(
                    connection.binary.clone(),
                    lease.id.clone(),
                    connection.video_epoch.clone(),
                ));
                relay.defer_until_ready();
                video.replace_output(relay.clone())?;
                lease.relay = Some(relay);
            } else {
                let sender = connection.video.clone().ok_or("新连接未协商视频轨道")?;
                video.replace_output(Arc::new(sender))?;
                lease.relay = None;
            }
            lease.liveness.adopt(&connection);
            lease.owner = connection;
            lease.expires = Instant::now() + Duration::from_secs(15);
            lease.handoff.as_mut().ok_or("迁移请求已结束")?.applied = true;
            Ok(describe(lease, "applied"))
        }
        _ => Err("迁移阶段无效".into()),
    }
}

#[cfg(test)]
mod tests {
    use super::super::super::video::{RelayVideo, Session, VideoOut};
    use super::super::{Disconnect, Worker};
    use super::*;
    fn connection(peer: u8) -> (Disconnect, tokio::sync::mpsc::Receiver<(Value, Vec<u8>)>) {
        let (tx, rx) = tokio::sync::mpsc::channel(8);
        (Disconnect::authenticated(None, tx, [peer; 32]), rx)
    }
    fn lease(owner: &Disconnect) -> Lease {
        let relay = Arc::new(RelayVideo::with_epoch(
            owner.0.binary.clone(),
            "lease".into(),
            owner.0.video_epoch.clone(),
        ));
        Lease {
            id: "lease".into(),
            liveness: Liveness::new(&owner.0),
            handoff: None,
            handoff_revision: 0,
            owner: owner.0.clone(),
            display: 1,
            expires: Instant::now() + Duration::from_secs(15),
            sequence: 42,
            controller: None,
            cursor: dogcodes_remote_desktop::cursor::Reader::default(),
            last_frame: None,
            capturer: None,
            video: Some(Session::fixture(relay.clone())),
            relay: Some(relay),
            portal: None,
        }
    }
    fn args(id: &str, phase: &str) -> Value {
        json!({"leaseId":"lease","ticket":id,"phase":phase,"chunkedMessages":true,"expectedInputSequence":"42","expectedRevision":"0"})
    }
    #[test]
    fn transfer_is_scoped_fenced_idempotent_and_preserves_capture_and_sequence() {
        let (old, mut old_frames) = connection(1);
        let (candidate, mut new_frames) = connection(1);
        let (other, _) = connection(2);
        let (rival, _) = connection(1);
        let mut lease = lease(&old);
        let relay = lease.relay.clone().unwrap();
        let instance = lease.video.as_ref().unwrap().describe()["resize"]["instanceId"].clone();
        let id = uuid::Uuid::new_v4().to_string();
        let frame = || dogcodes_remote_desktop::video::EncodedFrame {
            cursor_embedded: None,
            captured: std::time::Instant::now(),
            data: vec![0; 10],
            keyframe: true,
            latency: Duration::ZERO,
        };
        assert!(relay.push(frame(), Instant::now()));
        old_frames.try_recv().unwrap();
        assert_eq!(
            call(&mut lease, old.0.clone(), args(&id, "offer")).unwrap()["state"],
            "offered"
        );
        assert!(call(&mut lease, other.0.clone(), args(&id, "stage")).is_err());
        assert!(call(&mut lease, candidate.0.clone(), args(&id, "claim")).is_err());
        assert_eq!(
            call(&mut lease, candidate.0.clone(), args(&id, "stage")).unwrap()["state"],
            "staged"
        );
        assert!(call(&mut lease, rival.0.clone(), args(&id, "stage")).is_err());
        // An old input accepted before the swap must be known to the phone.
        lease.sequence = 43;
        assert!(call(&mut lease, candidate.0.clone(), args(&id, "claim")).is_err());
        assert_eq!(lease.owner.id, old.0.id);
        old.0.closed.store(true, Ordering::Release);
        drop(old_frames);
        assert!(lease.liveness.alive());
        assert!(!relay.closed());
        let mut claim = args(&id, "claim");
        claim["expectedInputSequence"] = json!("43");
        let accepted = call(&mut lease, candidate.0.clone(), claim.clone()).unwrap();
        assert_eq!(accepted["state"], "applied");
        assert_eq!(accepted["inputSequence"], "43");
        assert_eq!(lease.owner.id, candidate.0.id);
        assert!(Arc::ptr_eq(lease.relay.as_ref().unwrap(), &relay));
        assert_eq!(
            lease.video.as_ref().unwrap().describe()["resize"]["instanceId"],
            instance
        );
        assert_eq!(
            call(&mut lease, candidate.0.clone(), claim).unwrap(),
            accepted
        );
        assert_eq!(
            call(&mut lease, candidate.0.clone(), args(&id, "status")).unwrap(),
            accepted
        );
        assert_eq!(old.0.video_epoch.load(Ordering::Acquire), 2);
        assert!(!relay.push(frame(), Instant::now())); // explicit new decoder ready
        let generation = accepted["video"]["generation"].as_u64().unwrap();
        assert_eq!(generation, 2);
        relay.activate(generation).unwrap();
        assert!(relay.push(frame(), Instant::now()));
        let header = new_frames.try_recv().unwrap().0;
        assert_eq!(header["seq"], 2);
        assert_eq!(header["generation"], generation);
        let mut worker = Worker {
            lease: Some(lease),
            checked_lock: Some(Instant::now()),
            #[cfg(target_os = "linux")]
            preparing: None,
            #[cfg(target_os = "linux")]
            portal_store: None,
        };
        assert!(
            worker
                .call(
                    old.0.clone(),
                    "remote_desktop_video",
                    json!({"leaseId":"lease","ack":"999"})
                )
                .is_err()
        );
        assert!(
            worker
                .call(
                    old.0.clone(),
                    "remote_desktop_close",
                    json!({"leaseId":"lease"})
                )
                .is_err()
        );
        assert_eq!(worker.lease.as_ref().unwrap().owner.id, candidate.0.id);
        assert!(worker.lease.as_ref().unwrap().liveness.alive());
        drop(candidate);
        worker.expire();
        assert!(worker.lease.is_none());
    }
    #[test]
    fn cancellation_before_offer_admission_fences_the_late_offer() {
        let (old, _old_frames) = connection(1);
        let mut lease = lease(&old);
        let id = uuid::Uuid::new_v4().to_string();
        let unknown = call(&mut lease, old.0.clone(), args(&id, "status")).unwrap();
        assert_eq!(unknown["state"], "retired");
        assert_eq!(unknown["revision"], "0"); // absence alone is insufficient
        let canceled = call(&mut lease, old.0.clone(), args(&id, "cancel")).unwrap();
        assert_eq!(canceled["revision"], "1");
        assert!(call(&mut lease, old.0.clone(), args(&id, "offer")).is_err());
        assert_eq!(
            call(&mut lease, old.0.clone(), args(&id, "cancel")).unwrap()["revision"],
            "1"
        );
    }
    #[test]
    fn a_replaced_display_cannot_be_claimed_with_the_old_prepared_decoder() {
        let (old, _old_frames) = connection(1);
        let (candidate, _new_frames) = connection(1);
        let mut lease = lease(&old);
        let id = uuid::Uuid::new_v4().to_string();
        call(&mut lease, old.0.clone(), args(&id, "offer")).unwrap();
        call(&mut lease, candidate.0.clone(), args(&id, "stage")).unwrap();
        lease.video = Some(Session::fixture(lease.relay.clone().unwrap()));
        assert!(call(&mut lease, candidate.0.clone(), args(&id, "claim")).is_err());
        assert_eq!(lease.owner.id, old.0.id);
        assert_eq!(
            call(&mut lease, old.0.clone(), args(&id, "cancel")).unwrap()["state"],
            "canceled"
        );
    }
    #[test]
    fn expired_and_canceled_candidates_cannot_claim_or_extend_their_deadline() {
        let (old, _old_frames) = connection(1);
        let (candidate, _new_frames) = connection(1);
        let mut lease = lease(&old);
        let id = uuid::Uuid::new_v4().to_string();
        call(&mut lease, old.0.clone(), args(&id, "offer")).unwrap();
        let deadline = lease.handoff.as_ref().unwrap().until;
        call(&mut lease, old.0.clone(), args(&id, "offer")).unwrap();
        assert_eq!(lease.handoff.as_ref().unwrap().until, deadline);
        call(&mut lease, candidate.0.clone(), args(&id, "stage")).unwrap();
        lease.handoff.as_mut().unwrap().until = Instant::now();
        expire(&mut lease);
        assert!(call(&mut lease, candidate.0.clone(), args(&id, "claim")).is_err());
        assert_eq!(lease.owner.id, old.0.id);
        let next = uuid::Uuid::new_v4().to_string();
        assert!(call(&mut lease, old.0.clone(), args(&id, "offer")).is_err());
        let mut next_offer = args(&next, "offer");
        next_offer["expectedRevision"] = json!("1");
        call(&mut lease, old.0.clone(), next_offer).unwrap();
        call(&mut lease, candidate.0.clone(), args(&next, "stage")).unwrap();
        assert_eq!(
            call(&mut lease, old.0.clone(), args(&next, "cancel")).unwrap()["state"],
            "canceled"
        );
        assert!(call(&mut lease, candidate.0.clone(), args(&next, "claim")).is_err());
        let mut stale = args(&next, "offer");
        stale["expectedRevision"] = json!("1");
        assert!(call(&mut lease, old.0.clone(), stale).is_err());
        assert!(lease.handoff.is_none());
        let retired = call(&mut lease, candidate.0.clone(), args(&next, "status")).unwrap();
        assert_eq!(retired["state"], "retired");
        assert_eq!(retired["owner"], false);
        let original = call(&mut lease, old.0.clone(), args(&next, "status")).unwrap();
        assert_eq!(original["owner"], true);
        assert_eq!(original["revision"], "2");
        drop(old);
        assert!(!lease.liveness.alive());
    }
}

#[cfg(test)]
mod rtp_tests {
    use super::*;
    // SDP generated only for isolated negotiation; no ICE servers or remote
    // destination, capture, keyboard or mouse. Keep Answer alive while its
    // real VideoSender is owned by the fake PC desktop lease.
    async fn sender() -> dogcodes_remote_transport::Answer {
        let fingerprint = std::iter::repeat_n("00", 32).collect::<Vec<_>>().join(":");
        let transport = format!(
            "a=ice-ufrag:fixture\r\na=ice-pwd:fixturepassword0123456789\r\na=fingerprint:sha-256 {fingerprint}\r\na=setup:actpass\r\n"
        );
        let sdp = format!(
            "v=0\r\no=- 1 1 IN IP4 127.0.0.1\r\ns=-\r\nt=0 0\r\na=group:BUNDLE 0 1\r\na=msid-semantic: WMS\r\nm=application 9 UDP/DTLS/SCTP webrtc-datachannel\r\nc=IN IP4 0.0.0.0\r\na=mid:0\r\n{transport}a=sctp-port:5000\r\nm=video 9 UDP/TLS/RTP/SAVPF 102\r\nc=IN IP4 0.0.0.0\r\na=mid:1\r\n{transport}a=recvonly\r\na=rtcp-mux\r\na=rtpmap:102 H264/90000\r\na=fmtp:102 level-asymmetry-allowed=1;packetization-mode=1;profile-level-id=42e01f\r\na=rtcp-fb:102 nack\r\na=rtcp-fb:102 nack pli\r\n"
        );
        // Description is the protocol's serializable SDP, not a native type.
        let offer = serde_json::from_value(json!({"type":"offer","sdp":sdp})).unwrap();
        tokio::time::timeout(
            Duration::from_secs(6),
            dogcodes_remote_transport::answer_with_video(offer, Vec::new()),
        )
        .await
        .unwrap()
        .unwrap()
    }
    #[tokio::test]
    async fn rtp_senders_transfer_the_same_lease_and_gate_new_frames_until_ready() {
        use super::super::super::video::Session;
        use super::super::{Disconnect, Worker};
        let old_answer = sender().await;
        let next_answer = sender().await;
        let old_sender = old_answer.video.clone().expect("H.264 was negotiated");
        let next_sender = next_answer.video.clone().expect("H.264 was negotiated");
        let (old_tx, _old_rx) = tokio::sync::mpsc::channel(8);
        let (new_tx, _new_rx) = tokio::sync::mpsc::channel(8);
        let old = Disconnect::authenticated(Some(old_sender.clone()), old_tx, [7; 32]);
        let next = Disconnect::authenticated(Some(next_sender), new_tx, [7; 32]);
        let video = Session::fixture(Arc::new(old_sender));
        let instance = video.describe()["resize"]["instanceId"].clone();
        let mut lease = Lease {
            id: "rtp-lease".into(),
            liveness: Liveness::new(&old.0),
            handoff: None,
            handoff_revision: 0,
            owner: old.0.clone(),
            display: 1,
            expires: Instant::now() + Duration::from_secs(15),
            sequence: 9,
            controller: None,
            cursor: dogcodes_remote_desktop::cursor::Reader::default(),
            last_frame: None,
            capturer: None,
            video: Some(video),
            relay: None,
            portal: None,
        };
        let id = uuid::Uuid::new_v4().to_string();
        let args = |phase| json!({"leaseId":"rtp-lease","ticket":id,"phase":phase,"expectedRevision":"0","expectedInputSequence":"9","chunkedMessages":true,"videoTransport":"webrtc"});
        call(&mut lease, old.0.clone(), args("offer")).unwrap();
        call(&mut lease, next.0.clone(), args("stage")).unwrap();
        let moved = call(&mut lease, next.0.clone(), args("claim")).unwrap();
        assert_eq!(moved["state"], "applied");
        assert_eq!(moved["owner"], true);
        assert_eq!(moved["inputSequence"], "9");
        assert_eq!(moved["video"]["transport"], "webrtc");
        assert_eq!(moved["video"]["resize"]["instanceId"], instance);
        assert_eq!(moved["video"]["ready"], false);
        assert_eq!(moved["video"]["generation"], 1);
        let mut worker = Worker {
            lease: Some(lease),
            checked_lock: Some(Instant::now()),
            #[cfg(target_os = "linux")]
            preparing: None,
            #[cfg(target_os = "linux")]
            portal_store: None,
        };
        let ready = json!({"leaseId":"rtp-lease","ready":true,"generation":1});
        assert!(
            worker
                .call(old.0.clone(), "remote_desktop_video", ready.clone())
                .is_err()
        );
        let response = worker
            .call(next.0.clone(), "remote_desktop_video", ready)
            .unwrap();
        assert_eq!(response["video"]["ready"], true);
        assert_eq!(response["video"]["generation"], 1);
        drop(worker);
        drop(old_answer);
        drop(next_answer);
        tokio::task::yield_now().await;
    }
    #[tokio::test]
    async fn binary_and_rtp_transfers_keep_the_lease_but_gate_a_new_codec_in_both_directions() {
        use super::super::super::video::{RelayVideo, Session};
        use super::super::{Disconnect, Worker};
        for from_binary in [true, false] {
            let answer = sender().await;
            let sender = answer.video.clone().unwrap();
            let (old_tx, _old_frames) = tokio::sync::mpsc::channel(8);
            let (next_tx, _next_frames) = tokio::sync::mpsc::channel(8);
            let old = Disconnect::authenticated(
                if from_binary {
                    None
                } else {
                    Some(sender.clone())
                },
                old_tx,
                [7; 32],
            );
            let next = Disconnect::authenticated(
                if from_binary { Some(sender) } else { None },
                next_tx,
                [7; 32],
            );
            let relay = from_binary.then(|| {
                Arc::new(RelayVideo::with_epoch(
                    old.0.binary.clone(),
                    "cross-lease".into(),
                    old.0.video_epoch.clone(),
                ))
            });
            let video = if let Some(relay) = &relay {
                Session::fixture(relay.clone())
            } else {
                Session::fixture(Arc::new(old.0.video.clone().unwrap()))
            };
            let source = video.describe()["resize"]["instanceId"].clone();
            let old_generation = video.describe()["generation"].as_u64().unwrap();
            let mut lease = Lease {
                id: "cross-lease".into(),
                owner: old.0.clone(),
                liveness: Liveness::new(&old.0),
                handoff: None,
                handoff_revision: 0,
                display: 1,
                expires: Instant::now() + Duration::from_secs(15),
                sequence: 9,
                controller: None,
                cursor: dogcodes_remote_desktop::cursor::Reader::default(),
                last_frame: None,
                capturer: None,
                video: Some(video),
                relay,
                portal: None,
            };
            let ticket = uuid::Uuid::new_v4().to_string();
            let target = if from_binary { "webrtc" } else { "binary" };
            let args = |phase| json!({"leaseId":"cross-lease","ticket":ticket,"phase":phase,"expectedRevision":"0","expectedInputSequence":"9","chunkedMessages":true,"videoTransport":target});
            call(&mut lease, old.0.clone(), args("offer")).unwrap();
            let staged = call(&mut lease, next.0.clone(), args("stage")).unwrap();
            assert_eq!(staged["video"]["transport"], target);
            assert_eq!(staged["video"]["switchingCodec"], true);
            let mut changed = args("stage");
            changed["videoTransport"] = json!(if from_binary { "binary" } else { "webrtc" });
            assert!(call(&mut lease, next.0.clone(), changed).is_err());
            let moved = call(&mut lease, next.0.clone(), args("claim")).unwrap();
            assert_eq!(moved["owner"], true);
            assert_eq!(moved["inputSequence"], "9");
            assert_eq!(moved["video"]["resize"]["instanceId"], source);
            assert_eq!(moved["video"]["codecReady"], false);
            assert!(moved["video"]["generation"].as_u64().unwrap() > old_generation);
            if from_binary {
                assert!(old.0.video_epoch.load(Ordering::Acquire) > old_generation);
            }
            let mut worker = Worker {
                lease: Some(lease),
                checked_lock: Some(Instant::now()),
                #[cfg(target_os = "linux")]
                preparing: None,
                #[cfg(target_os = "linux")]
                portal_store: None,
            };
            let ready = json!({"leaseId":"cross-lease","ready":true,"generation":moved["video"]["generation"]});
            let early = worker
                .call(next.0.clone(), "remote_desktop_video", ready.clone())
                .unwrap();
            assert_eq!(early["video"]["ready"], false);
            worker
                .lease
                .as_ref()
                .unwrap()
                .video
                .as_ref()
                .unwrap()
                .fixture_codec_ready();
            let complete = worker
                .call(next.0.clone(), "remote_desktop_video", ready)
                .unwrap();
            assert_eq!(complete["video"]["ready"], true);
            assert_eq!(complete["video"]["transport"], target);
            assert!(
                worker
                    .call(
                        old.0.clone(),
                        "remote_desktop_video",
                        json!({"leaseId":"cross-lease"})
                    )
                    .is_err()
            );
            assert_eq!(worker.lease.as_ref().unwrap().sequence, 9);
        }
    }
}
