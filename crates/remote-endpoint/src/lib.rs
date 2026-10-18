//! Endpoint-only security. Never link device private keys into the relay service.
#[cfg(feature = "ffi")]
mod ffi;
pub mod pairing;
use snow::{Builder, HandshakeState, TransportState};
use subtle::ConstantTimeEq;

pub const PAIRING_PATTERN: &str = "Noise_XXpsk3_25519_ChaChaPoly_SHA256";
pub const RECONNECT_PATTERN: &str = "Noise_XX_25519_ChaChaPoly_SHA256";
pub const MAX_PLAINTEXT: usize = 65519;
const MAX_CIPHERTEXT: usize = 65535;

#[derive(Debug, PartialEq, Eq)]
pub enum Error {
    InvalidInput,
    Authentication,
    Closed,
    WrongPhase,
    Capacity,
}
pub type Result<T> = std::result::Result<T, Error>;

pub fn keypair() -> Result<snow::Keypair> {
    Builder::new(PAIRING_PATTERN.parse().map_err(|_| Error::InvalidInput)?)
        .generate_keypair()
        .map_err(|_| Error::Authentication)
}

enum State {
    Handshake(Box<HandshakeState>),
    Transport(Box<TransportState>),
}
/// Any failed crypto/state transition irreversibly closes this instance.
pub struct Session {
    state: Option<State>,
    expected_peer: Option<[u8; 32]>,
    peer: Option<[u8; 32]>,
    binding: Option<[u8; 32]>,
}
impl Session {
    pub fn new(
        initiator: bool,
        secret: &[u8; 32],
        expected_peer: Option<[u8; 32]>,
        psk: Option<&[u8; 32]>,
        prologue: &[u8],
    ) -> Result<Self> {
        if prologue.is_empty()
            || prologue.len() > 4096
            || (expected_peer.is_none() && (initiator || psk.is_none()))
        {
            return Err(Error::InvalidInput);
        }
        let pattern = if psk.is_some() {
            PAIRING_PATTERN
        } else {
            RECONNECT_PATTERN
        };
        let builder = Builder::new(pattern.parse().map_err(|_| Error::InvalidInput)?)
            .local_private_key(secret)
            .map_err(|_| Error::InvalidInput)?
            .prologue(prologue)
            .map_err(|_| Error::InvalidInput)?;
        let builder = match psk {
            Some(psk) => builder.psk(3, psk).map_err(|_| Error::InvalidInput)?,
            None => builder,
        };
        let noise = if initiator {
            builder.build_initiator()
        } else {
            builder.build_responder()
        }
        .map_err(|_| Error::Authentication)?;
        Ok(Self {
            state: Some(State::Handshake(Box::new(noise))),
            expected_peer,
            peer: None,
            binding: None,
        })
    }
    pub fn write_handshake(&mut self) -> Result<Vec<u8>> {
        let Some(State::Handshake(mut noise)) = self.state.take() else {
            return Err(Error::WrongPhase);
        };
        let mut output = vec![0; MAX_CIPHERTEXT];
        let len = noise
            .write_message(&[], &mut output)
            .map_err(|_| Error::Authentication)?;
        output.truncate(len);
        self.advance(noise)?;
        Ok(output)
    }
    pub fn read_handshake(&mut self, message: &[u8]) -> Result<()> {
        let Some(State::Handshake(mut noise)) = self.state.take() else {
            return Err(Error::WrongPhase);
        };
        if message.len() > MAX_CIPHERTEXT {
            return Err(Error::InvalidInput);
        }
        let mut payload = vec![0; MAX_CIPHERTEXT];
        let len = noise
            .read_message(message, &mut payload)
            .map_err(|_| Error::Authentication)?;
        if len != 0 {
            return Err(Error::InvalidInput);
        }
        self.advance(noise)
    }
    fn advance(&mut self, noise: Box<HandshakeState>) -> Result<()> {
        if let Some(remote) = noise.get_remote_static() {
            let peer: [u8; 32] = remote.try_into().map_err(|_| Error::Authentication)?;
            if let Some(expected) = &self.expected_peer
                && !bool::from(expected.ct_eq(&peer))
            {
                return Err(Error::Authentication);
            }
            self.peer = Some(peer);
        }
        if noise.is_handshake_finished() {
            if self.peer.is_none() {
                return Err(Error::Authentication);
            }
            self.binding = Some(
                noise
                    .get_handshake_hash()
                    .try_into()
                    .map_err(|_| Error::Authentication)?,
            );
            let transport = noise
                .into_transport_mode()
                .map_err(|_| Error::Authentication)?;
            self.state = Some(State::Transport(Box::new(transport)));
        } else {
            self.state = Some(State::Handshake(noise));
        }
        Ok(())
    }
    pub fn authenticated_peer(&self) -> Result<[u8; 32]> {
        if !matches!(self.state, Some(State::Transport(_))) {
            return Err(Error::WrongPhase);
        }
        self.peer.ok_or(Error::Authentication)
    }
    pub fn binding(&self) -> Result<[u8; 32]> {
        self.authenticated_peer()?;
        self.binding.ok_or(Error::Authentication)
    }
    pub fn seal(&mut self, plaintext: &[u8]) -> Result<Vec<u8>> {
        let Some(State::Transport(mut noise)) = self.state.take() else {
            return Err(Error::WrongPhase);
        };
        if plaintext.len() > MAX_PLAINTEXT {
            return Err(Error::InvalidInput);
        }
        let mut output = vec![0; plaintext.len() + 16];
        let len = noise
            .write_message(plaintext, &mut output)
            .map_err(|_| Error::Authentication)?;
        output.truncate(len);
        self.state = Some(State::Transport(noise));
        Ok(output)
    }
    pub fn open(&mut self, ciphertext: &[u8]) -> Result<Vec<u8>> {
        let Some(State::Transport(mut noise)) = self.state.take() else {
            return Err(Error::WrongPhase);
        };
        if ciphertext.len() < 16 || ciphertext.len() > MAX_CIPHERTEXT {
            return Err(Error::InvalidInput);
        }
        let mut output = vec![0; ciphertext.len()];
        let len = noise
            .read_message(ciphertext, &mut output)
            .map_err(|_| Error::Authentication)?;
        output.truncate(len);
        self.state = Some(State::Transport(noise));
        Ok(output)
    }
}
