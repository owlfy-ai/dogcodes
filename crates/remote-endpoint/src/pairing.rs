//! Host-owned grant journal. Contains public identities only, never private keys.
use crate::{Error as CryptoError, Session};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    fs::{self, OpenOptions},
    io::Write,
    path::{Path, PathBuf},
};
use zeroize::{Zeroize, Zeroizing};

#[derive(Debug)]
pub enum PairingError {
    Crypto(CryptoError),
    Io(std::io::Error),
    Invalid,
    Expired,
    Conflict,
    Revoked,
}
impl From<CryptoError> for PairingError {
    fn from(e: CryptoError) -> Self {
        Self::Crypto(e)
    }
}
impl From<std::io::Error> for PairingError {
    fn from(e: std::io::Error) -> Self {
        Self::Io(e)
    }
}
type Result<T> = std::result::Result<T, PairingError>;

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Grant {
    pub device_id: String,
    pub public_key: String,
    pub revision: u64,
    pub revoked: bool,
}
#[derive(Clone, Default, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Journal {
    version: u8,
    revision: u64,
    grants: BTreeMap<String, Grant>,
    receipts: BTreeMap<String, Receipt>,
}
#[derive(Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Receipt {
    device_id: String,
    revision: u64,
}
struct Invitation {
    epoch: [u8; 32],
    id: String,
    psk: Zeroizing<[u8; 32]>,
    context: Vec<u8>,
    expires_at: u64,
}
/// A single host owner serializes issue/complete/revoke; no other process writes this file.
pub struct PairingBook {
    path: PathBuf,
    journal: Journal,
    invitation: Option<Invitation>,
    unhealthy: bool,
}
pub struct PairingAttempt {
    epoch: [u8; 32],
    id: String,
    session: Session,
}
impl PairingAttempt {
    pub fn session(&mut self) -> &mut Session {
        &mut self.session
    }
}
/// Not serializable: only a successful, currently authorized handshake creates a lease.
pub struct DeviceLease {
    device_id: String,
    revision: u64,
}

impl PairingBook {
    pub fn grants(&self) -> Vec<Grant> {
        self.journal.grants.values().cloned().collect()
    }
    pub fn load(path: impl Into<PathBuf>) -> Result<Self> {
        let path = path.into();
        if path.with_extension("pending").exists() {
            return Err(PairingError::Invalid);
        }
        let journal = match fs::read(&path) {
            Ok(bytes) => {
                if bytes.len() > 1024 * 1024 {
                    return Err(PairingError::Invalid);
                }
                let journal: Journal =
                    serde_json::from_slice(&bytes).map_err(|_| PairingError::Invalid)?;
                if journal.version != 1
                    || journal.grants.len() > 128
                    || journal.receipts.len() > 1024
                {
                    return Err(PairingError::Invalid);
                }
                for (id, grant) in &journal.grants {
                    let key: [u8; 32] = hex::decode(&grant.public_key)
                        .map_err(|_| PairingError::Invalid)?
                        .try_into()
                        .map_err(|_| PairingError::Invalid)?;
                    if id != &device_id(&key)
                        || id != &grant.device_id
                        || grant.revision > journal.revision
                    {
                        return Err(PairingError::Invalid);
                    }
                }
                if journal.receipts.values().any(|r| {
                    !journal.grants.contains_key(&r.device_id) || r.revision > journal.revision
                }) {
                    return Err(PairingError::Invalid);
                }
                journal
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Journal {
                version: 1,
                ..Default::default()
            },
            Err(e) => return Err(e.into()),
        };
        Ok(Self {
            path,
            journal,
            invitation: None,
            unhealthy: false,
        })
    }
    pub fn issue(
        &mut self,
        id: &str,
        psk: [u8; 32],
        context: Vec<u8>,
        expires_at: u64,
        now: u64,
    ) -> Result<()> {
        self.healthy()?;
        if id.is_empty()
            || id.len() > 128
            || !id
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
            || self.journal.receipts.contains_key(id)
            || context.is_empty()
            || context.len() > 4096
            || expires_at <= now
            || expires_at - now > 300
        {
            return Err(PairingError::Invalid);
        }
        // Reissuing/closing the PC QR invalidates prior in-flight attempts.
        let mut random = crate::keypair()?;
        random.private.zeroize();
        let epoch = random
            .public
            .try_into()
            .map_err(|_| PairingError::Invalid)?;
        self.invitation = Some(Invitation {
            epoch,
            id: id.into(),
            psk: Zeroizing::new(psk),
            context,
            expires_at,
        });
        Ok(())
    }
    pub fn cancel_invitation(&mut self) {
        self.invitation = None;
    }
    pub fn begin(&self, id: &str, host_secret: &[u8; 32], now: u64) -> Result<PairingAttempt> {
        self.healthy()?;
        let invitation = self
            .invitation
            .as_ref()
            .filter(|i| i.id == id)
            .ok_or(PairingError::Expired)?;
        if invitation.expires_at <= now {
            return Err(PairingError::Expired);
        }
        Ok(PairingAttempt {
            epoch: invitation.epoch,
            id: id.into(),
            session: Session::new(
                false,
                host_secret,
                None,
                Some(&invitation.psk),
                &invitation.context,
            )?,
        })
    }
    pub fn complete(&mut self, attempt: &PairingAttempt, now: u64) -> Result<Grant> {
        self.healthy()?;
        let peer = attempt.session.authenticated_peer()?;
        let device_id = device_id(&peer);
        if let Some(existing) = self.journal.receipts.get(&attempt.id) {
            if existing.device_id != device_id {
                return Err(PairingError::Conflict);
            }
            let grant = self.active(&device_id)?;
            if grant.revision != existing.revision {
                return Err(PairingError::Revoked);
            }
            return Ok(grant.clone());
        }
        let invitation = self
            .invitation
            .as_ref()
            .filter(|i| i.id == attempt.id && i.epoch == attempt.epoch)
            .ok_or(PairingError::Expired)?;
        if invitation.expires_at <= now {
            return Err(PairingError::Expired);
        }
        if self.journal.grants.len() >= 128 || self.journal.receipts.len() >= 1024 {
            return Err(PairingError::Invalid);
        }
        // A fresh explicit QR may re-authorize a revoked phone, at a NEW revision.
        let mut next = self.journal.clone();
        next.revision = next.revision.checked_add(1).ok_or(PairingError::Invalid)?;
        let grant = Grant {
            device_id: device_id.clone(),
            public_key: hex::encode(peer),
            revision: next.revision,
            revoked: false,
        };
        next.grants.insert(device_id.clone(), grant.clone());
        next.receipts.insert(
            attempt.id.clone(),
            Receipt {
                device_id,
                revision: next.revision,
            },
        );
        self.persist(next)?;
        self.invitation = None;
        Ok(grant)
    }
    /// The session must have verified the phone's static key; caller-provided IDs never authorize.
    pub fn authorize(&self, session: &Session) -> Result<DeviceLease> {
        self.healthy()?;
        let id = device_id(&session.authenticated_peer()?);
        let grant = self.active(&id)?;
        Ok(DeviceLease {
            device_id: id,
            revision: grant.revision,
        })
    }
    /// Call before EVERY command/event delivery; revocation invalidates existing leases.
    pub fn check(&self, lease: &DeviceLease) -> Result<&Grant> {
        self.healthy()?;
        let grant = self.active(&lease.device_id)?;
        if grant.revision != lease.revision {
            return Err(PairingError::Revoked);
        }
        Ok(grant)
    }
    pub fn receipt(&self, invitation_id: &str, session: &Session) -> Result<Grant> {
        self.healthy()?;
        let id = device_id(&session.authenticated_peer()?);
        let receipt = self
            .journal
            .receipts
            .get(invitation_id)
            .filter(|r| r.device_id == id)
            .ok_or(PairingError::Conflict)?;
        let grant = self.active(&id)?;
        if grant.revision != receipt.revision {
            return Err(PairingError::Revoked);
        }
        Ok(grant.clone())
    }
    pub fn revoke(&mut self, device_id: &str) -> Result<()> {
        self.healthy()?;
        let Some(grant) = self.journal.grants.get(device_id) else {
            return Err(PairingError::Invalid);
        };
        if grant.revoked {
            return Ok(());
        }
        let mut next = self.journal.clone();
        next.revision = next.revision.checked_add(1).ok_or(PairingError::Invalid)?;
        if let Some(grant) = next.grants.get_mut(device_id) {
            grant.revoked = true;
            grant.revision = next.revision;
        }
        self.persist(next)
    }
    fn healthy(&self) -> Result<()> {
        if self.unhealthy {
            Err(PairingError::Invalid)
        } else {
            Ok(())
        }
    }
    fn active(&self, id: &str) -> Result<&Grant> {
        let grant = self.journal.grants.get(id).ok_or(PairingError::Revoked)?;
        if grant.revoked {
            return Err(PairingError::Revoked);
        }
        Ok(grant)
    }
    fn persist(&mut self, next: Journal) -> Result<()> {
        // Stop authorizing if durability is uncertain; don't retain stale grants after I/O failure.
        self.unhealthy = true;
        persist(&self.path, &next)?;
        self.journal = next;
        self.unhealthy = false;
        Ok(())
    }
}
fn device_id(public: &[u8; 32]) -> String {
    hex::encode(Sha256::digest(public))
}
fn persist(path: &Path, journal: &Journal) -> Result<()> {
    let bytes = serde_json::to_vec(journal).map_err(|_| PairingError::Invalid)?;
    let parent = path.parent().ok_or(PairingError::Invalid)?;
    fs::create_dir_all(parent)?;
    let temporary = path.with_extension("pending");
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options.open(&temporary)?;
    file.write_all(&bytes)?;
    file.sync_all()?;
    drop(file);
    fs::rename(&temporary, path)?;
    #[cfg(unix)]
    fs::File::open(parent)?.sync_all()?;
    Ok(())
}
