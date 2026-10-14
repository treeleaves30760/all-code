use std::ffi::OsString;

use anyhow::{Result, bail};

use crate::config::{Provider, ProviderKind};
use crate::launch::LaunchSpec;

/// Google's Antigravity CLI (the `agy` binary) holds its own Google login,
/// so alc has no key, base URL or config file to hand it: the launch is the
/// binary and whatever the user passed through.
pub(crate) fn build(
    spec: &mut LaunchSpec,
    provider: &Provider,
    passthrough: &[OsString],
) -> Result<()> {
    if provider.kind != ProviderKind::Antigravity {
        bail!("the Antigravity CLI runs only on an antigravity profile");
    }
    spec.args.extend_from_slice(passthrough);
    Ok(())
}
