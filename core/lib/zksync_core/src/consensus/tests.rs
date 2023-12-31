use zksync_concurrency::{ctx, scope};
use zksync_consensus_executor::testonly::ValidatorNode;
use zksync_dal::ConnectionPool;
use zksync_types::Address;

use super::*;

// In the current implementation, consensus certificates are created asynchronously
// for the miniblocks constructed by the StateKeeper. This means that consensus actor
// is effectively just backfilling the consensus certificates for the miniblocks in storage.
#[tokio::test(flavor = "multi_thread")]
async fn test_backfill() {
    const OPERATOR_ADDRESS: Address = Address::repeat_byte(17);

    zksync_concurrency::testonly::abort_on_panic();
    let ctx = &ctx::test_root(&ctx::RealClock);
    let rng = &mut ctx.rng();
    let pool = ConnectionPool::test_pool().await;

    scope::run!(ctx, |ctx, s| async {
        // Start state keeper.
        let (mut sk, sk_runner) = testonly::StateKeeperHandle::new(OPERATOR_ADDRESS);
        s.spawn_bg(sk_runner.run(ctx, &pool));

        // Populate storage with a bunch of blocks.
        sk.push_random_blocks(rng, 5).await;
        sk.sync(ctx, &pool).await.context("sk.sync(<1st phase>)")?;

        let cfg = ValidatorNode::for_single_validator(&mut ctx.rng());
        let validators = cfg.node.validators.clone();

        // Start consensus actor and wait for it to create a genesis block.
        let cfg = Config {
            executor: cfg.node,
            validator: cfg.validator,
            operator_address: OPERATOR_ADDRESS,
        };
        s.spawn_bg(cfg.run(ctx, pool.clone()));
        sk.sync_consensus(ctx, &pool)
            .await
            .context("sk.sync_consensus(<1st phase>)")?;

        // Generate couple more blocks and wait for consensus to catch up.
        sk.push_random_blocks(rng, 7).await;
        sk.sync_consensus(ctx, &pool)
            .await
            .context("sk.sync_consensus(<2nd phase>)")?;

        // Synchronously produce blocks one by one, and wait for consensus.
        for _ in 0..5 {
            sk.push_random_blocks(rng, 1).await;
            sk.sync_consensus(ctx, &pool)
                .await
                .context("sk.sync_consensus(<3rd phase>)")?;
        }

        sk.validate_consensus(ctx, &pool, &validators)
            .await
            .context("sk.validate_consensus()")?;
        Ok(())
    })
    .await
    .unwrap();

    // TODO: restart node and continue.
}
