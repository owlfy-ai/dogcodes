//! One original prompt and one cancellation outcome, independent of waiters.
use super::{Agent,CancelNotification,ConnectionTo,DriverError,SessionId,lifecycle};
use std::sync::{Arc,Mutex,atomic::AtomicBool};
use std::time::Duration;
use tokio::sync::{mpsc,watch};
pub(super) type Outcome=Option<Result<(),DriverError>>;
pub(super) type Receipt=watch::Receiver<Outcome>;
#[derive(Default)]
pub(super) struct Dispatch{pub cancel_requested:bool,pub sent:bool,cancel_sent:bool}
pub(super) struct Turn{
    pub session:String,
    pub epoch:Arc<AtomicBool>,
    pub native:Option<Arc<lifecycle::Control>>,
    pub done:Receipt,
    pub dispatch:Mutex<Dispatch>,
    cancels:mpsc::UnboundedSender<Arc<Self>>,
    stop:Mutex<Option<Receipt>>,
}
impl Turn{
    pub fn new(session:String,epoch:Arc<AtomicBool>,native:Option<Arc<lifecycle::Control>>,cancels:mpsc::UnboundedSender<Arc<Self>>)->(Arc<Self>,watch::Sender<Outcome>){
        let(completed,done)=watch::channel(None);
        (Arc::new(Self{session,epoch,native,done,dispatch:Mutex::new(Dispatch::default()),cancels,stop:Mutex::new(None)}),completed)
    }
    pub fn finish(&self,sender:&watch::Sender<Outcome>,result:Result<(),DriverError>){
        // A late queued cancel either precedes this settlement in the SDK
        // outbox, or observes it and sends nothing. It cannot hit the next turn.
        let _dispatch=self.dispatch.lock().expect("ACP prompt dispatch");
        sender.send_replace(Some(result));
    }
    pub fn cancel_on(&self,connection:&ConnectionTo<Agent>)->Result<(),super::AcpError>{
        let mut dispatch=self.dispatch.lock().expect("ACP prompt dispatch");
        if dispatch.sent&&!dispatch.cancel_sent&&self.done.borrow().is_none(){
            connection.send_notification(CancelNotification::new(SessionId::new(self.session.as_str())))?;
            dispatch.cancel_sent=true;
        }
        Ok(())
    }
    pub fn stop_pending(&self)->bool{
        self.stop.lock().expect("ACP stop receipt").as_ref().is_some_and(|receipt|receipt.borrow().is_none())
    }
    pub fn failed_stop(&self)->bool{
        self.stop.lock().expect("ACP stop receipt").as_ref().is_some_and(|receipt|matches!(*receipt.borrow(),Some(Err(_))))
    }
    pub fn stop(self:&Arc<Self>,timeout:Duration)->Receipt{
        let mut current=self.stop.lock().expect("ACP stop receipt");
        if let Some(receipt)=current.as_ref(){return receipt.clone();}
        let(completed,outcome)=watch::channel(None);
        *current=Some(outcome.clone());
        let queued={
            let mut dispatch=self.dispatch.lock().expect("ACP prompt dispatch");
            dispatch.cancel_requested=true;
            if self.done.borrow().is_none(){self.cancels.send(self.clone()).map_err(|_|failure("cancel channel is closed"))}else{Ok(())}
        };
        let original=self.clone();
        tokio::spawn(async move{
            let result=match queued{Ok(())=>wait(original.done.clone(),timeout).await,Err(error)=>Err(error)};
            if result.is_err()&&let Some(native)=&original.native{native.request_close();}
            completed.send_replace(Some(result));
        });
        outcome
    }
}
pub(super) async fn wait(mut receipt:Receipt,timeout:Duration)->Result<(),DriverError>{
    tokio::time::timeout(timeout,async{
        loop{
            if let Some(result)=receipt.borrow_and_update().clone(){return result;}
            receipt.changed().await.map_err(|_|failure("prompt ended without a termination receipt"))?;
        }
    }).await.map_err(|_|failure("stop has no confirmed prompt terminal response"))?
}
fn failure(reason:&str)->DriverError{DriverError::Disconnected(format!("ACP {reason}"))}
