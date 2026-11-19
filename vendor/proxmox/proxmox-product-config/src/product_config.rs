use std::path::{Path, PathBuf};

use nix::sys::stat::Mode;
use nix::unistd::User;

use proxmox_sys::fs::CreateOptions;

/// Parameter type for [`ProductConfig::new`].
///
/// This type is used to avoid having long lists of parameters with the same type, while also
/// keeping the members of the final type, [`ProductConfig`] private.
#[derive(Clone, Debug)]
pub struct ProductConfigParams {
    /// The user the unprivileged API daemon runs as.
    pub api_user: User,

    /// The user the privileged API daemon runs as.
    pub priv_user: User,

    /// Directory for configuration files.
    ///
    /// This is typically in `/etc/<product>`.
    pub config_dir: PathBuf,

    /// Directory for persistent state.
    ///
    /// This is typically in `/var/lib/<product>`.
    pub state_dir: PathBuf,

    /// Directory for runtime data, not persisted across reboots.
    ///
    /// This is typically in `/run/<product>`.
    pub run_dir: PathBuf,

    /// Directory for cached data, which can be regenerated if lost.
    ///
    /// This is typically in `/var/cache/<product>`.
    pub cache_dir: PathBuf,
}

/// Product-specific configuration, such as the users the product runs as and the directories it
/// stores its files in.
///
/// # Examples
///
/// ```no_run
/// use nix::unistd::User;
///
/// use proxmox_product_config::{ProductConfig, ProductConfigParams};
///
/// # fn main() -> Result<(), anyhow::Error> {
/// let config = ProductConfig::new(ProductConfigParams {
///     api_user: User::from_name("www-data")?.expect("www-data user exists"),
///     priv_user: User::from_name("root")?.expect("root user exists"),
///     config_dir: "/etc/proxmox-product".into(),
///     state_dir: "/var/lib/proxmox-product".into(),
///     run_dir: "/run/proxmox-product".into(),
///     cache_dir: "/var/cache/proxmox-product".into(),
/// });
///
/// // Write a config file as `www-data:www-data` with mode 0640.
/// let config_file = config.config_dir().join("product.conf");
/// let options = config.default_file_create_options();
/// proxmox_sys::fs::replace_file(&config_file, b"key: value\n", options, true)?;
///
/// // Files holding secrets are only accessible to the privileged daemon: `root:root`, mode 0600.
/// let key_file = config.config_dir().join("auth.key");
/// let options = config.secret_file_create_options();
/// proxmox_sys::fs::replace_file(&key_file, b"secret\n", options, true)?;
/// # Ok(())
/// # }
/// ```
#[derive(Clone, Debug)]
pub struct ProductConfig(ProductConfigParams);

impl ProductConfig {
    /// Create a new [`ProductConfig`] from the provided
    /// [`ProductConfigParams`].
    pub fn new(params: ProductConfigParams) -> Self {
        Self(params)
    }

    /// The user the unprivileged API daemon runs as.
    pub fn api_user(&self) -> &User {
        &self.0.api_user
    }

    /// The user the privileged API daemon runs as.
    pub fn priv_user(&self) -> &User {
        &self.0.priv_user
    }

    /// Directory for configuration files.
    ///
    /// This is typically in `/etc/<product>`.
    pub fn config_dir(&self) -> &Path {
        &self.0.config_dir
    }

    /// Directory for persistent state.
    ///
    /// This is typically in `/var/lib/<product>`.
    pub fn state_dir(&self) -> &Path {
        &self.0.state_dir
    }

    /// Directory for runtime data, not persisted across reboots.
    ///
    /// This is typically in `/run/<product>`.
    pub fn run_dir(&self) -> &Path {
        &self.0.run_dir
    }

    /// Directory for cached data, which can be regenerated if lost.
    ///
    /// This is typically in `/var/cache/<product>`.
    pub fn cache_dir(&self) -> &Path {
        &self.0.cache_dir
    }

    /// Default options for creating files: mode 0640, owned by the API user.
    pub fn default_file_create_options(&self) -> CreateOptions {
        let api_user = self.api_user();
        let mode = Mode::from_bits_truncate(0o0640);

        CreateOptions::new()
            .perm(mode)
            .owner(api_user.uid)
            .group(api_user.gid)
    }

    /// Default options for creating directories: mode 0750, owned by the API user.
    pub fn default_dir_create_options(&self) -> CreateOptions {
        let api_user = self.api_user();
        let mode = Mode::from_bits_truncate(0o0750);

        CreateOptions::new()
            .perm(mode)
            .owner(api_user.uid)
            .group(api_user.gid)
    }

    /// Return [`CreateOptions`] for files owned by `priv_user.uid:api_user.gid`
    /// with permission `0640`.
    ///
    /// Only `priv_user` can write those files, but group `api_user.gid` can read them.
    pub fn privileged_file_create_options(&self) -> CreateOptions {
        let api_user = self.api_user();
        let priv_user = self.priv_user();
        let mode = Mode::from_bits_truncate(0o0640);

        CreateOptions::new()
            .perm(mode)
            .owner(priv_user.uid)
            .group(api_user.gid)
    }

    /// Return [`CreateOptions`] for files owned by `priv_user.uid:api_user.gid`
    /// with permission `0750`.
    ///
    /// Only `priv_user` can write those files, but group `api_user.gid` can read them.
    pub fn privileged_dir_create_options(&self) -> CreateOptions {
        let api_user = self.api_user();
        let priv_user = self.priv_user();
        let mode = Mode::from_bits_truncate(0o0750);

        CreateOptions::new()
            .perm(mode)
            .owner(priv_user.uid)
            .group(api_user.gid)
    }

    /// Return [`CreateOptions`] for files owned by `priv_user.uid:priv_user.gid`
    /// with permission `0600`.
    ///
    /// Only `priv_user` can read and write those files.
    pub fn secret_file_create_options(&self) -> CreateOptions {
        let priv_user = self.priv_user();
        let mode = Mode::from_bits_truncate(0o0600);

        CreateOptions::new()
            .perm(mode)
            .owner(priv_user.uid)
            .group(priv_user.gid)
    }

    /// Return [`CreateOptions`] for directories owned by `priv_user.uid:priv_user.gid`
    /// with permission `0700`.
    ///
    /// Only `priv_user` can read and write those files.
    pub fn secret_dir_create_options(&self) -> CreateOptions {
        let priv_user = self.priv_user();
        let mode = Mode::from_bits_truncate(0o0700);

        CreateOptions::new()
            .perm(mode)
            .owner(priv_user.uid)
            .group(priv_user.gid)
    }

    /// Return [`CreateOptions`] for lock files, owner `api_user.uid/api_user.gid` and mode `0660`.
    pub fn lockfile_create_options(&self) -> CreateOptions {
        let api_user = self.api_user();
        CreateOptions::new()
            .perm(Mode::from_bits_truncate(0o660))
            .owner(api_user.uid)
            .group(api_user.gid)
    }
}
