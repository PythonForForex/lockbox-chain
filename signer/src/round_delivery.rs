use crate::peer_transport::{call_peer, PeerDirectory};
use futures::channel::mpsc as futures_channel;
use futures::{Sink, SinkExt, Stream, StreamExt};
use round_based::{Incoming, MessageDestination, MessageType, Outgoing};
use serde::de::DeserializeOwned;
use serde::Serialize;
use serde_json::json;
use std::fmt;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::sync::mpsc;

const INBOX_CAPACITY: usize = 4096;
const ROUND_MESSAGE_SEND_TIMEOUT_SECONDS: u64 = 300;

pub enum InboxEvent {
    ProtocolMessage { sender_signer_id: String, is_broadcast: bool, payload: String },
    DeliveryFailure { blamed_signer_id: Option<String>, detail: String },
}

pub type InboxSender = mpsc::Sender<InboxEvent>;
pub type InboxReceiver = mpsc::Receiver<InboxEvent>;

pub fn build_session_inbox() -> (InboxSender, InboxReceiver) {
    mpsc::channel(INBOX_CAPACITY)
}

#[derive(Default)]
pub struct DeliveryBlame {
    pub blamed_signer_id: Option<String>,
    pub detail: Option<String>,
}

pub type SharedDeliveryBlame = Arc<Mutex<DeliveryBlame>>;

#[derive(Debug)]
pub struct RoundDeliveryError(pub String);

impl fmt::Display for RoundDeliveryError {
    fn fmt(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl std::error::Error for RoundDeliveryError {}

fn note_blame(blame: &SharedDeliveryBlame, blamed_signer_id: Option<String>, detail: String) {
    let mut delivery_blame = blame.lock().expect("blame lock");
    if delivery_blame.detail.is_none() {
        delivery_blame.blamed_signer_id = blamed_signer_id;
        delivery_blame.detail = Some(detail);
    }
}

// The library's indices are positions within this round, and only a sender authenticated by its envelope reaches
// this point, so a message that fails to parse or names no party is attributable to that signer.
fn inbox_event_to_incoming<M: DeserializeOwned>(
    event: InboxEvent,
    message_id: u64,
    participants: &[String],
    blame: &SharedDeliveryBlame,
) -> Result<Incoming<M>, RoundDeliveryError> {
    match event {
        InboxEvent::DeliveryFailure { blamed_signer_id, detail } => {
            note_blame(blame, blamed_signer_id, detail.clone());
            Err(RoundDeliveryError(detail))
        }
        InboxEvent::ProtocolMessage { sender_signer_id, is_broadcast, payload } => {
            let Some(sender_position) = participants.iter().position(|signer| *signer == sender_signer_id) else {
                let detail = format!("message from {sender_signer_id}, not a participant");
                note_blame(blame, Some(sender_signer_id), detail.clone());
                return Err(RoundDeliveryError(detail));
            };
            let message = serde_json::from_str::<M>(&payload).map_err(|error| {
                let detail = format!("malformed protocol message from {sender_signer_id}: {error}");
                note_blame(blame, Some(sender_signer_id.clone()), detail.clone());
                RoundDeliveryError(detail)
            })?;
            Ok(Incoming {
                id: message_id,
                sender: sender_position as u16,
                msg_type: if is_broadcast { MessageType::Broadcast } else { MessageType::P2P },
                msg: message,
            })
        }
    }
}

async fn send_one_round_message(
    directory: &PeerDirectory,
    session_id: &str,
    recipient: &str,
    sequence: u64,
    is_broadcast: bool,
    payload: &str,
) -> Result<(), String> {
    let body = json!({"session_id": session_id, "sequence": sequence, "is_broadcast": is_broadcast, "payload": payload});
    let timeout = Duration::from_secs(ROUND_MESSAGE_SEND_TIMEOUT_SECONDS);
    call_peer(directory, recipient, "round_message", &body, timeout).await.map(|_| ())
}

async fn forward_outgoing_messages<M: Serialize>(
    mut outgoing_receiver: futures_channel::UnboundedReceiver<Outgoing<M>>,
    directory: Arc<PeerDirectory>,
    session_id: String,
    participants: Vec<String>,
    own_inbox: InboxSender,
) {
    let mut sequence = 0u64;
    while let Some(outgoing) = outgoing_receiver.next().await {
        let payload = serde_json::to_string(&outgoing.msg).expect("protocol message serialises");
        let recipients: Vec<String> = match outgoing.recipient {
            MessageDestination::AllParties => {
                participants.iter().filter(|signer| **signer != directory.own_signer_id).cloned().collect()
            }
            MessageDestination::OneParty(position) => vec![participants[position as usize].clone()],
        };
        let is_broadcast = matches!(outgoing.recipient, MessageDestination::AllParties);
        sequence += 1;
        for recipient in recipients {
            if let Err(detail) =
                send_one_round_message(&directory, &session_id, &recipient, sequence, is_broadcast, &payload).await
            {
                let failure = InboxEvent::DeliveryFailure { blamed_signer_id: Some(recipient), detail };
                let _ = own_inbox.send(failure).await;
                return;
            }
        }
    }
}

pub struct RoundDelivery<M> {
    pub incoming: std::pin::Pin<Box<dyn Stream<Item = Result<Incoming<M>, RoundDeliveryError>> + Send>>,
    pub outgoing: std::pin::Pin<Box<dyn Sink<Outgoing<M>, Error = RoundDeliveryError> + Send>>,
}

pub fn build_round_delivery<M>(
    directory: Arc<PeerDirectory>,
    session_id: String,
    participants: Vec<String>,
    own_inbox_sender: InboxSender,
    own_inbox_receiver: InboxReceiver,
    blame: SharedDeliveryBlame,
) -> RoundDelivery<M>
where
    M: Serialize + DeserializeOwned + Send + 'static,
{
    let (outgoing_sender, outgoing_receiver) = futures_channel::unbounded::<Outgoing<M>>();
    tokio::spawn(forward_outgoing_messages(
        outgoing_receiver,
        directory,
        session_id,
        participants.clone(),
        own_inbox_sender,
    ));
    let incoming = futures::stream::unfold(
        (own_inbox_receiver, 0u64),
        move |(mut receiver, message_id)| {
            let participants = participants.clone();
            let blame = blame.clone();
            async move {
                let event = receiver.recv().await?;
                let incoming = inbox_event_to_incoming::<M>(event, message_id, &participants, &blame);
                Some((incoming, (receiver, message_id + 1)))
            }
        },
    );
    let outgoing =
        outgoing_sender.sink_map_err(|error| RoundDeliveryError(format!("outgoing channel closed: {error}")));
    RoundDelivery { incoming: Box::pin(incoming), outgoing: Box::pin(outgoing) }
}
