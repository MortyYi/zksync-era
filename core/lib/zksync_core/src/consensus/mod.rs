//! Consensus-related functionality.
use anyhow::Context as _;
use zksync_concurrency::{ctx, scope, error::Wrap as _};
use zksync_consensus_executor as executor;
use zksync_consensus_roles::{node, validator};
use zksync_consensus_storage::BlockStore;
use zksync_dal::{consensus_dal::Payload,ConnectionPool};
use crate::sync_layer::sync_action::ActionQueueSender;
use zksync_types::Address;

mod storage;

#[cfg(test)]
pub(crate) mod testonly;
#[cfg(test)]
mod tests;
//#[cfg(test)]
//mod gossip_tests;

use self::storage::Store;
use serde::de::Error;
use std::collections::{HashMap, HashSet};
use zksync_consensus_crypto::{Text, TextFmt};

#[derive(PartialEq, Eq, Hash)]
pub struct SerdeText<T: TextFmt>(pub T);

impl<'de, T: TextFmt> serde::Deserialize<'de> for SerdeText<T> {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        Ok(Self(
            T::decode(Text::new(<&str>::deserialize(d)?)).map_err(Error::custom)?,
        ))
    }
}

#[derive(serde::Deserialize)]
pub struct SerdeConfig {
    /// Local socket address to listen for the incoming connections.
    pub server_addr: std::net::SocketAddr,
    /// Public address of this node (should forward to server_addr)
    /// that will be advertised to peers, so that they can connect to this
    /// node.
    pub public_addr: std::net::SocketAddr,

    /// Validator private key. Should be set only for the validator node.
    pub validator_key: Option<SerdeText<validator::SecretKey>>,

    /// Validators participating in consensus.
    pub validator_set: Vec<SerdeText<validator::PublicKey>>,

    /// Key of this node. It uniquely identifies the node.
    pub node_key: SerdeText<node::SecretKey>,
    /// Limit on the number of inbound connections outside
    /// of the `static_inbound` set.
    pub gossip_dynamic_inbound_limit: u64,
    /// Inbound gossip connections that should be unconditionally accepted.
    pub gossip_static_inbound: HashSet<SerdeText<node::PublicKey>>,
    /// Outbound gossip connections that the node should actively try to
    /// establish and maintain.
    pub gossip_static_outbound: HashMap<SerdeText<node::PublicKey>, std::net::SocketAddr>,

    pub operator_address: Option<Address>,
}

impl SerdeConfig {
    pub(crate) fn executor(&self) -> anyhow::Result<executor::Config> {
        Ok(executor::Config {
            server_addr: self.server_addr.clone(),
            validators: validator::ValidatorSet::new(
                self.validator_set.iter().map(|k| k.0.clone()),
            )
            .context("validator_set")?,
            node_key: self.node_key.0.clone(),
            gossip_dynamic_inbound_limit: self.gossip_dynamic_inbound_limit,
            gossip_static_inbound: self
                .gossip_static_inbound
                .iter()
                .map(|k| k.0.clone())
                .collect(),
            gossip_static_outbound: self
                .gossip_static_outbound
                .iter()
                .map(|(k, v)| (k.0.clone(), v.clone()))
                .collect(),
        })
    }
    pub(crate) fn validator(&self) -> anyhow::Result<executor::ValidatorConfig> {
        let key = self
            .validator_key
            .as_ref()
            .context("validator_key is required")?;
        Ok(executor::ValidatorConfig {
            key: key.0.clone(),
            public_addr: self.public_addr.clone(),
        })
    }
}

impl TryFrom<SerdeConfig> for Config {
    type Error = anyhow::Error;
    fn try_from(cfg: SerdeConfig) -> anyhow::Result<Self> {
        Ok(Self {
            executor: cfg.executor()?,
            validator: cfg.validator()?,
            operator_address: cfg
                .operator_address
                .context("operator_address is required")?,
        })
    }
}

#[derive(Debug,Clone)]
pub struct Config {
    pub executor: executor::Config,
    pub validator: executor::ValidatorConfig,
    pub operator_address: Address,
}

impl Config {
    #[allow(dead_code)]
    pub async fn run(self, ctx: &ctx::Ctx, pool: ConnectionPool) -> anyhow::Result<()> {
        if self.executor.validators != validator::ValidatorSet::new(vec![self.validator.key.public()]).unwrap() {
            return Err(anyhow::anyhow!("currently only consensus with just 1 validator is supported").into());
        }
        scope::run!(&ctx, |ctx, s| async {
            let store = Store::new(pool, self.operator_address);
            let mut block_store = store.clone().into_block_store();
            block_store.try_init_genesis(ctx,&self.validator.key).await.wrap("block_store.try_init_genesis()")?;
            let (block_store,runner) = BlockStore::new(ctx,Box::new(block_store),1000).await.wrap("BlockStore::new()")?;
            s.spawn_bg(runner.run(ctx));
            let executor = executor::Executor {
                config: self.executor,
                block_store,
                validator: Some(executor::Validator {
                    config: self.validator,
                    replica_store: Box::new(store.clone()),
                    payload_manager: Box::new(store.clone()),
                }),
            };
            executor.run(ctx).await
        })
        .await
    }
}

#[derive(Debug,Clone)]
pub struct FetcherConfig {
    executor: executor::Config,
    operator_address: Address,
}

impl TryFrom<SerdeConfig> for FetcherConfig {
    type Error = anyhow::Error;
    fn try_from(cfg: SerdeConfig) -> anyhow::Result<Self> {
        Ok(Self {
            executor: cfg.executor()?,
            operator_address: cfg
                .operator_address
                .context("operator_address is required")?,
        })
    }
}

impl FetcherConfig {
    /// Starts fetching L2 blocks using peer-to-peer gossip network.
    #[allow(dead_code)]
    pub async fn run(
        self,
        ctx: &ctx::Ctx,
        pool: ConnectionPool,
        actions: ActionQueueSender,
    ) -> anyhow::Result<()> {
        tracing::info!(
            "Starting gossip fetcher with {:?} and node key {:?}",
            self.executor,
            self.executor.node_key.public(),
        );

        scope::run!(ctx, |ctx, s| async {
             let store = Store::new(pool, self.operator_address);
            let mut block_store = store.clone().into_block_store();
            block_store.set_actions_queue(ctx,actions).await.wrap("block_store.try_init_genesis()")?;
            let (block_store,runner) = BlockStore::new(ctx,Box::new(block_store),1000).await.wrap("BlockStore::new()")?;
            s.spawn_bg(runner.run(ctx));
            let executor = executor::Executor {
                config: self.executor,
                block_store,
                validator: None,
            };
            executor.run(ctx).await
        })
        .await
    }
}
