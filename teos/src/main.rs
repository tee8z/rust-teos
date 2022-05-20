use std::fs;
use bitcoin::secp256k1::{PublicKey, Secp256k1, SecretKey};
use structopt::StructOpt;
use teos::config::{self, Config, Opt};
use teos::dbm::DBM;
use teos::startup;
use std::sync::{Arc, Mutex};
use teos_common::cryptography::get_random_keypair;


fn create_new_tower_keypair(db: &DBM) -> (SecretKey, PublicKey) {
    let (sk, pk) = get_random_keypair();
    db.store_tower_key(&sk).unwrap();
    (sk, pk)
}


#[tokio::main]
async fn main() {
    let opt = Opt::from_args();
    let path = config::data_dir_absolute_path(opt.data_dir.clone());
    let (finished_startup_trigger, _) = triggered::trigger();
    // Create data dir if it does not exist
    fs::create_dir_all(&path).unwrap_or_else(|e| {
        eprintln!("Cannot create data dir: {:?}", e);
        std::process::exit(1);
    });

    // Load conf (from file or defaults) and patch it with the command line parameters received (if any)
    let mut conf = config::from_file::<Config>(path.join("teos.toml"));
    conf.patch_with_options(opt);
    conf.verify().unwrap_or_else(|e| {
        eprintln!("{}", e);
        std::process::exit(1);
    });
    // Create network dir
    let path_network = path.join(conf.btc_network.clone());
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
        if conf.overwrite_key {
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
    log::info!("tower_id: {}", tower_pk);

    startup::run(conf, tower_sk,tower_pk,dbm, finished_startup_trigger).await;
}
