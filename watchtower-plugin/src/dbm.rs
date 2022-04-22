use std::collections::{HashMap, HashSet};
use std::iter::FromIterator;
use std::path::PathBuf;
use std::str::FromStr;

use rusqlite::{params, Connection, Error as SqliteError};

use bitcoin::secp256k1::SecretKey;

use teos_common::appointment::{Appointment, Locator};
use teos_common::dbm::{DatabaseConnection, DatabaseManager, Error};
use teos_common::receipts::RegistrationReceipt;
use teos_common::UserId as TowerId;

use crate::TowerInfo;

const TABLES: [&str; 7] = [
    "CREATE TABLE IF NOT EXISTS towers (
    tower_id INT PRIMARY KEY,
    net_addr TEXT NOT NULL,
    available_slots INT NOT NULL,
    subscription_expiry INT NOT NULL
)",
    "CREATE TABLE IF NOT EXISTS appointments (
    locator INT PRIMARY KEY,
    encrypted_blob BLOB,
    to_self_delay INT,
    user_signature BLOB
)",
    "CREATE TABLE IF NOT EXISTS accepted_appointments (
    locator INT PRIMARY KEY,
    tower_id INT NOT NULL,
    start_block INT NOT NULL,
    tower_signature BLOB NOT NULL,
    FOREIGN KEY(tower_id)
        REFERENCES towers(tower_id)
        ON DELETE CASCADE
)",
    "CREATE TABLE IF NOT EXISTS pending_appointments (
    locator INT PRIMARY KEY,
    tower_id INT NOT NULL,
    FOREIGN KEY(locator)
        REFERENCES appointments(locator)
        ON DELETE CASCADE
    FOREIGN KEY(tower_id)
        REFERENCES towers(tower_id)
        ON DELETE CASCADE
)",
    "CREATE TABLE IF NOT EXISTS invalid_appointments (
    locator INT PRIMARY KEY,
    tower_id INT NOT NULL,
    FOREIGN KEY(locator)
        REFERENCES appointments(locator)
        ON DELETE CASCADE
    FOREIGN KEY(tower_id)
        REFERENCES towers(tower_id)
        ON DELETE CASCADE
)",
    "CREATE TABLE IF NOT EXISTS misbehaving_proofs (
    UUID INT PRIMARY KEY,
    tower_id INT NOT NULL,
    penalty_tx BLOB NOT NULL,
    height INT NOT NULL,
    confirmed BOOL NOT NULL,
    FOREIGN KEY(tower_id)
        REFERENCES towers(tower_id)
        ON DELETE CASCADE
)",
    "CREATE TABLE IF NOT EXISTS keys (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    key INT NOT NULL
)",
];

/// Component in charge of interacting with the underlying database.
///
/// Currently works for `SQLite`. `PostgreSQL` should also be added in the future.
#[derive(Debug)]
pub struct DBM {
    /// The underlying database connection.
    connection: Connection,
}

impl DatabaseConnection for DBM {
    fn get_connection(&self) -> &Connection {
        &self.connection
    }

    fn get_mut_connection(&mut self) -> &mut Connection {
        &mut self.connection
    }
}

impl DBM {
    /// Creates a new [DBM] instance.
    pub fn new(db_path: &PathBuf) -> Result<Self, SqliteError> {
        let connection = Connection::open(db_path)?;
        connection.execute("PRAGMA foreign_keys=1;", [])?;
        let mut dbm = Self { connection };
        dbm.create_tables(Vec::from_iter(TABLES))?;

        Ok(dbm)
    }

    /// Stores the client secret key into the database.
    ///
    /// When a new key is generated, old keys are not overwritten but are not retrievable from the API either.
    pub fn store_client_key(&self, sk: &SecretKey) -> Result<(), Error> {
        let query = "INSERT INTO keys (key) VALUES (?)";
        self.store_data(query, params![sk.to_string()])
    }

    /// Loads the last known client secret key from the database.
    ///
    /// Loads the key with higher id from the database. Old keys are not overwritten just in case a recovery is needed,
    /// but they are not accessible from the API either.
    pub fn load_client_key(&self) -> Result<SecretKey, Error> {
        let mut stmt = self
            .connection
            .prepare(
                "SELECT key FROM keys WHERE id = (SELECT seq FROM sqlite_sequence WHERE name=(?))",
            )
            .unwrap();

        stmt.query_row(["keys"], |row| {
            let sk: String = row.get(0).unwrap();
            Ok(SecretKey::from_str(&sk).unwrap())
        })
        .map_err(|_| Error::NotFound)
    }

    /// Stores a tower record into the database.
    pub fn store_tower_record(
        &self,
        tower_id: TowerId,
        net_addr: String,
        receipt: &RegistrationReceipt,
    ) -> Result<(), Error> {
        let query =
            "INSERT OR REPLACE INTO towers (tower_id, net_addr, available_slots, subscription_expiry) VALUES (?1, ?2, ?3, ?4)";
        self.store_data(
            query,
            params![
                tower_id.to_vec(),
                net_addr,
                receipt.available_slots(),
                receipt.subscription_expiry()
            ],
        )
    }

    /// Loads a tower record from the database.
    pub fn load_tower_record(&self, tower_id: TowerId) -> Result<TowerInfo, Error> {
        let mut stmt = self
            .connection
            .prepare("SELECT locator FROM accepted_appointments WHERE tower_id = ?")
            .unwrap();

        let mut rows = stmt.query([tower_id.to_vec()]).unwrap();
        let mut appointments = HashSet::new();
        while let Ok(Some(inner_row)) = rows.next() {
            let raw_locator: Vec<u8> = inner_row.get(0).unwrap();
            appointments.insert(Locator::from_slice(&raw_locator).unwrap());
        }

        let mut stmt = self
            .connection
            .prepare("SELECT locator FROM pending_appointments WHERE tower_id = ?")
            .unwrap();

        let mut rows = stmt.query([tower_id.to_vec()]).unwrap();
        let mut pending_appointments = HashSet::new();
        while let Ok(Some(inner_row)) = rows.next() {
            let raw_locator: Vec<u8> = inner_row.get(0).unwrap();
            pending_appointments.insert(Locator::from_slice(&raw_locator).unwrap());
        }

        let mut stmt = self
        .connection
        .prepare("SELECT net_addr, available_slots, subscription_expiry FROM towers WHERE tower_id = ?")
        .unwrap();

        stmt.query_row([tower_id.to_vec()], |row| {
            let net_addr: String = row.get(0).unwrap();
            let available_slots: u32 = row.get(1).unwrap();
            let subscription_expiry: u32 = row.get(2).unwrap();
            Ok(TowerInfo::with_appointments(
                net_addr,
                available_slots,
                subscription_expiry,
                appointments,
                pending_appointments,
            ))
        })
        .map_err(|_| Error::NotFound)
    }

    /// Loads all tower records from the database.
    pub fn load_towers(&self) -> HashMap<TowerId, TowerInfo> {
        let mut towers = HashMap::new();
        let mut stmt = self.connection.prepare("SELECT * FROM towers").unwrap();
        let mut rows = stmt.query([]).unwrap();

        while let Ok(Some(row)) = rows.next() {
            let raw_towerid: Vec<u8> = row.get(0).unwrap();
            let tower_id = TowerId::from_slice(&raw_towerid).unwrap();
            let net_addr: String = row.get(1).unwrap();
            let available_slots: u32 = row.get(2).unwrap();
            let subscription_expiry: u32 = row.get(3).unwrap();

            towers.insert(
                tower_id,
                TowerInfo::new(net_addr, available_slots, subscription_expiry),
            );
        }

        let mut stmt1 = self
            .connection
            .prepare("SELECT locator FROM accepted_appointments WHERE tower_id = ?")
            .unwrap();
        let mut stmt2 = self
            .connection
            .prepare("SELECT locator FROM pending_appointments WHERE tower_id = ?")
            .unwrap();

        for (tower_id, tower_info) in towers.iter_mut() {
            let mut rows = stmt1.query([tower_id.to_vec()]).unwrap();
            let mut appointments = HashSet::new();

            while let Ok(Some(inner_row)) = rows.next() {
                let raw_locator: Vec<u8> = inner_row.get(0).unwrap();
                appointments.insert(Locator::from_slice(&raw_locator).unwrap());
            }
            tower_info.appointments = appointments;

            let mut rows = stmt2.query([tower_id.to_vec()]).unwrap();
            let mut pending_appointments = HashSet::new();
            while let Ok(Some(inner_row)) = rows.next() {
                let raw_locator: Vec<u8> = inner_row.get(0).unwrap();
                pending_appointments.insert(Locator::from_slice(&raw_locator).unwrap());
            }

            tower_info.pending_appointments = pending_appointments;
        }

        towers
    }

    pub fn store_appointment(
        &mut self,
        locator: Locator,
        appointment: Appointment,
        user_signature: String,
    ) -> Result<(), SqliteError> {
        let tx = self.get_mut_connection().transaction().unwrap();
        tx.execute(
            "INSERT INTO appointments (locator, encrypted_blob, to_self_delay, user_signature) VALUES (?1, ?2, ?3, ?4)",
            params![
                locator.to_vec(),
                appointment.encrypted_blob,
                appointment.to_self_delay,
                user_signature
            ],
        )?;
        tx.commit()
    }

    pub fn store_accepted_appointment(
        &mut self,
        tower_id: TowerId,
        locator: Locator,
        start_block: u32,
        tower_signature: String,
        available_slots: u32,
    ) -> Result<(), SqliteError> {
        let tx = self.get_mut_connection().transaction().unwrap();
        tx.execute(
            "INSERT INTO accepted_appointments (locator, tower_id, start_block, tower_signature) VALUES (?1, ?2, ?3, ?4)",
            params![
                locator.to_vec(),
                tower_id.to_vec(),
                start_block,
                tower_signature
            ],
        )?;
        tx.execute(
            "UPDATE towers SET available_slots=?1 WHERE tower_id=?2",
            params![available_slots, tower_id.to_vec()],
        )?;
        tx.commit()
    }

    pub fn store_pending_appointment(
        &mut self,
        locator: Locator,
        tower_id: TowerId,
    ) -> Result<(), SqliteError> {
        let tx = self.get_mut_connection().transaction().unwrap();
        tx.execute(
            "INSERT INTO pending_appointments (locator, tower_id) VALUES (?1, ?2)",
            params![locator.to_vec(), tower_id.to_vec(),],
        )?;
        tx.commit()
    }
}
