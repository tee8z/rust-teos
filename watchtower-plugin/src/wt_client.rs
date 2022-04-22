use std::collections::HashMap;

use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use tokio::fs;

use bitcoin::secp256k1::{PublicKey, Secp256k1, SecretKey};

use teos_common::cryptography;
use teos_common::{UserId, UserId as TowerId};

use crate::dbm::DBM;
use crate::TowerInfo;

#[derive(Clone)]
pub struct WTClient {
    pub dbm: Arc<Mutex<DBM>>,
    pub towers: HashMap<TowerId, TowerInfo>,
    pub user_sk: SecretKey,
    pub user_id: UserId,
}

impl WTClient {
    pub async fn new(data_dir: PathBuf) -> Self {
        // Create data dir if it does not exist
        fs::create_dir_all(&data_dir).await.unwrap_or_else(|e| {
            log::error!("Cannot create data dir: {:?}", e);
            std::process::exit(1);
        });

        let dbm = DBM::new(&data_dir.join("watchtowers_db.sql3")).unwrap();
        let (user_sk, user_id) = match dbm.load_client_key() {
            Ok(sk) => (
                sk,
                UserId(PublicKey::from_secret_key(&Secp256k1::new(), &sk)),
            ),
            Err(_) => {
                log::info!("Watchtower client keys not found. Creating a fresh set");
                let (sk, pk) = cryptography::get_random_keypair();
                dbm.store_client_key(&sk).unwrap();
                (sk, UserId(pk))
            }
        };

        log::info!(
            "Plugin watchtower client initialized. User id = {}",
            user_id
        );

        WTClient {
            towers: dbm.load_towers(),
            dbm: Arc::new(Mutex::new(dbm)),
            user_sk,
            user_id,
        }
    }
}
