//! Signalling arrives inside an already authenticated Noise connection. Each
//! upgraded connection authenticates again, with fresh Noise state and the same pins.
use super::{
    Arc, Core, Established, HANDSHAKE_WAIT, Message, RemoteHost, ServiceControl, Session, Socket,
    Value, failure, host_name, http, json, receive, send_json,
};
use serde::Deserialize;

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Offer {
    offer: dogcodes_remote_transport::Description,
    purpose: String,
}
impl RemoteHost {
    pub(super) async fn open_direct(
        &self,
        core: &Arc<Core>,
        control: &Arc<ServiceControl>,
        peer: [u8; 32],
        args: Value,
    ) -> Result<Value, String> {
        let request: Offer = serde_json::from_value(args).map_err(failure)?;
        if !matches!(
            request.purpose.as_str(),
            "business" | "desktop" | "preview" | "attachments"
        ) {
            return Err("直连通道用途无效".into());
        }
        let permit = self
            .direct_slots
            .clone()
            .try_acquire_owned()
            .map_err(|_| "直连通道数量已达上限")?;
        let (config, secrets, device) = {
            let state = self.state.lock().await;
            let ready = state.ready.as_ref().ok_or("remote unavailable")?;
            let device = ready
                .book
                .grants()
                .into_iter()
                .find(|g| !g.revoked && g.public_key == hex::encode(peer))
                .ok_or("device revoked")?;
            (
                ready.config.clone(),
                ready.secrets.clone(),
                device.device_id,
            )
        };
        let ice = match http(&config, &secrets, reqwest::Method::GET, "ice").await {
            Ok(value) => {
                serde_json::from_value(value["iceServers"].clone()).map_err(|_| "ICE 配置无效")?
            }
            // Older/local service deployments can still use LAN candidates.
            Err(_) => Vec::new(),
        };
        // A desktop connection also carries the screen as an H.264 track
        // when the phone offered to receive one.
        let answer = if request.purpose == "desktop" {
            dogcodes_remote_transport::answer_with_video(request.offer, ice).await?
        } else {
            dogcodes_remote_transport::answer(request.offer, ice).await?
        };
        let video = answer.video.clone();
        let id = uuid::Uuid::new_v4().to_string();
        let purpose = request.purpose;
        let response = json!({"connectionId":id,"purpose":purpose,"answer":answer.description,
            "noiseContext":["dogcodes-direct",1,config.host_id,id,purpose],"transport":"webrtc-datachannel",
            "video":video.is_some()});
        let (core, control, host) = (core.clone(), control.clone(), control.remote.clone());
        tokio::spawn(async move {
            let _permit = permit;
            let _=async {
                let channel=answer.accept().await?;
                let mut socket=Socket::Direct(channel);
                let (config,mut noise)={
                    let state=host.state.lock().await;
                    let ready=state.ready.as_ref().ok_or("remote unavailable")?;
                    if !ready.book.grants().iter().any(|g|g.device_id==device && !g.revoked && g.public_key==hex::encode(peer)) {return Err("device revoked".to_string());}
                    let key:[u8;32]=hex::decode(&ready.secrets.private_key).map_err(failure)?.try_into().map_err(|_|"invalid key")?;
                    let context=serde_json::to_vec(&json!(["dogcodes-direct",1,ready.config.host_id,id,purpose])).map_err(failure)?;
                    (ready.config.clone(),Session::new(false,&key,Some(peer),None,&context).map_err(failure)?)
                };
                noise.read_handshake(&receive(&mut socket,HANDSHAKE_WAIT).await?).map_err(failure)?;
                socket.send(Message::Binary(noise.write_handshake().map_err(failure)?.into())).await?;
                noise.read_handshake(&receive(&mut socket,HANDSHAKE_WAIT).await?).map_err(failure)?;
                let lease={
                    let state=host.state.lock().await;
                    state.ready.as_ref().ok_or("remote unavailable")?.book.authorize(&noise).map_err(failure)?
                };
                let ack=json!({"type":"connected","hostId":config.host_id,"hostName":host_name(),
                    "binding":hex::encode(noise.binding().map_err(failure)?),"transport":"webrtc","connectionId":id,"purpose":purpose});
                send_json(&mut socket,&mut noise,&ack).await?;
                host.serve(&core,&control,Established{socket,session:&mut noise,config:&config,lease,peer,video}).await
            }.await;
        });
        Ok(response)
    }
}
