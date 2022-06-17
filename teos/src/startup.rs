use simple_logger::init_with_level;
use std::io::ErrorKind;
use std::ops::{Deref, DerefMut};
use std::str::FromStr;
use std::sync::{Arc, Condvar, Mutex};
use tokio::task;
use tonic::transport::Server;

use bitcoin::network::constants::Network;
use bitcoin::secp256k1::{PublicKey, SecretKey};
use bitcoincore_rpc::{Auth, Client};
use lightning_block_sync::init::validate_best_block_header;
use lightning_block_sync::poll::{
    ChainPoller, Poll, ValidatedBlock, ValidatedBlockHeader,
};
use lightning_block_sync::{BlockSource, SpvClient, UnboundedCache};


use crate::api::internal::InternalAPI;
use crate::api::{http, tor};
use crate::bitcoin_cli::BitcoindClient;
use crate::carrier::Carrier;
use crate::chain_monitor::ChainMonitor;
use crate::config::Config;
use crate::dbm::DBM;
use crate::gatekeeper::Gatekeeper;
use crate::protos::private_tower_services_server::PrivateTowerServicesServer;
use crate::protos::public_tower_services_server::PublicTowerServicesServer;
use crate::responder::Responder;
use crate::watcher::Watcher;

use teos_common::UserId;

async fn get_last_n_blocks<B, T>(
    poller: &mut ChainPoller<B, T>,
    mut last_known_block: ValidatedBlockHeader,
    n: usize,
) -> Vec<ValidatedBlock>
where
    B: DerefMut<Target = T> + Sized + Send + Sync,
    T: BlockSource,
{
    let mut last_n_blocks = Vec::new();
    for _ in 0..n {
        let block = poller.fetch_block(&last_known_block).await.unwrap();
        last_known_block = poller
            .look_up_previous_header(&last_known_block)
            .await
            .unwrap();
        last_n_blocks.push(block);
    }

    last_n_blocks
}

pub async fn run(conf: Config, tower_sk:SecretKey, tower_pk: PublicKey, dbm: std::sync::Arc<std::sync::Mutex<DBM>>, finished_startup_trigger: triggered::Trigger) {
    // Set log level
    if conf.debug {
        init_with_level(log::Level::Debug).unwrap()
    } else {
        init_with_level(log::Level::Info).unwrap()
    }

    log::info!("tower_id: {}", tower_pk);

    // Initialize our bitcoind client
    let (bitcoin_cli, bitcoind_reachable) = match BitcoindClient::new(
        &conf.btc_rpc_connect,
        conf.btc_rpc_port,
        &conf.btc_rpc_user,
        &conf.btc_rpc_password,
    )
    .await
    {
        Ok(client) => (
            Arc::new(client),
            Arc::new((Mutex::new(true), Condvar::new())),
        ),
        Err(e) => {
            let e_msg = match e.kind() {
                ErrorKind::InvalidData => "invalid btcrpcuser or btcrpcpassword".into(),
                _ => e.to_string(),
            };
            log::error!("Failed to connect to bitcoind. Error: {}", e_msg);
            return;
        }
    };

    // FIXME: Temporary. We're using bitcoin_core_rpc and rust-lightning's rpc until they both get merged
    // https://github.com/rust-bitcoin/rust-bitcoincore-rpc/issues/166
    let schema = if !conf.btc_rpc_connect.starts_with("http") {
        "http://"
    } else {
        ""
    };
    let rpc = Arc::new(
        Client::new(
            &format!("{}{}:{}", schema, conf.btc_rpc_connect, conf.btc_rpc_port),
            Auth::UserPass(conf.btc_rpc_user.clone(), conf.btc_rpc_password.clone()),
        )
        .unwrap(),
    );
    let mut derefed = bitcoin_cli.deref();
    let tip = validate_best_block_header(&mut derefed).await.unwrap();
    log::info!("Last known block: {}", tip.header.block_hash());

    let mut poller = ChainPoller::new(&mut derefed, Network::from_str(&conf.btc_network).unwrap());
    let last_n_blocks = get_last_n_blocks(&mut poller, tip, 6).await;

    // Build components
    let gatekeeper = Arc::new(Gatekeeper::new(
        tip.height,
        conf.subscription_slots,
        conf.subscription_duration,
        conf.expiry_delta,
        dbm.clone(),
    ));

    let carrier = Carrier::new(rpc, bitcoind_reachable.clone(), tip.deref().height);
    let responder = Arc::new(Responder::new(carrier, gatekeeper.clone(), dbm.clone()));
    let watcher = Arc::new(Watcher::new(
        gatekeeper.clone(),
        responder.clone(),
        last_n_blocks,
        tip.height,
        tower_sk,
        UserId(tower_pk),
        dbm.clone(),
    ));

    if watcher.is_fresh() & responder.is_fresh() & gatekeeper.is_fresh() {
        log::info!("Fresh bootstrap");
    } else {
        log::info!("Bootstrapping from backed up data");
    }

    let (shutdown_trigger, shutdown_signal_rpc_api) = triggered::trigger();
    let shutdown_signal_internal_rpc_api = shutdown_signal_rpc_api.clone();
    let shutdown_signal_http = shutdown_signal_rpc_api.clone();
    let shutdown_signal_cm = shutdown_signal_rpc_api.clone();
    let shutdown_signal_tor = shutdown_signal_rpc_api.clone();

    // The ordering here actually matters. Listeners are called by order, and we want the gatekeeper to be called
    // last, so both the Watcher and the Responder can query the necessary data from it during data deletion.
    let listener = &(watcher.clone(), &(responder, gatekeeper));
    let cache = &mut UnboundedCache::new();
    let spv_client = SpvClient::new(tip, poller, cache, listener);
    let mut chain_monitor = ChainMonitor::new(
        spv_client,
        tip,
        dbm,
        conf.polling_delta,
        shutdown_signal_cm,
        bitcoind_reachable.clone(),
    )
    .await;

    // Get all the components up to date if there's a backlog of blocks
    chain_monitor.poll_best_tip().await;
    log::info!("Bootstrap completed. Turning on interfaces");

    // Build interfaces
    let rpc_api = Arc::new(InternalAPI::new(
        watcher,
        bitcoind_reachable.clone(),
        shutdown_trigger,
    ));
    let internal_rpc_api = rpc_api.clone();

    let rpc_api_addr = format!("{}:{}", conf.rpc_bind, conf.rpc_port)
        .parse()
        .unwrap();
    let internal_rpc_api_addr = format!("{}:{}", conf.internal_api_bind, conf.internal_api_port)
        .parse()
        .unwrap();
    let internal_rpc_api_uri = format!(
        "http://{}:{}",
        conf.internal_api_bind, conf.internal_api_port
    );
    let http_api_addr = format!("{}:{}", conf.api_bind, conf.api_port)
        .parse()
        .unwrap();

    // Start tasks
    let private_api_task = task::spawn(async move {
        Server::builder()
            .add_service(PrivateTowerServicesServer::new(rpc_api))
            .serve_with_shutdown(rpc_api_addr, shutdown_signal_rpc_api)
            .await
            .unwrap();
    });

    let public_api_task = task::spawn(async move {
        Server::builder()
            .add_service(PublicTowerServicesServer::new(internal_rpc_api))
            .serve_with_shutdown(internal_rpc_api_addr, shutdown_signal_internal_rpc_api)
            .await
            .unwrap();
    });

    let http_api_task = task::spawn(http::serve(
        http_api_addr,
        internal_rpc_api_uri,
        shutdown_signal_http,
    ));

    // Add Tor Onion Service for public API
    let mut tor_task = Option::None;
    if conf.tor_support {
        log::info!("Starting up hidden tor service");
        let tor_control_port = conf.tor_control_port;
        let api_port = conf.api_port;
        let onion_port = conf.onion_hidden_service_port;

        tor_task = Some(task::spawn(async move {
            tor::expose_onion_service(tor_control_port, api_port, onion_port, shutdown_signal_tor)
                .await
                .unwrap();
        }));
    }
    finished_startup_trigger.trigger();
    chain_monitor.monitor_chain().await;
    // Wait until shutdown
    http_api_task.await.unwrap();
    private_api_task.await.unwrap();
    public_api_task.await.unwrap();
    if conf.tor_support {
        tor_task.unwrap().await.unwrap();
    }

    log::info!("Shutting down tower");
}
