mod frames;

use std::{collections::HashMap, sync::Arc};

use atrium_api::{
    agent::{store::MemorySessionStore, AtpAgent},
    app::bsky::feed::post::Record,
    com::atproto::sync::subscribe_repos::{Commit, NSID},
    types::{string::Did, Collection as _},
};
use atrium_xrpc_client::reqwest::ReqwestClient;
use chrono::Local;
use cja::color_eyre::Result;
use cja::{app_state::AppState as _, color_eyre};
use frames::Frame;
use futures::StreamExt as _;
use sqlx::PgPool;
use tokio::sync::RwLock;
use tokio_tungstenite::{connect_async, tungstenite::Message};

use crate::{sms, AppState};

const CREATE_ACTION: &str = "create";
const POST_PATH_TYPE: &str = atrium_api::app::bsky::feed::Post::NSID;

struct NewPost {
    record: Record,
    author: Did,
}

impl NewPost {
    fn print(&self) {
        println!(
            "{} - {}",
            self.record.created_at.as_ref().with_timezone(&Local),
            self.author.as_str()
        );
        for line in self.record.text.split('\n') {
            println!("  {line}");
        }
    }
}

#[derive(Debug, Clone)]
struct Subscription {
    did: String,
    handle: String,
    phone_number: String,
}

impl Subscription {
    async fn send_sms(&self, config: &sms::TwilioConfig, post: &NewPost) -> Result<()> {
        sms::send_sms(config, &self.phone_number, &post.record.text).await?;

        Ok(())
    }
}

#[derive(Clone)]
pub struct Handler {
    state: AppState,
    dids_to_subscriptions: Arc<RwLock<HashMap<Did, Vec<Subscription>>>>,
}

impl Handler {
    pub async fn from_db(state: &AppState) -> Result<Self> {
        let subscriptions = sqlx::query_as!(
            Subscription,
            "SELECT did, handle, phone_number FROM SmsHandleSubscriptions"
        )
        .fetch_all(state.db())
        .await?;

        let mut map: HashMap<Did, Vec<Subscription>> = HashMap::new();

        for sub in subscriptions {
            let did = sub.did.parse().unwrap();
            map.entry(did).or_default().push(sub);
        }

        Ok(Self {
            state: state.clone(),
            dids_to_subscriptions: Arc::new(RwLock::new(map)),
        })
    }

    pub async fn update_from_db(&self) -> Result<()> {
        let subscriptions = sqlx::query_as!(
            Subscription,
            "SELECT did, handle, phone_number FROM SmsHandleSubscriptions"
        )
        .fetch_all(self.state.db())
        .await?;

        let mut map: HashMap<Did, Vec<Subscription>> = HashMap::new();

        for sub in subscriptions {
            let did = sub.did.parse().unwrap();
            map.entry(did).or_default().push(sub);
        }

        let mut write = self.dids_to_subscriptions.write().await;
        *write = map;
        drop(write);

        println!("updated subscriptions");
        let map = self.dids_to_subscriptions.read().await;
        let dids: Vec<_> = map.keys().collect();
        println!("listening for posts from {:?}", dids);
        drop(map);

        Ok(())
    }

    async fn handle_commit(&self, commit: &Commit) -> Result<()> {
        let map = self.dids_to_subscriptions.read().await;
        let subscriptions = map.get(&commit.repo).cloned();
        let Some(subscriptions) = subscriptions else {
            return Ok(());
        };
        drop(map);

        for op in &commit.ops {
            if op.action == CREATE_ACTION {
                let path_type = op.path.split('/').next().unwrap();

                if path_type == POST_PATH_TYPE {
                    let (items, _) =
                        rs_car::car_read_all(&mut commit.blocks.as_slice(), true).await?;
                    if let Some((_, item)) = items.iter().find(|(cid, _)| {
                        Some(cid.to_bytes()) == op.cid.as_ref().map(|cid| cid.0.to_bytes())
                    }) {
                        let record =
                            serde_ipld_dagcbor::from_reader::<Record, _>(&mut item.as_slice())?;

                        let new_post = NewPost {
                            record,
                            author: commit.repo.clone(),
                        };

                        for sub in subscriptions.iter() {
                            sub.send_sms(&self.state.twilio_config, &new_post).await?;
                        }
                    } else {
                        return Err(color_eyre::eyre::eyre!(
                            "FAILED: could not find item with operation cid {:?} out of {} items",
                            op.cid,
                            items.len()
                        ));
                    }
                }
            }
        }
        Ok(())
    }
}

pub async fn consume_firehose(handler: Handler) -> Result<()> {
    const FIREHOSE_DOMAIN: &str = "bsky.network";

    let map = handler.dids_to_subscriptions.read().await;
    let dids: Vec<_> = map.keys().collect();
    println!("listening for posts from {:?}", dids);
    drop(map);

    let (mut stream, _) = connect_async(format!("wss://{FIREHOSE_DOMAIN}/xrpc/{NSID}")).await?;
    while let Some(result) = {
        if let Some(Ok(Message::Binary(data))) = stream.next().await {
            Some(Frame::try_from(data.as_slice()))
        } else {
            None
        }
    } {
        if let Ok(Frame::Message(Some(t), message)) = result {
            if t.as_str() == "#commit" {
                let commit = serde_ipld_dagcbor::from_reader(message.body.as_slice())?;
                if let Err(err) = handler.handle_commit(&commit).await {
                    eprintln!("FAILED: {err:?}");
                }
            }
        }
    }
    Ok(())
}
