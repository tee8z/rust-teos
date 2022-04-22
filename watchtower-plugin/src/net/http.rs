use reqwest::{RequestBuilder, Response};
use serde::{de::DeserializeOwned, Deserialize, Serialize};

use teos_common::appointment::Appointment;
use teos_common::cryptography;
use teos_common::protos as common_msgs;
use teos_common::receipts::AppointmentReceipt;
use teos_common::UserId as TowerId;

use crate::TowerInfo;

#[derive(Serialize, Deserialize, Debug)]
#[serde(untagged)]
pub enum ApiResponse<T> {
    Response(T),
    Error(ApiError),
}

#[derive(Serialize, Deserialize, Debug)]
pub struct ApiError {
    pub error: String,
    pub error_code: u8,
}

#[derive(Debug)]
pub enum RequestError {
    ConnectionError(String),
    DeserializeError(String),
    Unexpected(String),
}

impl RequestError {
    pub fn is_connection(&self) -> bool {
        match self {
            RequestError::ConnectionError(_) => true,
            _ => false,
        }
    }
}

#[derive(Debug)]
pub enum AddAppointmentError {
    RequestError(RequestError),
    ApiError(ApiError),
    SignatureError(SignatureError),
}

impl From<RequestError> for AddAppointmentError {
    fn from(r: RequestError) -> Self {
        AddAppointmentError::RequestError(r)
    }
}

#[derive(Debug)]
pub struct SignatureError {
    receipt: AppointmentReceipt,
    signature: String,
    recovered_id: TowerId,
}

impl SignatureError {
    pub fn new(receipt: AppointmentReceipt, signature: String, recovered_id: TowerId) -> Self {
        SignatureError {
            receipt,
            signature,
            recovered_id,
        }
    }
}

pub async fn add_appointment(
    tower_id: TowerId,
    tower_info: &mut TowerInfo,
    appointment: &Appointment,
    signature: &String,
) -> Result<(String, u32), AddAppointmentError> {
    log::debug!(
        "Sending appointment {} to tower {}",
        appointment.locator,
        tower_id
    );
    let response = send_appointment(tower_id, tower_info, appointment, signature).await?;
    log::debug!("Appointment accepted and signed by {}", tower_id);
    log::debug!("Remaining slots: {}", response.available_slots);
    log::debug!("Start block: {}", response.start_block);

    Ok((response.signature, response.available_slots))
}

pub async fn send_appointment(
    tower_id: TowerId,
    tower_info: &mut TowerInfo,
    appointment: &Appointment,
    signature: &String,
) -> Result<common_msgs::AddAppointmentResponse, AddAppointmentError> {
    let request_data = common_msgs::AddAppointmentRequest {
        appointment: Some(appointment.clone().into()),
        signature: signature.clone(),
    };

    match process_post_response(
        post_request(
            reqwest::Client::new()
                .post(format!("{}/add_appointment", tower_info.net_addr))
                .json(&request_data),
        )
        .await,
    )
    .await
    .map(|r: ApiResponse<common_msgs::AddAppointmentResponse>| r)?
    {
        ApiResponse::Response(r) => {
            let receipt = AppointmentReceipt::new(signature.clone(), r.start_block);
            let recovered_id =
                TowerId(cryptography::recover_pk(&receipt.serialize(), &r.signature).unwrap());
            if recovered_id == tower_id {
                Ok(r)
            } else {
                Err(AddAppointmentError::SignatureError(SignatureError::new(
                    receipt,
                    r.signature,
                    recovered_id,
                )))
            }
        }
        ApiResponse::Error(e) => Err(AddAppointmentError::ApiError(e)),
    }
}

pub async fn post_request(builder: RequestBuilder) -> Result<Response, RequestError> {
    builder.send().await.map_err(|e| {
        log::error!("{}", e);
        if e.is_connect() | e.is_timeout() {
            RequestError::ConnectionError("Cannot connect to the tower. Connection refused".into())
        } else {
            RequestError::Unexpected("Unexpected error ocurred (see logs for more info)".into())
        }
    })
}

pub async fn process_post_response<T: DeserializeOwned>(
    post_request: Result<Response, RequestError>,
) -> Result<T, RequestError> {
    // TODO: Check if this can be switched fr a map. Not sure how to handle async with maps
    match post_request {
        Ok(r) => r.json().await.map_err(|e| {
            RequestError::DeserializeError(format!("Unexpected response body. Error: {}", e))
        }),
        Err(e) => Err(e),
    }
}
