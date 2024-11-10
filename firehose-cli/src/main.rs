use std::collections::HashMap;

use atrium_api::{
    agent::{store::MemorySessionStore, AtpAgent},
    app::bsky::feed::post::Record,
    com::atproto::sync::subscribe_repos::{Commit, NSID},
    types::{string::Did, Collection as _},
};
use atrium_xrpc_client::reqwest::ReqwestClient;
use chrono::Local;
use color_eyre::Result;
use frames::Frame;
use futures::StreamExt as _;
use tokio_tungstenite::{connect_async, tungstenite::Message};

const FIREHOSE_DOMAIN: &str = "bsky.network";

mod frames;

#[tokio::main]
async fn main() -> Result<()> {
    let handler = Handler::resolve_handles(HANDLES).await?;
    println!("listening for posts from {:?}", handler.dids_to_handles);

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

struct Handler {
    dids_to_handles: HashMap<Did, String>,
}

const HANDLES: &[&str] = &["coreyja.com"];

impl Handler {
    async fn resolve_handles(handles: &[&str]) -> Result<Self> {
        let client = ReqwestClient::new("https://bsky.social");
        let agent = AtpAgent::new(client, MemorySessionStore::default());

        let mut dids_to_handles = HashMap::new();
        for handle in handles {
            let resp = agent
                .api
                .com
                .atproto
                .identity
                .resolve_handle(
                    atrium_api::com::atproto::identity::resolve_handle::ParametersData {
                        handle: handle
                            .parse()
                            .map_err(|_| color_eyre::eyre::eyre!("invalid handle"))?,
                    }
                    .into(),
                )
                .await?;
            let did = resp.data.did;
            dids_to_handles.insert(did, handle.to_string());
        }
        Ok(Self { dids_to_handles })
    }

    async fn handle_commit(&self, commit: &Commit) -> Result<()> {
        if !self.dids_to_handles.contains_key(&commit.repo) {
            return Ok(());
        }

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
                        new_post.print();
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
