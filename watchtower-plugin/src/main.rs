use std::collections::HashSet;
use std::convert::TryFrom;
use std::env;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use hex::FromHex;
use home::home_dir;
use serde_json::json;
use tokio::io::{stdin, stdout};

use bitcoin::util::psbt::serialize::Deserialize;
use bitcoin::Transaction;

use cln_plugin::options::{ConfigOption, Value};
use cln_plugin::{anyhow, Builder, Error, Plugin};

use teos_common::appointment::{Appointment, Locator};
use teos_common::cryptography;
use teos_common::protos as common_msgs;
use teos_common::receipts::RegistrationReceipt;
use teos_common::UserId as TowerId;

use watchtower_plugin::convert::{CommitmentRevocation, GetAppointmentParams, RegisterParams};
use watchtower_plugin::net::http::{
    add_appointment, post_request, process_post_response, ApiResponse, RequestError,
};
use watchtower_plugin::wt_client::WTClient;
use watchtower_plugin::{TowerInfo, TowerStatus};

fn to_cln_error(e: RequestError) -> Error {
    match e {
        RequestError::ConnectionError(e) => anyhow!(e),
        RequestError::DeserializeError(e) => anyhow!(e),
        RequestError::Unexpected(e) => anyhow!(e),
    }
}

/// Registers the client to a given tower.
///
/// Accepted tower_id formats:
///     - tower_id@host:port
///     - tower_id host port
///     - tower_id@host (will default port to DEFAULT_PORT)
///     - tower_id host (will default port to DEFAULT_PORT)
async fn register(
    plugin: Plugin<Arc<Mutex<WTClient>>>,
    v: serde_json::Value,
) -> Result<serde_json::Value, Error> {
    let params = RegisterParams::try_from(v).map_err(|x| anyhow!(x))?;
    let host = params.host.unwrap_or("localhost".into());
    let tower_id = params.tower_id;
    let user_id = plugin.state().lock().unwrap().user_id;

    // FIXME: This is a workaround. Ideally, `cln_plugin::options::Value` will implement `as_u64` so we can simply call and unwrap
    // given that we are certain the option exists.
    let port = params.port.unwrap_or(
        if let Value::Integer(x) = plugin.option("watchtower-port").unwrap() {
            x as u16
        } else {
            // We will never end up here, but we need to define an else. Should be fixed alongside the previous fixme.
            9814
        },
    );

    let mut tower_net_addr = format!("{}:{}", host, port);
    if !tower_net_addr.starts_with("http") {
        tower_net_addr = format!("http://{}", tower_net_addr)
    }

    let register_endpoint = format!("{}/register", tower_net_addr);
    log::info!("Registering in the Eye of Satoshi (tower_id={})", tower_id);

    let (receipt, signature) = process_post_response(
        post_request(reqwest::Client::new().post(register_endpoint).json(
            &common_msgs::RegisterRequest {
                user_id: user_id.to_vec(),
            },
        ))
        .await,
    )
    .await
    .map(|r: common_msgs::RegisterResponse| {
        (
            RegistrationReceipt::new(user_id, r.available_slots, r.subscription_expiry),
            r.subscription_signature,
        )
    })
    .map_err(|e| {
        let mut state = plugin.state().lock().unwrap();
        if let Some(tower) = state.towers.get_mut(&tower_id) {
            if e.is_connection() {
                tower.status = TowerStatus::TemporaryUnreachable;
            }
        }
        to_cln_error(e)
    })?;

    if !cryptography::verify(&receipt.to_vec(), &signature, &tower_id.0.clone()) {
        return Err(anyhow!(
            "Registration receipt contains bad signature. Are you using the right tower_id?"
        ));
    }

    log::info!(
        "Registration succeeded. Available slots: {}",
        receipt.available_slots()
    );

    let mut state = plugin.state().lock().unwrap();
    state.towers.insert(
        tower_id,
        TowerInfo::new(
            tower_net_addr.clone(),
            receipt.available_slots(),
            receipt.subscription_expiry(),
        ),
    );

    state
        .dbm
        .lock()
        .unwrap()
        .store_tower_record(tower_id, tower_net_addr, &receipt)
        .unwrap();

    Ok(json!(receipt))
}

/// Gets information about an appointment from the tower.
async fn get_appointment(
    plugin: Plugin<Arc<Mutex<WTClient>>>,
    v: serde_json::Value,
) -> Result<serde_json::Value, Error> {
    let params = GetAppointmentParams::try_from(v).map_err(|x| anyhow!(x))?;

    let user_sk = plugin.state().lock().unwrap().user_sk;
    let tower_net_addr = {
        let state = plugin.state().lock().unwrap();
        if let Some(info) = state.towers.get(&params.tower_id) {
            Ok(info.net_addr.clone())
        } else {
            Err(anyhow!("Unknown tower id: {}", params.tower_id))
        }
    }?;

    let get_appointment_endpoint = format!("{}/get_appointment", tower_net_addr);
    let signature = cryptography::sign(
        format!("get appointment {}", params.locator).as_bytes(),
        &user_sk,
    )
    .unwrap();

    let response = process_post_response(
        post_request(reqwest::Client::new().post(get_appointment_endpoint).json(
            &common_msgs::GetAppointmentRequest {
                locator: params.locator.to_vec(),
                signature,
            },
        ))
        .await,
    )
    .await
    .map(|r: ApiResponse<common_msgs::GetAppointmentResponse>| r)
    .map_err(|e| {
        let mut state = plugin.state().lock().unwrap();
        if let Some(tower) = state.towers.get_mut(&params.tower_id) {
            if e.is_connection() {
                tower.status = TowerStatus::TemporaryUnreachable;
            }
        }
        to_cln_error(e)
    })?;

    Ok(json!(response))
}

/// Lists all the registered towers.
///
/// The given information comes from memory, so it is summarized.
async fn list_towers(
    plugin: Plugin<Arc<Mutex<WTClient>>>,
    _: serde_json::Value,
) -> Result<serde_json::Value, Error> {
    Ok(json!(plugin.state().lock().unwrap().towers))
}

/// Gets information about a given tower.
///
/// Data comes from disk (DB), so all stored data is provided.
async fn get_tower_info(
    plugin: Plugin<Arc<Mutex<WTClient>>>,
    v: serde_json::Value,
) -> Result<serde_json::Value, Error> {
    let tower_id = TowerId::try_from(v).map_err(|e| anyhow!(e))?;
    let state = plugin.state().lock().unwrap();
    let tower_info = state
        .dbm
        .lock()
        .unwrap()
        .load_tower_record(tower_id)
        .map_err(|_| {
            anyhow!(
                "Cannot find {} within the known towers. Have you registered?",
                tower_id
            )
        })?;

    Ok(json!(tower_info))
}

/// Triggers a manual retry of a tower, tries to send all pending appointments to it.
///
/// Only works if the tower is unreachable or there's been a subscription error.
async fn retry_tower(
    _p: Plugin<Arc<Mutex<WTClient>>>,
    v: serde_json::Value,
) -> Result<serde_json::Value, Error> {
    Ok(v)
}

/// Sends an appointment to all registered towers for every new commitment transaction.
///
/// The appointment is built using the data provided by the backend (dispute txid and penalty transaction).
async fn on_commitment_revocation(
    plugin: Plugin<Arc<Mutex<WTClient>>>,
    v: serde_json::Value,
) -> Result<serde_json::Value, Error> {
    let commitment_revocation = serde_json::from_value::<CommitmentRevocation>(v)
        .map_err(|e| anyhow!("Cannot decode commitment_revocation data. Error: {}", e))?;
    log::debug!(
        "New commitment revocation received for channel {}. Commit number {}",
        commitment_revocation.channel_id,
        commitment_revocation.commit_num
    );

    // TODO: This could be simplified if Transaction implemented serde, then CommitmentRevocation::penalty_tx could be Transaction instead of String
    let penalty_tx =
        Transaction::deserialize(&Vec::from_hex(&commitment_revocation.penalty_tx).unwrap())
            .unwrap();
    // TODO: For now, to_self_delay is hardcoded to 42. Revisit and define it better / remove it when / if needed/
    let locator = Locator::new(commitment_revocation.commitment_txid);
    let appointment = Appointment::new(
        locator,
        cryptography::encrypt(&penalty_tx, &commitment_revocation.commitment_txid).unwrap(),
        42,
    );
    let signature = cryptography::sign(
        &appointment.to_vec(),
        &plugin.state().lock().unwrap().user_sk,
    )
    .unwrap();

    let mut towers = plugin.state().lock().unwrap().towers.clone();
    let mut pending_appointments = HashSet::new();
    for (tower_id, tower_info) in towers.iter_mut() {
        if tower_info.status.is_reachable() {
            let response = add_appointment(*tower_id, tower_info, &appointment, &signature).await;
        } else {
            if tower_info.status.is_subscription_error() {
                log::warn!(
                    "There is a subscription issue with {}. Adding appointment to pending",
                    tower_id,
                );
            } else {
                log::warn!(
                    "{} is {}. Adding appointment to pending",
                    tower_id,
                    tower_info.status
                );
            }
            pending_appointments.insert(tower_id);
        }
    }

    // TODO: This is pretty ugly, but I don't know how to handle it within the previous for given there's an async call
    // and the compiler complains about locked state not being Send.
    let mut state = plugin.state().lock().unwrap();
    state
        .dbm
        .lock()
        .unwrap()
        .store_appointment(locator, appointment, signature)
        .unwrap();

    for tower_id in pending_appointments {
        state
            .towers
            .get_mut(tower_id)
            .unwrap()
            .pending_appointments
            .insert(locator);

        state
            .dbm
            .lock()
            .unwrap()
            .store_pending_appointment(locator, *tower_id)
            .unwrap();
    }

    // FIXME: Ask cdecker: Do hooks need to return something?
    Ok(json!(r#" {"result": continue}"#))
}

#[tokio::main]
async fn main() -> Result<(), Error> {
    let data_dir = match env::var("TOWERS_DATA_DIR") {
        Ok(v) => PathBuf::from(v),
        Err(_) => home_dir().unwrap().join(".watchtower"),
    };

    let builder = Builder::new(
        Arc::new(Mutex::new(WTClient::new(data_dir).await)),
        stdin(),
        stdout(),
    )
    .option(ConfigOption::new(
        "watchtower-port",
        Value::Integer(9814),
        "tower API port",
    ))
    .option(ConfigOption::new(
        "watchtower-max-retries",
        Value::Integer(30),
        "maximum POST retries if the tower is unreachable",
    ))
    .rpcmethod(
        "registertower",
        "Registers the client public key (user id) with the tower.",
        register,
    )
    .rpcmethod(
        "getappointment",
        "Gets appointment data from the tower given the tower id and the locator.",
        get_appointment,
    )
    .rpcmethod("listtowers", "Lists all registered towers.", list_towers)
    .rpcmethod(
        "gettowerinfo",
        "Shows the info about a given tower.",
        get_tower_info,
    )
    .rpcmethod(
        "retrytower",
        "Retries to send pending appointment to an unreachable tower.",
        retry_tower,
    )
    .hook("commitment_revocation", on_commitment_revocation);

    let plugin = builder.start().await.unwrap();
    plugin.join().await
}
