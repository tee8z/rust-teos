use std::env;
use std::fs;
use std::io::Error;


use bitcoind::bitcoincore_rpc::RpcApi;
use bitcoind::downloaded_exe_path;
use crate::teos::config;
use crate::teos::dbm::DBM;
use std::sync::{Arc, Mutex};
use bitcoin::secp256k1::{PublicKey,Secp256k1, SecretKey};
use teos_common::cryptography::{encrypt, get_random_bytes, get_random_keypair};
use bitcoin::hash_types::Txid;
use bitcoin::hashes::hex::FromHex;
use bitcoin::hashes::Hash;
use bitcoin::consensus;
use teos::protos as msgs;
use teos_common::appointment::{Appointment, Locator};
//TODO: Add access to bitcoind process in an Arc<Mutex> to mock transactions happening in integration testing

fn create_new_tower_keypair(db: &DBM) -> (SecretKey, PublicKey) {
    let (sk, pk) = get_random_keypair();
    db.store_tower_key(&sk).unwrap();
    (sk, pk)
}

pub async fn start_bitcoind(mut teosconf: config::Config) -> Result<(config::Config, bitcoind::BitcoinD), Error>{ 
        // Create bitcoind deamond for testing
        let bitcoind_exe = env::var("BITCOIND_EXE")
        .ok()
        .or_else(|| downloaded_exe_path())
        .expect("version feature or env BITCOIND_EXE is required for tests");
    
        let mut conf = bitcoind::Conf::default(); 
        conf.view_stdout = true;
        let bitcoind = bitcoind::BitcoinD::with_conf(bitcoind_exe, &conf).unwrap();
    
        let testwallet = bitcoind.create_wallet("testwallet").unwrap();
        let test_address = testwallet.get_new_address(None, None).unwrap();
    
        // Need to mine 100 blocks before coins can be spent
        bitcoind.client.generate_to_address(101, &test_address).unwrap();
    
        let cookie = bitcoind.params.cookie_file.to_str().unwrap();
        let contents = fs::read_to_string(cookie).unwrap();
    
        // The bitcoind library only works with connecting via a cookie auth but the watchtower uses username/password
        // this pulls out the username/password from the cookie file and allows the watchtower to connect over rpc
        let auth_params: Vec<&str> = contents.split(":").collect();
        let username = auth_params[0];
        let pwd = auth_params[1];
        teosconf.btc_rpc_user = username.to_string();
        teosconf.btc_rpc_password = pwd.to_string();
        teosconf.btc_rpc_port = bitcoind.params.rpc_socket.port();
        teosconf.btc_rpc_connect = format!("{}",bitcoind.params.rpc_socket.ip());
        teosconf.debug = true;
        Ok((teosconf, bitcoind))
}

pub async fn setup(teosconf: config::Config) -> Result<(SecretKey, PublicKey, std::sync::Arc<std::sync::Mutex<teos::dbm::DBM>>), Error>{
    let path = config::data_dir_absolute_path("~/.teos".to_owned());
    // Create network dir
    let path_network = path.join(teosconf.btc_network.clone());
    fs::create_dir_all(&path_network).unwrap_or_else(|e| {
        eprintln!("Cannot create network dir: {:?}", e);
        std::process::exit(1);
    });
    let dbm = Arc::new(Mutex::new(
        DBM::new(path_network.join("teos_db.sql3")).unwrap(),
    ));

    // Load tower secret key or create a fresh one if none is found. If overwrite key is set, create a new
    // key straightaway
    let (tower_sk, tower_pk) = {
        let locked_db = dbm.lock().unwrap();
        if teosconf.overwrite_key {
            log::info!("Overwriting tower keys");
            create_new_tower_keypair(&locked_db)
        } else {
            match locked_db.load_tower_key() {
                Ok(sk) => (sk, PublicKey::from_secret_key(&Secp256k1::new(), &sk)),
                Err(_) => {
                    log::info!("Tower keys not found. Creating a fresh set");
                    create_new_tower_keypair(&locked_db)
                }
            }
        }
    };
    Ok((tower_sk, tower_pk, dbm.clone()))
}

pub fn generate_dummy_appointment(dispute_txid: Option<&Txid>) -> msgs::Appointment {
    let dispute_txid = match dispute_txid {
        Some(l) => *l,
        None => {
            let prev_txid_bytes = get_random_bytes(32);
            Txid::from_slice(&prev_txid_bytes).unwrap()
        }
    };
    static TX_HEX: &str =  "010000000001010000000000000000000000000000000000000000000000000000000000000000ffffffff54038e830a1b4d696e656420627920416e74506f6f6c373432c2005b005e7a0ae3fabe6d6d7841cd582ead8ea5dd8e3de1173cae6fcd2a53c7362ebb7fb6f815604fe07cbe0200000000000000ac0e060005f90000ffffffff04d9476026000000001976a91411dbe48cc6b617f9c6adaf4d9ed5f625b1c7cb5988ac0000000000000000266a24aa21a9ed7248c6efddd8d99bfddd7f499f0b915bffa8253003cc934df1ff14a81301e2340000000000000000266a24b9e11b6d7054937e13f39529d6ad7e685e9dd4efa426f247d5f5a5bed58cdddb2d0fa60100000000000000002b6a2952534b424c4f434b3a054a68aa5368740e8b3e3c67bce45619c2cfd07d4d4f0936a5612d2d0034fa0a0120000000000000000000000000000000000000000000000000000000000000000000000000";

    let tx_bytes = Vec::from_hex(TX_HEX).unwrap();
    let penalty_tx = consensus::deserialize(&tx_bytes).unwrap();

    let encrypted_blob = encrypt(&penalty_tx, &dispute_txid).unwrap();

    return msgs::Appointment{
        encrypted_blob: encrypted_blob,
        locator: get_random_bytes(16),
        to_self_delay:21
    }
}