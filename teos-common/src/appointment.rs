//! Logic related to appointments shared between users and the towers.

use hex;
use std::array::TryFromSliceError;
use std::{convert::TryInto, fmt};

use bitcoin::Txid;

pub const LOCATOR_LEN: usize = 16;

/// User identifier for appointments.
#[derive(Debug, Eq, PartialEq, Copy, Clone, Hash)]
pub struct Locator([u8; LOCATOR_LEN]);

impl Locator {
    /// Creates a new [Locator].
    pub fn new(txid: Txid) -> Self {
        Locator(txid[..LOCATOR_LEN].try_into().unwrap())
    }

    /// Encodes a locator into its byte representation.
    pub fn serialize(&self) -> Vec<u8> {
        self.0.to_vec()
    }

    /// Builds a locator from its byte representation.
    pub fn deserialize(data: &[u8]) -> Result<Self, TryFromSliceError> {
        data.try_into().map(Self)
    }
}

impl std::str::FromStr for Locator {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let raw_locator = hex::decode(s).map_err(|_| "Locator is not hex encoded")?;
        Locator::deserialize(&raw_locator)
            .map_err(|_| "Locator cannot be built from the given data".into())
    }
}

impl fmt::Display for Locator {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "{}", hex::encode(self.serialize()))
    }
}

/// Contains data regarding an appointment between a client and the tower.
///
/// An appointment is requested for every new channel update.
#[derive(Debug, Eq, PartialEq, Clone)]
pub struct Appointment {
    /// The user identifier for the appointment.
    pub locator: Locator,
    /// The encrypted blob of data to be handed to the tower.
    /// Should match an encrypted penalty transaction.
    pub encrypted_blob: Vec<u8>,
    /// The delay of the `to_self` output in the penalty transaction.
    /// Can be used by the tower to decide whether the job is worth accepting or not
    /// (useful for accountable towers). Currently not used.
    pub to_self_delay: u32,
}

/// Represents all the possible states of an appointment in the tower, or in a response to a client request.
pub enum AppointmentStatus {
    NotFound = 0,
    BeingWatched = 1,
    DisputeResponded = 2,
}

impl From<i32> for AppointmentStatus {
    fn from(x: i32) -> Self {
        match x {
            1 => AppointmentStatus::BeingWatched,
            2 => AppointmentStatus::DisputeResponded,
            _ => AppointmentStatus::NotFound,
        }
    }
}

impl std::str::FromStr for AppointmentStatus {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "being_watched" => Ok(AppointmentStatus::BeingWatched),
            "dispute_responded" => Ok(AppointmentStatus::DisputeResponded),
            "not_found" => Ok(AppointmentStatus::NotFound),
            _ => Err(format!("Unknown status: {}", s)),
        }
    }
}

impl fmt::Display for AppointmentStatus {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        let s = match self {
            AppointmentStatus::BeingWatched => "being_watched",
            AppointmentStatus::DisputeResponded => "dispute_responded",
            AppointmentStatus::NotFound => "not_found",
        };
        write!(f, "{}", s)
    }
}

impl Appointment {
    /// Creates a new [Appointment] instance.
    pub fn new(locator: Locator, encrypted_blob: Vec<u8>, to_self_delay: u32) -> Self {
        Appointment {
            locator,
            encrypted_blob,
            to_self_delay,
        }
    }

    /// Serializes an appointment to be signed.
    /// The serialization follows the same ordering as the fields in the appointment:
    ///
    /// `locator || encrypted_blob || to_self_delay`
    ///
    /// All values are big endian.
    pub fn serialize(&self) -> Vec<u8> {
        let mut result = self.locator.serialize();
        result.extend(&self.encrypted_blob);
        result.extend(self.to_self_delay.to_be_bytes().to_vec());
        result
    }
}
