use serde::{ser::SerializeSeq, Deserialize, Serialize, Serializer};
use std::convert::Infallible;
use std::error::Error;
use std::net::SocketAddr;
use tonic::transport::Channel;
use triggered::Listener;
use warp::{http::StatusCode, reject, reply, Filter, Rejection, Reply};

use teos_common::appointment::LOCATOR_LEN;
use teos_common::protos as common_msgs;
use teos_common::{errors, USER_ID_LEN};

use crate::protos::public_tower_services_client::PublicTowerServicesClient;

// TODO: Limit the body length for /add_appointment should not be needed, since slots are consumed proportionally to it.
// Setting a limit for now just to prevent spam to some extend, but this is likely to be lifted.
const REGISTER_BODY_LEN: u64 = 87;
const ADD_APPOINTMENT_BODY_LEN: u64 = 2048;
const GET_APPOINTMENT_BODY_LEN: u64 = 178;
const GET_SUBSCRIPTION_INFO_BODY_LEN: u64 = 127;

#[derive(Serialize, Deserialize, Debug, PartialEq, Eq)]
pub(crate) struct ApiError {
    error: String,
    error_code: u8,
}

impl reject::Reject for ApiError {}

impl ApiError {
    fn new(error: String, error_code: u8) -> Self {
        ApiError { error, error_code }
    }

    fn missing_field(field_name: &str) -> Rejection {
        reject::custom(Self::new(
            format!("missing field `{}`", field_name),
            errors::MISSING_FIELD,
        ))
    }

    fn empty_field(field_name: &str) -> Rejection {
        reject::custom(Self::new(
            format!("`{}` field is empty", field_name),
            errors::EMPTY_FIELD,
        ))
    }

    fn wrong_field_length(field_name: &str, field_size: usize, expected_size: usize) -> Rejection {
        reject::custom(Self::new(
            format!(
                "Wrong `{}` field size. Expected {}, received {}",
                field_name, expected_size, field_size
            ),
            errors::WRONG_FIELD_SIZE,
        ))
    }
}

pub fn serialize_vec_bytes<S>(v: &[Vec<u8>], s: S) -> Result<S::Ok, S::Error>
where
    S: Serializer,
{
    let mut seq = s.serialize_seq(Some(v.len()))?;
    for element in v.iter() {
        seq.serialize_element(&hex::encode(element))?;
    }
    seq.end()
}

fn with_grpc(
    grpc_endpoint: PublicTowerServicesClient<Channel>,
) -> impl Filter<Extract = (PublicTowerServicesClient<Channel>,), Error = Infallible> + Clone {
    warp::any().map(move || grpc_endpoint.clone())
}

fn match_status(s: &tonic::Status) -> (StatusCode, u8) {
    let mut status_code = StatusCode::BAD_REQUEST;
    let error_code = match s.code() {
        tonic::Code::InvalidArgument => errors::WRONG_FIELD_FORMAT,
        tonic::Code::NotFound => {
            status_code = StatusCode::NOT_FOUND;
            errors::APPOINTMENT_NOT_FOUND
        }
        tonic::Code::AlreadyExists => errors::APPOINTMENT_ALREADY_TRIGGERED,
        tonic::Code::ResourceExhausted => errors::REGISTRATION_RESOURCE_EXHAUSTED,
        tonic::Code::Unauthenticated => {
            status_code = StatusCode::UNAUTHORIZED;
            errors::INVALID_SIGNATURE_OR_SUBSCRIPTION_ERROR
        }
        tonic::Code::Unavailable => {
            status_code = StatusCode::SERVICE_UNAVAILABLE;
            errors::SERVICE_UNAVAILABLE
        }
        _ => {
            log::debug!("Unexpected error ocurred: {}", s.message());
            errors::UNEXPECTED_ERROR
        }
    };

    (status_code, error_code)
}

fn parse_grpc_response<T: serde::Serialize>(
    result: Result<tonic::Response<T>, tonic::Status>,
) -> (reply::Json, StatusCode) {
    match result {
        Ok(r) => {
            let inner = r.into_inner();
            log::info!("Request succeeded");
            log::debug!("Response: {}", serde_json::json!(inner));
            (reply::json(&inner), StatusCode::OK)
        }
        Err(s) => {
            let (status_code, error_code) = match_status(&s);
            log::info!("Request failed, error_code={}", error_code);
            log::debug!("Response: {}", serde_json::json!(s.message()));
            (
                reply::json(&ApiError::new(s.message().into(), error_code)),
                status_code,
            )
        }
    }
}

async fn register(
    req: common_msgs::RegisterRequest,
    addr: Option<std::net::SocketAddr>,
    mut grpc_conn: PublicTowerServicesClient<Channel>,
) -> std::result::Result<impl Reply, Rejection> {
    match addr {
        Some(a) => log::info!("Received register request from {}", a),
        None => log::info!("Received register request from unknown address"),
    }

    let user_id = req.user_id.clone();
    if user_id.is_empty() {
        return Err(ApiError::empty_field("user_id"));
    }
    if user_id.len() != USER_ID_LEN {
        return Err(ApiError::wrong_field_length(
            "user_id",
            user_id.len(),
            USER_ID_LEN,
        ));
    }

    let (body, status) = parse_grpc_response(grpc_conn.register(req).await);
    Ok(reply::with_status(body, status))
}

async fn add_appointment(
    req: common_msgs::AddAppointmentRequest,
    addr: Option<std::net::SocketAddr>,
    mut grpc_conn: PublicTowerServicesClient<Channel>,
) -> std::result::Result<impl Reply, Rejection> {
    match addr {
        Some(a) => log::info!("Received add_appointment request from {}", a),
        None => log::info!("Received add_appointment request from unknown address"),
    }

    if let Some(a) = &req.appointment {
        if a.locator.is_empty() {
            return Err(ApiError::empty_field("locator"));
        }
        if a.locator.len() != LOCATOR_LEN {
            return Err(ApiError::wrong_field_length(
                "locator",
                a.locator.len(),
                LOCATOR_LEN,
            ));
        }
    } else {
        return Err(ApiError::missing_field("appointment"));
    }
    if req.signature.is_empty() {
        return Err(ApiError::empty_field("signature"));
    }

    let (body, status) = parse_grpc_response(grpc_conn.add_appointment(req).await);
    Ok(reply::with_status(body, status))
}

async fn get_appointment(
    req: common_msgs::GetAppointmentRequest,
    addr: Option<std::net::SocketAddr>,
    mut grpc_conn: PublicTowerServicesClient<Channel>,
) -> std::result::Result<impl Reply, Rejection> {
    match addr {
        Some(a) => log::info!("Received get_appointment request from {}", a),
        None => log::info!("Received get_appointment request from unknown address"),
    }

    if req.locator.is_empty() {
        return Err(ApiError::empty_field("locator"));
    }
    if req.locator.len() != LOCATOR_LEN {
        return Err(ApiError::wrong_field_length(
            "locator",
            req.locator.len(),
            LOCATOR_LEN,
        ));
    }
    if req.signature.is_empty() {
        return Err(ApiError::empty_field("signature"));
    }

    let (body, status) = parse_grpc_response(grpc_conn.get_appointment(req).await);
    Ok(reply::with_status(body, status))
}

async fn get_subscription_info(
    req: common_msgs::GetSubscriptionInfoRequest,
    addr: Option<std::net::SocketAddr>,
    mut grpc_conn: PublicTowerServicesClient<Channel>,
) -> std::result::Result<impl Reply, Rejection> {
    match addr {
        Some(a) => log::info!("Received get_subscription_info request from {}", a),
        None => log::info!("Received get_subscription_info request from unknown address"),
    }

    if req.signature.is_empty() {
        return Err(ApiError::empty_field("signature"));
    }

    let (body, status) = parse_grpc_response(grpc_conn.get_subscription_info(req).await);
    Ok(reply::with_status(body, status))
}

fn router(
    grpc_conn: PublicTowerServicesClient<Channel>,
) -> impl Filter<Extract = impl Reply, Error = Rejection> + Clone {
    let register = warp::post()
        .and(warp::path("register"))
        .and(warp::body::content_length_limit(REGISTER_BODY_LEN).and(warp::body::json()))
        .and(warp::addr::remote())
        .and(with_grpc(grpc_conn.clone()))
        .and_then(register);

    let add_appointment = warp::post()
        .and(warp::path("add_appointment"))
        .and(warp::body::content_length_limit(ADD_APPOINTMENT_BODY_LEN).and(warp::body::json()))
        .and(warp::addr::remote())
        .and(with_grpc(grpc_conn.clone()))
        .and_then(add_appointment);

    let get_appointment = warp::post()
        .and(warp::path("get_appointment"))
        .and(warp::body::content_length_limit(GET_APPOINTMENT_BODY_LEN).and(warp::body::json()))
        .and(warp::addr::remote())
        .and(with_grpc(grpc_conn.clone()))
        .and_then(get_appointment);

    let get_subscription_info = warp::post()
        .and(warp::path("get_subscription_info"))
        .and(
            warp::body::content_length_limit(GET_SUBSCRIPTION_INFO_BODY_LEN)
                .and(warp::body::json()),
        )
        .and(warp::addr::remote())
        .and(with_grpc(grpc_conn))
        .and_then(get_subscription_info);

    register
        .or(add_appointment)
        .or(get_appointment)
        .or(get_subscription_info)
        .recover(handle_rejection)
}

async fn handle_rejection(err: Rejection) -> Result<impl Reply, Rejection> {
    match err.find::<warp::body::BodyDeserializeError>() {
        Some(e) => {
            let mut error = e
                .source()
                .map(|cause| cause.to_string())
                .unwrap_or_else(|| "Invalid Body".to_string());

            let error_code = if error.contains("invalid type") {
                errors::WRONG_FIELD_TYPE
            } else if error.contains("missing field") {
                error = error.split(" at").take(1).next().unwrap_or(&error).into();
                errors::MISSING_FIELD
            } else if error.contains("Odd number of digits") | error.contains("Invalid character") {
                errors::WRONG_FIELD_FORMAT
            } else {
                errors::INVALID_REQUEST_FORMAT
            };
            Ok(reply::with_status(
                reply::json(&ApiError { error, error_code }),
                StatusCode::BAD_REQUEST,
            ))
        }
        None => match err.find::<ApiError>() {
            Some(x) => Ok(reply::with_status(reply::json(x), StatusCode::BAD_REQUEST)),
            None => Err(err),
        },
    }
}

pub async fn serve(http_bind: SocketAddr, grpc_bind: String, shutdown_signal: Listener) {
    let grpc_conn = PublicTowerServicesClient::connect(grpc_bind).await.unwrap();
    let (_, server) = warp::serve(router(grpc_conn))
        .bind_with_graceful_shutdown(http_bind, async { shutdown_signal.await });
    server.await
}

#[cfg(test)]
mod test_helpers {
    use super::*;

    use serde::de::DeserializeOwned;
    use serde_json::Value;
    use std::sync::Arc;
    use tokio::net::TcpListener;
    use tonic::transport::Server;

    use crate::api::internal::InternalAPI;
    use crate::protos::public_tower_services_server::PublicTowerServicesServer;
    use crate::test_utils::{create_api_with_config, ApiConfig};

    pub(crate) enum RequestBody<'a> {
        Jsonify(&'a str),
        DoNotJsonify(&'a str),
        Json(Value),
        Body(&'a str),
    }

    pub(crate) async fn run_tower_in_background_with_config(
        api_config: ApiConfig,
    ) -> (SocketAddr, Arc<InternalAPI>) {
        let internal_rpc_api = create_api_with_config(api_config).await;
        let cloned = internal_rpc_api.clone();

        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();

        tokio::spawn(async move {
            Server::builder()
                .add_service(PublicTowerServicesServer::new(internal_rpc_api))
                .serve_with_incoming(tokio_stream::wrappers::TcpListenerStream::new(listener))
                .await
                .unwrap();
        });

        (addr, cloned)
    }

    pub(crate) async fn run_tower_in_background() -> SocketAddr {
        run_tower_in_background_with_config(ApiConfig::default())
            .await
            .0
    }

    pub(crate) async fn check_api_error<'a>(
        endpoint: &str,
        body: RequestBody<'a>,
        server_addr: SocketAddr,
    ) -> (ApiError, StatusCode) {
        let grpc_conn = PublicTowerServicesClient::connect(format!(
            "http://{}:{}",
            server_addr.ip(),
            server_addr.port()
        ))
        .await
        .unwrap();

        let req = match body {
            RequestBody::Json(j) => warp::test::request().method("POST").path(endpoint).json(&j),
            RequestBody::DoNotJsonify(j) => {
                warp::test::request().method("POST").path(endpoint).json(&j)
            }
            RequestBody::Jsonify(j) => warp::test::request()
                .method("POST")
                .path(endpoint)
                .json(&serde_json::from_str::<Value>(j).unwrap()),
            RequestBody::Body(b) => warp::test::request().method("POST").path(endpoint).body(b),
        };

        let res = req.reply(&router(grpc_conn)).await;
        (
            serde_json::from_slice::<ApiError>(res.body()).unwrap(),
            res.status(),
        )
    }

    pub(crate) async fn request_to_api<B, T>(
        endpoint: &str,
        body: B,
        server_addr: SocketAddr,
    ) -> Result<T, serde_json::Error>
    where
        B: Serialize,
        T: DeserializeOwned,
    {
        let grpc_conn = PublicTowerServicesClient::connect(format!(
            "http://{}:{}",
            server_addr.ip(),
            server_addr.port()
        ))
        .await
        .unwrap();

        let res = warp::test::request()
            .method("POST")
            .path(endpoint)
            .json(&serde_json::json!(body))
            .reply(&router(grpc_conn))
            .await;

        serde_json::from_slice::<T>(res.body())
    }
}

#[cfg(test)]
mod tests_failures {
    use super::*;

    use super::test_helpers::{check_api_error, run_tower_in_background, RequestBody};
    use crate::test_utils::get_random_user_id;

    #[tokio::test]
    async fn test_no_json_request_body() {
        let server_addr = run_tower_in_background().await;
        let (api_error, status) =
            check_api_error("/register", RequestBody::Body(""), server_addr).await;
        assert!(api_error.error.contains("EOF while parsing"));
        assert_eq!(api_error.error_code, errors::INVALID_REQUEST_FORMAT);
        assert_eq!(status, StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn test_wrong_json_request_body() {
        let server_addr = run_tower_in_background().await;
        let (api_error, status) =
            check_api_error("/register", RequestBody::DoNotJsonify(""), server_addr).await;
        assert!(api_error.error.contains("expected struct"));
        assert_eq!(api_error.error_code, errors::WRONG_FIELD_TYPE);
        assert_eq!(status, StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn test_empty_json_request_body() {
        let server_addr = run_tower_in_background().await;
        let (api_error, status) =
            check_api_error("/register", RequestBody::Jsonify(r#"{}"#), server_addr).await;
        assert!(api_error.error.contains("missing field"));
        assert_eq!(api_error.error_code, errors::MISSING_FIELD);
        assert_eq!(status, StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn test_empty_field() {
        let server_addr = run_tower_in_background().await;
        let (api_error, status) = check_api_error(
            "/register",
            RequestBody::Jsonify(r#"{"user_id": ""}"#),
            server_addr,
        )
        .await;
        assert!(api_error.error.contains("field is empty"));
        assert_eq!(api_error.error_code, errors::EMPTY_FIELD);
        assert_eq!(status, StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn test_wrong_field_hex_encoding_odd() {
        let server_addr = run_tower_in_background().await;
        let (api_error, status) = check_api_error(
            "/register",
            RequestBody::Jsonify(r#"{"user_id": "a"}"#),
            server_addr,
        )
        .await;
        assert!(api_error.error.contains("Odd number of digits"));
        assert_eq!(api_error.error_code, errors::WRONG_FIELD_FORMAT);
        assert_eq!(status, StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn test_wrong_hex_encoding_character() {
        let server_addr = run_tower_in_background().await;
        let (api_error, status) =
        check_api_error("/register",  
        RequestBody::Jsonify(r#"{"user_id": "022fa2900ed7fc07b4e8ca3ea081e846245b0497944644aa78ea0b994ac22074dZ"}"#),
        server_addr
    ).await;

        assert!(api_error.error.contains("Invalid character"));
        assert_eq!(api_error.error_code, errors::WRONG_FIELD_FORMAT);
        assert_eq!(status, StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn test_wrong_field_size() {
        let server_addr = run_tower_in_background().await;
        let (api_error, status) = check_api_error(
            "/register",
            RequestBody::Jsonify(r#"{"user_id": "aa"}"#),
            server_addr,
        )
        .await;

        assert!(api_error.error.contains("Wrong `user_id` field size"));
        assert_eq!(api_error.error_code, errors::WRONG_FIELD_SIZE);
        assert_eq!(status, StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn test_wrong_field_type() {
        let server_addr = run_tower_in_background().await;
        let (api_error, status) = check_api_error(
            "/register",
            RequestBody::DoNotJsonify(r#"{"user_id": 1}"#),
            server_addr,
        )
        .await;
        assert!(api_error.error.contains("invalid type"));
        assert_eq!(api_error.error_code, errors::WRONG_FIELD_TYPE);
        assert_eq!(status, StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn test_request_missing_field() {
        // We'll use a different endpoint here since we need a json object with more than one field
        let server_addr = run_tower_in_background().await;
        let (api_error, status) = check_api_error(
            "/add_appointment",
            RequestBody::Jsonify(r#"{"signature": "aa"}"#),
            server_addr,
        )
        .await;
        assert!(api_error.error.contains("missing field"));
        assert_eq!(api_error.error_code, errors::MISSING_FIELD);
        assert_eq!(status, StatusCode::BAD_REQUEST);
    }

    // Tests with no json body return (passed trough handle_rejection)

    #[tokio::test]
    async fn test_empty_request_body() {
        let server_addr = run_tower_in_background().await;
        let grpc_conn = PublicTowerServicesClient::connect(format!(
            "http://{}:{}",
            server_addr.ip(),
            server_addr.port()
        ))
        .await
        .unwrap();

        let res = warp::test::request()
            .method("POST")
            .path("/register")
            .reply(&router(grpc_conn))
            .await;

        assert_eq!(res.status(), StatusCode::LENGTH_REQUIRED)
    }

    #[tokio::test]
    async fn test_payload_too_large() {
        let server_addr = run_tower_in_background().await;
        let grpc_conn = PublicTowerServicesClient::connect(format!(
            "http://{}:{}",
            server_addr.ip(),
            server_addr.port()
        ))
        .await
        .unwrap();

        let res = warp::test::request()
            .method("POST")
            .path("/register")
            .json(&format!("{}{}", get_random_user_id(), get_random_user_id()))
            .reply(&router(grpc_conn))
            .await;

        assert_eq!(res.status(), StatusCode::PAYLOAD_TOO_LARGE)
    }

    #[tokio::test]
    async fn test_wrong_endpoint() {
        let server_addr = run_tower_in_background().await;
        let grpc_conn = PublicTowerServicesClient::connect(format!(
            "http://{}:{}",
            server_addr.ip(),
            server_addr.port()
        ))
        .await
        .unwrap();

        let res = warp::test::request()
            .method("POST")
            .path("/")
            .json(&"")
            .reply(&router(grpc_conn))
            .await;

        assert_eq!(res.status(), StatusCode::NOT_FOUND)
    }

    #[tokio::test]
    async fn test_wrong_method() {
        let server_addr = run_tower_in_background().await;
        let grpc_conn = PublicTowerServicesClient::connect(format!(
            "http://{}:{}",
            server_addr.ip(),
            server_addr.port()
        ))
        .await
        .unwrap();

        let res = warp::test::request()
            .path("/")
            .json(&"")
            .reply(&router(grpc_conn))
            .await;

        assert_eq!(res.status(), StatusCode::METHOD_NOT_ALLOWED)
    }
}

#[cfg(test)]
mod tests_methods {
    use super::*;

    use super::test_helpers::{
        check_api_error, request_to_api, run_tower_in_background,
        run_tower_in_background_with_config, RequestBody,
    };
    use crate::extended_appointment::UUID;
    use crate::test_utils::{
        generate_dummy_appointment, get_random_user_id, ApiConfig, DURATION, SLOTS,
    };
    use teos_common::{cryptography, UserId};

    #[tokio::test]
    async fn test_register() {
        let server_addr = run_tower_in_background().await;
        let response =
            request_to_api::<common_msgs::RegisterRequest, common_msgs::RegisterResponse>(
                "/register",
                common_msgs::RegisterRequest {
                    user_id: get_random_user_id().to_vec(),
                },
                server_addr,
            )
            .await;
        assert!(matches!(response, Ok(common_msgs::RegisterResponse { .. })));
    }

    #[tokio::test]
    async fn test_register_max_slots() {
        let (server_addr, _) =
            run_tower_in_background_with_config(ApiConfig::new(u32::MAX, DURATION)).await;
        let user_id = get_random_user_id();

        // Register once, this should go trough and set slots to the limit
        request_to_api::<common_msgs::RegisterRequest, common_msgs::RegisterResponse>(
            "/register",
            common_msgs::RegisterRequest {
                user_id: user_id.to_vec(),
            },
            server_addr,
        )
        .await
        .unwrap();

        // Register again to get additional slots, this should fail
        assert_eq!(
            check_api_error(
                "/register",
                RequestBody::Json(serde_json::json!(common_msgs::RegisterRequest {
                    user_id: user_id.to_vec(),
                })),
                server_addr,
            )
            .await,
            (
                ApiError::new(
                    "Subscription maximum slots count reached".into(),
                    errors::REGISTRATION_RESOURCE_EXHAUSTED
                ),
                StatusCode::BAD_REQUEST
            )
        );
    }

    #[tokio::test]
    async fn test_register_service_unavailable() {
        let (server_addr, _) = run_tower_in_background_with_config(
            ApiConfig::new(SLOTS, DURATION).bitcoind_unreachable(),
        )
        .await;
        let user_id = get_random_user_id();

        // Register with bitcoind down
        assert_eq!(
            check_api_error(
                "/register",
                RequestBody::Json(serde_json::json!(common_msgs::RegisterRequest {
                    user_id: user_id.to_vec(),
                })),
                server_addr,
            )
            .await,
            (
                ApiError::new(
                    "Service currently unavailable".into(),
                    errors::SERVICE_UNAVAILABLE
                ),
                StatusCode::SERVICE_UNAVAILABLE
            )
        );
    }

    #[tokio::test]
    async fn test_add_appointment() {
        let server_addr = run_tower_in_background().await;

        // Register first
        let (user_sk, user_pk) = cryptography::get_random_keypair();
        request_to_api::<common_msgs::RegisterRequest, common_msgs::RegisterResponse>(
            "/register",
            common_msgs::RegisterRequest {
                user_id: user_pk.serialize().to_vec(),
            },
            server_addr,
        )
        .await
        .unwrap();

        // Then try to add an appointment
        let appointment = generate_dummy_appointment(None).inner;
        let signature = cryptography::sign(&appointment.to_vec(), &user_sk).unwrap();

        let response = request_to_api::<
            common_msgs::AddAppointmentRequest,
            common_msgs::AddAppointmentResponse,
        >(
            "/add_appointment",
            common_msgs::AddAppointmentRequest {
                appointment: Some(appointment.into()),
                signature,
            },
            server_addr,
        )
        .await;

        assert!(matches!(
            response,
            Ok(common_msgs::AddAppointmentResponse { .. })
        ));
    }

    #[tokio::test]
    async fn test_add_appointment_non_registered() {
        let server_addr = run_tower_in_background().await;
        let (user_sk, _) = cryptography::get_random_keypair();
        let appointment = generate_dummy_appointment(None).inner;
        let signature = cryptography::sign(&appointment.to_vec(), &user_sk).unwrap();

        assert_eq!(
            check_api_error(
                "/add_appointment",
                RequestBody::Json(serde_json::json!(common_msgs::AddAppointmentRequest {
                    appointment: Some(appointment.into()),
                    signature,
                })),
                server_addr,
            )
            .await,
            (
                ApiError::new(
                    "Invalid signature or user does not have enough slots available".into(),
                    errors::INVALID_SIGNATURE_OR_SUBSCRIPTION_ERROR
                ),
                StatusCode::UNAUTHORIZED
            )
        );
    }

    #[tokio::test]
    async fn test_add_appointment_already_triggered() {
        // Get the InternalAPI so we can mess with the inner state
        let (server_addr, internal_api) =
            run_tower_in_background_with_config(ApiConfig::new(u32::MAX, DURATION)).await;

        // Register
        let (user_sk, user_pk) = cryptography::get_random_keypair();
        request_to_api::<common_msgs::RegisterRequest, common_msgs::RegisterResponse>(
            "/register",
            common_msgs::RegisterRequest {
                user_id: user_pk.serialize().to_vec(),
            },
            server_addr,
        )
        .await
        .unwrap();

        // Add the appointment to the Responder so it counts as triggered
        let appointment = generate_dummy_appointment(None).inner;
        let signature = cryptography::sign(&appointment.to_vec(), &user_sk).unwrap();
        internal_api
            .get_watcher()
            .add_random_tracker_to_responder(UUID::new(appointment.locator, UserId(user_pk)));

        // Try to add it via the http API
        assert_eq!(
            check_api_error(
                "/add_appointment",
                RequestBody::Json(serde_json::json!(common_msgs::AddAppointmentRequest {
                    appointment: Some(appointment.into()),
                    signature,
                })),
                server_addr,
            )
            .await,
            (
                ApiError::new(
                    "The provided appointment has already been triggered".into(),
                    errors::APPOINTMENT_ALREADY_TRIGGERED
                ),
                StatusCode::BAD_REQUEST
            )
        );
    }

    #[tokio::test]
    async fn test_add_appointment_service_unavailable() {
        let (server_addr, _) = run_tower_in_background_with_config(
            ApiConfig::new(SLOTS, DURATION).bitcoind_unreachable(),
        )
        .await;
        let (user_sk, _) = cryptography::get_random_keypair();
        let appointment = generate_dummy_appointment(None).inner;
        let signature = cryptography::sign(&appointment.to_vec(), &user_sk).unwrap();

        assert_eq!(
            check_api_error(
                "/add_appointment",
                RequestBody::Json(serde_json::json!(common_msgs::AddAppointmentRequest {
                    appointment: Some(appointment.into()),
                    signature,
                })),
                server_addr,
            )
            .await,
            (
                ApiError::new(
                    "Service currently unavailable".into(),
                    errors::SERVICE_UNAVAILABLE
                ),
                StatusCode::SERVICE_UNAVAILABLE
            )
        );
    }

    #[tokio::test]
    async fn test_get_appointment() {
        let server_addr = run_tower_in_background().await;

        // Register first
        let (user_sk, user_pk) = cryptography::get_random_keypair();
        request_to_api::<common_msgs::RegisterRequest, common_msgs::RegisterResponse>(
            "/register",
            common_msgs::RegisterRequest {
                user_id: user_pk.serialize().to_vec(),
            },
            server_addr,
        )
        .await
        .unwrap();

        // Add an appointment
        let appointment = generate_dummy_appointment(None).inner;
        let signature = cryptography::sign(&appointment.to_vec(), &user_sk).unwrap();

        request_to_api::<common_msgs::AddAppointmentRequest, common_msgs::AddAppointmentResponse>(
            "/add_appointment",
            common_msgs::AddAppointmentRequest {
                appointment: Some(appointment.clone().into()),
                signature,
            },
            server_addr,
        )
        .await
        .unwrap();

        // Get it back
        let response = request_to_api::<
            common_msgs::GetAppointmentRequest,
            common_msgs::GetAppointmentResponse,
        >(
            "/get_appointment",
            common_msgs::GetAppointmentRequest {
                locator: appointment.locator.to_vec(),
                signature: cryptography::sign(
                    format!("get appointment {}", appointment.locator).as_bytes(),
                    &user_sk,
                )
                .unwrap(),
            },
            server_addr,
        )
        .await;

        assert!(matches!(
            response,
            Ok(common_msgs::GetAppointmentResponse { .. })
        ));
    }

    #[tokio::test]
    async fn test_get_appointment_non_registered() {
        let server_addr = run_tower_in_background().await;

        // User is not registered
        let (user_sk, _) = cryptography::get_random_keypair();
        // Appointment hasn't been added either, but the user check comes first
        let appointment = generate_dummy_appointment(None).inner;

        assert_eq!(
            check_api_error(
                "/get_appointment",
                RequestBody::Json(serde_json::json!(common_msgs::GetAppointmentRequest {
                    locator: appointment.locator.to_vec(),
                    signature: cryptography::sign(
                        format!("get appointment {}", appointment.locator).as_bytes(),
                        &user_sk,
                    )
                    .unwrap()
                })),
                server_addr,
            )
            .await,
            (
                ApiError::new(
                    "User cannot be authenticated".into(),
                    errors::INVALID_SIGNATURE_OR_SUBSCRIPTION_ERROR
                ),
                StatusCode::UNAUTHORIZED
            )
        );
    }

    #[tokio::test]
    async fn test_get_appointment_not_found() {
        let server_addr = run_tower_in_background().await;

        // Register first
        let (user_sk, user_pk) = cryptography::get_random_keypair();
        request_to_api::<common_msgs::RegisterRequest, common_msgs::RegisterResponse>(
            "/register",
            common_msgs::RegisterRequest {
                user_id: user_pk.serialize().to_vec(),
            },
            server_addr,
        )
        .await
        .unwrap();

        // Appointment hasn't been added
        let appointment = generate_dummy_appointment(None).inner;

        assert_eq!(
            check_api_error(
                "/get_appointment",
                RequestBody::Json(serde_json::json!(common_msgs::GetAppointmentRequest {
                    locator: appointment.locator.to_vec(),
                    signature: cryptography::sign(
                        format!("get appointment {}", appointment.locator).as_bytes(),
                        &user_sk,
                    )
                    .unwrap()
                })),
                server_addr,
            )
            .await,
            (
                ApiError::new(
                    "Appointment not found".into(),
                    errors::APPOINTMENT_NOT_FOUND
                ),
                StatusCode::NOT_FOUND
            )
        );
    }

    #[tokio::test]
    async fn test_get_appointment_service_unavailable() {
        let (server_addr, _) = run_tower_in_background_with_config(
            ApiConfig::new(SLOTS, DURATION).bitcoind_unreachable(),
        )
        .await;

        // Appointment hasn't been added
        let (user_sk, _) = cryptography::get_random_keypair();
        let appointment = generate_dummy_appointment(None).inner;

        assert_eq!(
            check_api_error(
                "/get_appointment",
                RequestBody::Json(serde_json::json!(common_msgs::GetAppointmentRequest {
                    locator: appointment.locator.to_vec(),
                    signature: cryptography::sign(
                        format!("get appointment {}", appointment.locator).as_bytes(),
                        &user_sk,
                    )
                    .unwrap()
                })),
                server_addr,
            )
            .await,
            (
                ApiError::new(
                    "Service currently unavailable".into(),
                    errors::SERVICE_UNAVAILABLE
                ),
                StatusCode::SERVICE_UNAVAILABLE
            )
        );
    }

    #[tokio::test]
    async fn test_get_subscription_info() {
        let server_addr = run_tower_in_background().await;

        // Register first
        let (user_sk, user_pk) = cryptography::get_random_keypair();
        request_to_api::<common_msgs::RegisterRequest, common_msgs::RegisterResponse>(
            "/register",
            common_msgs::RegisterRequest {
                user_id: user_pk.serialize().to_vec(),
            },
            server_addr,
        )
        .await
        .unwrap();

        // Get the subscription info
        let response = request_to_api::<
            common_msgs::GetSubscriptionInfoRequest,
            common_msgs::GetSubscriptionInfoResponse,
        >(
            "/get_subscription_info",
            common_msgs::GetSubscriptionInfoRequest {
                signature: cryptography::sign("get subscription info".as_bytes(), &user_sk)
                    .unwrap(),
            },
            server_addr,
        )
        .await;

        assert!(matches!(
            response,
            Ok(common_msgs::GetSubscriptionInfoResponse { .. })
        ));
    }

    #[tokio::test]
    async fn test_get_subscription_info_non_registered() {
        let server_addr = run_tower_in_background().await;

        // User is not registered
        let (user_sk, _) = cryptography::get_random_keypair();

        assert_eq!(
            check_api_error(
                "/get_subscription_info",
                RequestBody::Json(serde_json::json!(common_msgs::GetSubscriptionInfoRequest {
                    signature: cryptography::sign("get subscription info".as_bytes(), &user_sk)
                        .unwrap(),
                })),
                server_addr,
            )
            .await,
            (
                ApiError::new(
                    "User not found. Have you registered?".into(),
                    errors::INVALID_SIGNATURE_OR_SUBSCRIPTION_ERROR
                ),
                StatusCode::UNAUTHORIZED
            )
        );
    }

    #[tokio::test]
    async fn test_get_subscription_info_service_unavailable() {
        let (user_sk, _) = cryptography::get_random_keypair();
        let (server_addr, _) = run_tower_in_background_with_config(
            ApiConfig::new(SLOTS, DURATION).bitcoind_unreachable(),
        )
        .await;

        assert_eq!(
            check_api_error(
                "/get_subscription_info",
                RequestBody::Json(serde_json::json!(common_msgs::GetSubscriptionInfoRequest {
                    signature: cryptography::sign("get subscription info".as_bytes(), &user_sk)
                        .unwrap(),
                })),
                server_addr,
            )
            .await,
            (
                ApiError::new(
                    "Service currently unavailable".into(),
                    errors::SERVICE_UNAVAILABLE
                ),
                StatusCode::SERVICE_UNAVAILABLE
            )
        );
    }
}
