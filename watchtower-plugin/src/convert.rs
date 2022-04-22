use std::fmt;
use std::{convert::TryFrom, str::FromStr};

use bitcoin::Txid;
use hex::FromHex;
use serde::{Deserialize, Serialize};

use teos_common::appointment::Locator;
use teos_common::UserId as TowerId;

#[derive(Debug)]
pub enum RegisterError {
    InvalidId(String),
    InvalidHost(String),
    InvalidPort(String),
    InvalidFormat(String),
}

impl std::fmt::Display for RegisterError {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            RegisterError::InvalidId(x) => write!(f, "{}", x),
            RegisterError::InvalidHost(x) => write!(f, "{}", x),
            RegisterError::InvalidPort(x) => write!(f, "{}", x),
            RegisterError::InvalidFormat(x) => write!(f, "{}", x),
        }
    }
}

#[derive(Debug, Serialize)]
pub struct RegisterParams {
    pub tower_id: TowerId,
    pub host: Option<String>,
    pub port: Option<u16>,
}

impl RegisterParams {
    fn new(tower_id: &str, host: Option<&str>, port: Option<&str>) -> Result<Self, RegisterError> {
        let params = RegisterParams::from_id(tower_id)?.with_host(host)?;
        let port = if let Some(p) = port {
            p.parse()
                .map(|x| Some(x))
                .map_err(|_| RegisterError::InvalidPort(format!("Port is not a number: {}", p)))?
        } else {
            None
        };

        params.with_port(port)
    }

    fn from_id(tower_id: &str) -> Result<Self, RegisterError> {
        Ok(Self {
            tower_id: TowerId::from_str(tower_id)
                .map_err(|_| RegisterError::InvalidId("Invalid tower id".into()))?,
            host: None,
            port: None,
        })
    }

    fn with_host(self, host: Option<&str>) -> Result<Self, RegisterError> {
        let host = if let Some(h) = host {
            if h.is_empty() {
                return Err(RegisterError::InvalidHost("hostname is empty".into()));
            } else if h.contains(" ") {
                return Err(RegisterError::InvalidHost(
                    "hostname contains white spaces".into(),
                ));
            } else {
                Some(String::from(h))
            }
        } else {
            None
        };

        Ok(Self { host, ..self })
    }

    fn with_port(self, port: Option<u64>) -> Result<Self, RegisterError> {
        if let Some(p) = port {
            if p > u16::MAX as u64 {
                return Err(RegisterError::InvalidHost(format!(
                    "port must be a 16-byte integer. Received: {}",
                    p
                )));
            }
            Ok(Self {
                port: Some(p as u16),
                ..self
            })
        } else {
            Ok(Self { port: None, ..self })
        }
    }
}

impl TryFrom<serde_json::Value> for RegisterParams {
    type Error = RegisterError;

    fn try_from(value: serde_json::Value) -> Result<Self, Self::Error> {
        match value {
            serde_json::Value::String(s) => {
                let s = s.trim();
                let mut v = s.split("@");
                let tower_id = v.next().unwrap();

                match v.next() {
                    Some(x) => {
                        let mut v = x.split(":");
                        let host = v.next();
                        let port = v.next();

                        RegisterParams::new(tower_id, host, port)
                    }
                    None => RegisterParams::from_id(tower_id),
                }
            }
            serde_json::Value::Array(mut a) => {
            let param_count = a.len();
                match param_count {
                    1 => RegisterParams::try_from(a.pop().unwrap()),
                    2 | 3 => {
                        let tower_id = a.get(0).unwrap();
                        let host = a.get(1).unwrap();

                        if !tower_id.is_string() {
                            return Err(RegisterError::InvalidId(format!("tower_id must be a string. Received: {}", tower_id)));
                        }
                        if !host.is_string() {
                            return Err(RegisterError::InvalidHost(format!("host must be a string. Received: {}", host)));
                        }
                        let port = if param_count == 3 {
                            let p = a.get(2).unwrap();
                            if !p.is_u64() {
                                return Err(RegisterError::InvalidPort(format!("port must be a number. Received: {}", p))); 
                            }
                            p.as_u64()
                        } else{
                            None
                        };

                        RegisterParams::from_id(tower_id.as_str().unwrap())?.with_host(host.as_str())?.with_port(port)
                    }
                    _ => Err(RegisterError::InvalidFormat(format!("Unexpected request format. The request needs 1-3 parameters. Received: {}", param_count))),
                }
            },
            _ => Err(RegisterError::InvalidFormat(
                format!("Unexpected request format. Expected: 'tower_id[@host][:port]' or 'tower_id [host] [port]'. Received: '{}'", value),
            )),
        }
    }
}

#[derive(Debug)]
pub struct GetAppointmentParams {
    pub tower_id: TowerId,
    pub locator: Locator,
}

impl TryFrom<serde_json::Value> for GetAppointmentParams {
    type Error = String;

    fn try_from(value: serde_json::Value) -> Result<Self, Self::Error> {
        match value {
            serde_json::Value::Array(a) => {
                let param_count = a.len();
                if param_count != 2 {
                    Err(format!(
                        "Unexpected request format. The request needs 2 parameter. Received: {}",
                        param_count
                    ))
                } else {
                    let tower_id = if let Some(s) = a.get(0).unwrap().as_str() {
                        TowerId::from_str(s).map_err(|_| "Invalid tower id".into())
                    } else {
                        Err("tower_id must be a hex encoded string")
                    }?;

                    let locator = if let Some(s) = a.get(1).unwrap().as_str() {
                        Locator::from_hex(s)
                    } else {
                        Err("locator must be a hex encoded string".into())
                    }?;

                    Ok(Self { tower_id, locator })
                }
            }
            _ => Err(format!(
                "Unexpected request format. Expected: tower_id locator. Received: '{}'",
                value
            )),
        }
    }
}

// FIXME: Check if there is a way to deserialize (serde) a transaction from hex.
// Asked @stevenroose, looks like not atm but there will be.
#[derive(Debug, Serialize, Deserialize)]
pub struct CommitmentRevocation {
    pub channel_id: String,
    #[serde(rename(deserialize = "commitnum"))]
    pub commit_num: u32,
    pub commitment_txid: Txid,
    pub penalty_tx: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_build_register_params() {
        let ok = [
            "020dea894c967319407265764aba31bdef75d463f96800f34dd6df61380d82dfc0@host:80",
            "020dea894c967319407265764aba31bdef75d463f96800f34dd6df61380d82dfc0@host",
            "020dea894c967319407265764aba31bdef75d463f96800f34dd6df61380d82dfc0",
        ];
        let wrong_id = ["", "id@host:port", "@host:port", "@:port", "@:"];
        let wrong_host = [
            "020dea894c967319407265764aba31bdef75d463f96800f34dd6df61380d82dfc0@",
            "020dea894c967319407265764aba31bdef75d463f96800f34dd6df61380d82dfc0@ ",
            "020dea894c967319407265764aba31bdef75d463f96800f34dd6df61380d82dfc0@ host",
            "020dea894c967319407265764aba31bdef75d463f96800f34dd6df61380d82dfc0@:",
        ];
        let wrong_port = [
            "020dea894c967319407265764aba31bdef75d463f96800f34dd6df61380d82dfc0@host:",
            "020dea894c967319407265764aba31bdef75d463f96800f34dd6df61380d82dfc0@host:port",
        ];

        for s in ok {
            let v = serde_json::Value::Array(vec![serde_json::Value::String(s.to_string())]);
            let p = RegisterParams::try_from(v);
            assert!(matches!(p, Ok(..)));
        }

        for s in wrong_id {
            let v = serde_json::Value::Array(vec![serde_json::Value::String(s.to_string())]);
            let p = RegisterParams::try_from(v);
            assert!(matches!(p, Err(RegisterError::InvalidId(..))));
        }

        for s in wrong_host {
            let v = serde_json::Value::Array(vec![serde_json::Value::String(s.to_string())]);
            let p = RegisterParams::try_from(v);
            assert!(matches!(p, Err(RegisterError::InvalidHost(..))));
        }

        for s in wrong_port {
            let v = serde_json::Value::Array(vec![serde_json::Value::String(s.to_string())]);
            let p = RegisterParams::try_from(v);
            assert!(matches!(p, Err(RegisterError::InvalidPort(..))));
        }
    }
}
