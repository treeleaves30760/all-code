//! A durable observer never gives a native vendor key to a loopback listener.
//! Its helper authenticates the host, then seals a route/instance-bound key.

use std::path::Path;

use anyhow::{Context, Result, bail};
use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use ring::{aead, hmac};
use serde::{Deserialize, Serialize};

use super::Hello;

const PREFIX: &str = "alc-observer-v1.";
const MAX_KEY_BYTES: usize = 4096;
const MAX_CREDENTIAL_BYTES: usize = 5600;

pub(super) struct ObserverKey(hmac::Key);

#[derive(Serialize, Deserialize)]
pub(super) struct AuthenticatedHello {
    #[serde(flatten)]
    pub hello: Hello,
    pub proof: String,
}

#[derive(Serialize, Deserialize)]
pub(super) struct ForwardControl {
    pub token: String,
    pub registration: super::ForwardRegistration,
}

impl ObserverKey {
    pub fn load(config_dir: &Path) -> Result<Self> {
        let encoded = super::files::load_or_create_observer_key(config_dir)?;
        let bytes = URL_SAFE_NO_PAD.decode(encoded).context(
            "invalid local observer secret; stop the bridge and recreate its observer secret",
        )?;
        if bytes.len() != 32 {
            bail!("invalid local observer secret length");
        }
        Ok(Self(hmac::Key::new(hmac::HMAC_SHA256, &bytes)))
    }

    pub fn proof(&self, nonce: &str, hello: &Hello) -> Result<String> {
        let message = serde_json::to_vec(&("alc-observer-hello-v1", nonce, hello))?;
        Ok(URL_SAFE_NO_PAD.encode(hmac::sign(&self.0, &message).as_ref()))
    }

    pub fn verify(&self, nonce: &str, reply: &AuthenticatedHello) -> Result<()> {
        let message = serde_json::to_vec(&("alc-observer-hello-v1", nonce, &reply.hello))?;
        let tag = URL_SAFE_NO_PAD
            .decode(&reply.proof)
            .map_err(|_| anyhow::anyhow!("could not authenticate the local observer"))?;
        hmac::verify(&self.0, &message, &tag)
            .map_err(|_| anyhow::anyhow!("could not authenticate the local observer"))
    }

    pub fn seal_forward_control(
        &self,
        hello: &Hello,
        token: &str,
        registration: super::ForwardRegistration,
    ) -> Result<String> {
        let context = format!("control-v1:{}:{}", hello.instance, hello.port);
        let value = serde_json::to_string(&ForwardControl {
            token: token.to_owned(),
            registration,
        })?;
        self.seal("alc-observer-forward-control-v1", &context, &value)
    }

    pub fn open_forward_control(
        &self,
        instance: &str,
        port: u16,
        body: &str,
    ) -> Result<ForwardControl> {
        let context = format!("control-v1:{instance}:{port}");
        let value = self.open("alc-observer-forward-control-v1", &context, body)?;
        serde_json::from_str(&value).context("invalid encrypted observer registration")
    }

    fn cipher(&self, instance: &str) -> Result<aead::LessSafeKey> {
        let context = serde_json::to_vec(&("alc-observer-sealing-key-v1", instance))?;
        let key = hmac::sign(&self.0, &context);
        let key = aead::UnboundKey::new(&aead::AES_256_GCM, key.as_ref())
            .map_err(|_| anyhow::anyhow!("could not prepare the local observer credential"))?;
        Ok(aead::LessSafeKey::new(key))
    }

    pub fn seal(&self, route: &str, instance: &str, value: &str) -> Result<String> {
        if value.is_empty()
            || value.len() > MAX_KEY_BYTES
            || !value.bytes().all(|byte| byte.is_ascii_graphic())
        {
            bail!("the native API key cannot be carried by the local observer");
        }
        let mut nonce = [0_u8; aead::NONCE_LEN];
        getrandom::fill(&mut nonce)
            .context("failed to read randomness for an observer credential")?;
        let aad = serde_json::to_vec(&("alc-observer-credential-v1", route, instance))?;
        let mut ciphertext = value.as_bytes().to_vec();
        self.cipher(instance)?
            .seal_in_place_append_tag(
                aead::Nonce::assume_unique_for_key(nonce),
                aead::Aad::from(aad),
                &mut ciphertext,
            )
            .map_err(|_| anyhow::anyhow!("could not seal the local observer credential"))?;
        let mut envelope = nonce.to_vec();
        envelope.extend_from_slice(&ciphertext);
        Ok(format!("{PREFIX}{}", URL_SAFE_NO_PAD.encode(envelope)))
    }

    pub fn open(&self, route: &str, instance: &str, credential: &str) -> Result<String> {
        let invalid = || anyhow::anyhow!("invalid local observer credential; rerun apiKeyHelper");
        if credential.len() > MAX_CREDENTIAL_BYTES {
            return Err(invalid());
        }
        let encoded = credential.strip_prefix(PREFIX).ok_or_else(invalid)?;
        let envelope = URL_SAFE_NO_PAD.decode(encoded).map_err(|_| invalid())?;
        if envelope.len() < aead::NONCE_LEN + aead::AES_256_GCM.tag_len() {
            return Err(invalid());
        }
        let (nonce, ciphertext) = envelope.split_at(aead::NONCE_LEN);
        let nonce: [u8; aead::NONCE_LEN] = nonce.try_into().map_err(|_| invalid())?;
        let mut plaintext = ciphertext.to_vec();
        let aad = serde_json::to_vec(&("alc-observer-credential-v1", route, instance))?;
        let value = self
            .cipher(instance)?
            .open_in_place(
                aead::Nonce::assume_unique_for_key(nonce),
                aead::Aad::from(aad),
                &mut plaintext,
            )
            .map_err(|_| invalid())?;
        let value = std::str::from_utf8(value).map_err(|_| invalid())?;
        if value.is_empty()
            || value.len() > MAX_KEY_BYTES
            || !value.bytes().all(|byte| byte.is_ascii_graphic())
        {
            return Err(invalid());
        }
        Ok(value.to_owned())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key() -> ObserverKey {
        ObserverKey(hmac::Key::new(hmac::HMAC_SHA256, &[7; 32]))
    }

    fn hello() -> Hello {
        Hello {
            instance: "instance-one".to_owned(),
            alc: env!("CARGO_PKG_VERSION").to_owned(),
            pid: 1,
            port: 24817,
            capabilities: vec![super::super::FORWARD_CAPABILITY.to_owned()],
        }
    }

    #[test]
    fn proof_binds_fresh_challenge_and_every_trusted_host_field() {
        let hello = hello();
        let mut reply = AuthenticatedHello {
            proof: key().proof("fresh", &hello).unwrap(),
            hello,
        };
        key().verify("fresh", &reply).unwrap();
        assert!(key().verify("different", &reply).is_err());
        reply.hello.port += 1;
        assert!(key().verify("fresh", &reply).is_err());
        reply.hello.port -= 1;
        reply.hello.capabilities.clear();
        assert!(key().verify("fresh", &reply).is_err());
    }

    #[test]
    fn surrogate_hides_native_key_and_binds_route_and_current_host_instance() {
        let sealed = key()
            .seal("route-one", "instance-one", "fake-vendor-key")
            .unwrap();
        assert!(!sealed.contains("fake-vendor-key"));
        assert_eq!(
            key().open("route-one", "instance-one", &sealed).unwrap(),
            "fake-vendor-key"
        );
        assert!(key().open("route-two", "instance-one", &sealed).is_err());
        assert!(key().open("route-one", "instance-two", &sealed).is_err());
        assert!(
            key()
                .open("route-one", "instance-one", "fake-vendor-key")
                .is_err()
        );
        let another = key()
            .seal("route-one", "instance-one", "fake-vendor-key")
            .unwrap();
        assert_ne!(sealed, another);
    }

    #[test]
    fn encrypted_registration_does_not_disclose_control_bearer_or_authorize_model_routes() {
        let hello = hello();
        let registration = super::super::ForwardRegistration {
            route: "route-one".to_owned(),
            key_digests: vec!["a".repeat(64)],
        };
        let sealed = key()
            .seal_forward_control(&hello, "fake-control-token", registration)
            .unwrap();
        assert!(!sealed.contains("fake-control-token"));
        let opened = key()
            .open_forward_control(&hello.instance, hello.port, &sealed)
            .unwrap();
        assert_eq!(opened.token, "fake-control-token");
        assert_eq!(opened.registration.route, "route-one");
        assert!(
            key()
                .open_forward_control("another-instance", hello.port, &sealed)
                .is_err()
        );
        assert!(
            key()
                .open_forward_control(&hello.instance, hello.port + 1, &sealed)
                .is_err()
        );
        assert!(key().open("route-one", &hello.instance, &sealed).is_err());
        let model = key()
            .seal("route-one", &hello.instance, "fake-native-key")
            .unwrap();
        assert!(
            key()
                .open_forward_control(&hello.instance, hello.port, &model)
                .is_err()
        );
    }

    #[test]
    fn corrupted_and_oversized_surrogates_are_opaque_refusals() {
        let sealed = key()
            .seal("route-one", "instance-one", "fake-vendor-key")
            .unwrap();
        let mut bytes = URL_SAFE_NO_PAD
            .decode(sealed.strip_prefix(PREFIX).unwrap())
            .unwrap();
        let last = bytes.len() - 1;
        bytes[last] ^= 1;
        for invalid in [
            format!("{PREFIX}{}", URL_SAFE_NO_PAD.encode(bytes)),
            format!("{PREFIX}{}", "A".repeat(MAX_CREDENTIAL_BYTES)),
            format!("{PREFIX}invalid!"),
        ] {
            let error = key()
                .open("route-one", "instance-one", &invalid)
                .unwrap_err()
                .to_string();
            assert!(!error.contains("fake-vendor-key") && !error.contains(&invalid));
        }
    }
}
