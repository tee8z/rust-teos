use std::collections::HashSet;
use std::fmt;

use serde::Serialize;

use teos_common::appointment::Locator;

pub mod convert;
pub mod dbm;
pub mod net;
pub mod wt_client;

#[derive(Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum TowerStatus {
    Reachable,
    TemporaryUnreachable,
    Unreachable,
    SubscriptionError,
}

impl fmt::Display for TowerStatus {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{}",
            match self {
                TowerStatus::Reachable => "reachable",
                TowerStatus::TemporaryUnreachable => "temporary unreachable",
                TowerStatus::Unreachable => "unreachable",
                TowerStatus::SubscriptionError => "subscription error",
            }
        )
    }
}

impl TowerStatus {
    pub fn is_reachable(&self) -> bool {
        *self == TowerStatus::Reachable
    }

    pub fn is_subscription_error(&self) -> bool {
        *self == TowerStatus::SubscriptionError
    }
}

#[derive(Clone, Serialize)]
pub struct TowerInfo {
    pub net_addr: String,
    pub available_slots: u32,
    pub subscription_expiry: u32,
    pub status: TowerStatus,
    #[serde(serialize_with = "teos_common::ser::serialize_locators")]
    pub appointments: HashSet<Locator>,
    #[serde(serialize_with = "teos_common::ser::serialize_locators")]
    pub pending_appointments: HashSet<Locator>,
}

impl TowerInfo {
    // TODO: Currently, when a tower is pulled from the DB after a reset the status is always set as reachable. It may be nice to
    // test it out first (This does not apply to register since new is created after receiving a response from the tower).
    pub fn new(net_addr: String, available_slots: u32, subscription_expiry: u32) -> Self {
        Self {
            net_addr,
            available_slots,
            subscription_expiry,
            status: TowerStatus::Reachable,
            appointments: HashSet::new(),
            pending_appointments: HashSet::new(),
        }
    }

    pub fn with_appointments(
        net_addr: String,
        available_slots: u32,
        subscription_expiry: u32,
        appointments: HashSet<Locator>,
        pending_appointments: HashSet<Locator>,
    ) -> Self {
        Self {
            net_addr,
            available_slots,
            subscription_expiry,
            status: TowerStatus::Reachable,
            appointments,
            pending_appointments,
        }
    }
}
