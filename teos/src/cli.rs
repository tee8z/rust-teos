use serde_json::to_string_pretty as pretty_json;
use std::fs;
use std::str::FromStr;
use structopt::StructOpt;
use tonic::Request;

use teos::cli_config::{Command, Config, Opt};
use teos::config;
use teos::protos as msgs;
use teos::protos::private_tower_services_client::PrivateTowerServicesClient;
use teos_common::UserId;

#[tokio::main]
async fn main() {
    let opt = Opt::from_args();
    let path = config::data_dir_absolute_path(opt.data_dir.clone());

    // Create data dir if it does not exist
    fs::create_dir_all(&path).unwrap_or_else(|e| {
        eprintln!("Cannot create data dir: {:?}", e);
        std::process::exit(1);
    });

    let command = opt.command.clone();

    // Load conf (from file or defaults) and patch it with the command line parameters received (if any)
    let mut conf = config::from_file::<Config>(path.join("teos.toml"));
    conf.patch_with_options(opt);

    // Create gRPC client and send request
    let mut client =
        PrivateTowerServicesClient::connect(format!("http://{}:{}", conf.rpc_bind, conf.rpc_port))
            .await
            .unwrap_or_else(|e| {
                eprintln!("Cannot connect to the tower. Connection refused");
                if conf.debug {
                    eprintln!("{:?}", e);
                }
                std::process::exit(1);
            });

    match command {
        Command::GetAllAppointments => {
            let appointments = client.get_all_appointments(Request::new(())).await.unwrap();
            println!("{}", pretty_json(&appointments.into_inner()).unwrap());
        }
        Command::GetTowerInfo => {
            let info = client.get_tower_info(Request::new(())).await.unwrap();
            println!("{}", pretty_json(&info.into_inner()).unwrap())
        }
        Command::GetUsers => {
            let users = client.get_users(Request::new(())).await.unwrap();
            println!("{}", pretty_json(&users.into_inner()).unwrap());
        }
        Command::GetUser(data) => {
            match UserId::from_str(&data.user_id) {
                Ok(user_id) => {
                    match client
                        .get_user(Request::new(msgs::GetUserRequest {
                            user_id: user_id.to_vec(),
                        }))
                        .await
                    {
                        Ok(response) => {
                            println!("{}", pretty_json(&response.into_inner()).unwrap())
                        }
                        Err(status) => println!("{}", status.message()),
                    }
                }
                Err(e) => println!("{}", e),
            };
        }
        Command::Stop => {
            println!("Shutting down tower");
            client.stop(Request::new(())).await.unwrap();
        }
    };
}
