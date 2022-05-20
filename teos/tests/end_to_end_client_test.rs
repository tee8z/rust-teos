extern crate teos;

use std::collections::HashMap;
use std::time::Duration;
use teos::protos as msgs;
use tokio;
use tokio::time::sleep;
use teos_common::cryptography;
use serde_json::json;
mod common;
/*
#[tokio::test]
async fn test_commands_registered() {
    // test setup
    let (finished_startup_trigger, startup_signal) = triggered::trigger();
    let conf = teos::config::Config::default();
    let (tower_sk, tower_pk, dbm) = common::setup(conf.clone()).await.unwrap();

    log::info!("tower_id: {}", tower_pk);
    tokio::spawn(async move {
        let conf_cl = conf.clone();
        //NOTE: need to assign bitcoind output to something, otherwise it will spin down and the tower wont be able to connect to it
        let (conf_chg, _bitcoind) = common::start_bitcoind(conf_cl.clone()).await.unwrap();
        teos::startup::run(conf_chg, tower_sk, tower_pk, dbm, finished_startup_trigger).await
    });

    startup_signal.clone().await;
    //NOTE: needs a few more seconds for the http server to complete it's setup
    sleep(Duration::from_millis(100)).await;

    // preparing request to register
    let conf = teos::config::Config::default();
    let url = format!("http://{}:{}/register", conf.api_bind, conf.api_port);

    let mut map = HashMap::new();
    map.insert(
        "user_id",
        "02fa501fe552d26687fa07e32cad4b12def4fe493c676fe1bd58b35655c53fadd3",
    );

    let client = reqwest::Client::new();
    let resp = client
        .post(&url)
        .header(reqwest::header::ACCEPT, "application/json")
        .header(reqwest::header::CONTENT_TYPE, "application/json")
        .json(&map)
        .send()
        .await
        .unwrap();

    assert_eq!(200, resp.status());
    let resp_body = resp.json::<msgs::RegisterResponse>().await.unwrap();
    assert!(matches!(resp_body, msgs::RegisterResponse { .. }));
}

*/
#[tokio::test]
async fn test_command_non_registered() {
    // test setup
    let (finished_startup_trigger, startup_signal) = triggered::trigger();
    let conf = teos::config::Config::default();
    let (tower_sk, tower_pk, dbm) = common::setup(conf.clone()).await.unwrap();

    log::info!("tower_id: {}", tower_pk);
    tokio::spawn(async move {
        let conf_cl = conf.clone();
        //NOTE: need to assign bitcoind output to something, otherwise it will spin down and the tower wont be able to connect to it
        let (conf_chg, _bitcoind) = common::start_bitcoind(conf_cl.clone()).await.unwrap();
        teos::startup::run(conf_chg, tower_sk, tower_pk, dbm, finished_startup_trigger).await
    });

    startup_signal.clone().await;
    //NOTE: needs a few more seconds for the http server to complete it's setup
    sleep(Duration::from_millis(100)).await;
    let client = reqwest::Client::new();
    let conf = teos::config::Config::default();

    let url_get_appointment = format!("http://{}:{}/get_appointment", conf.api_bind, conf.api_port);
    let resp_get_appointment = client
        .post(&url_get_appointment)
        .header(reqwest::header::ACCEPT, "application/json")
        .header(reqwest::header::CONTENT_TYPE, "application/json")
        .json(&msgs::GetAppointmentRequest {
            locator:Vec::from([1,2,3,4,5,6,7,8,9,10,11,12,13,14,15,16]),
            signature:"dhdhdhd".to_string(),
        })
        .send()
        .await
        .unwrap();

    assert_eq!(401, resp_get_appointment.status());
    let resp_str = resp_get_appointment.text().await.unwrap();
    assert_eq!(json!({"error":"User cannot be authenticated","error_code":7}).to_string(),resp_str);
    

    let url_get_subscription_info = format!(
        "http://{}:{}/get_subscription_info",
        conf.api_bind, conf.api_port
    );

    let resp_get_subscription_info = client
        .post(&url_get_subscription_info)
        .header(reqwest::header::ACCEPT, "application/json")
        .header(reqwest::header::CONTENT_TYPE, "application/json")
        .json(&msgs::GetSubscriptionInfoRequest{
            signature:"dhdhdhd".to_string(),
        })
        .send()
        .await
        .unwrap();
   
    assert_eq!(401, resp_get_subscription_info.status());
    let resp_str = resp_get_subscription_info.text().await.unwrap();
    assert_eq!(json!({"error":"User not found. Have you registered?","error_code":7}).to_string(),resp_str);
        

    // preparing request to add_appointment that should fail
    let url_add_appointment = format!("http://{}:{}/add_appointment", conf.api_bind, conf.api_port);

    let resp_add_appointment = client
        .post(&url_add_appointment)
        .header(reqwest::header::ACCEPT, "application/json")
        .header(reqwest::header::CONTENT_TYPE, "application/json")
        .json(&msgs::AddAppointmentRequest{
            appointment: Some(common::generate_dummy_appointment(None)),
            signature:"dhdhdhd".to_string(),
        })
        .send()
        .await
        .unwrap();

    assert_eq!(401, resp_add_appointment.status());
    let resp_str = resp_add_appointment.text().await.unwrap();
    assert_eq!(json!({"error":"User cannot be authenticated","error_code":7}).to_string(),resp_str);
        
}
