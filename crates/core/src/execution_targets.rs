//! Saved machines that can run agents, using the canonical event/ack transaction.
//! Only routes and settings live here: secrets stay in the system credential store.
use crate::{Core, CoreError};
use agent_protocol::{
    ActorRef, CommandEnvelope, CoreCommand, ExecutionTarget, ExecutionTargets,
    ExecutionTargetsSnapshot, SshAuth, SshConnection,
};
use std::collections::BTreeSet;

pub const STREAM_ID: &str = "settings.execution_targets";
pub const EVENT_TYPE: &str = "settings.execution_targets.updated";

const MAX_TARGETS: usize = 128;
const MAX_NAME_BYTES: usize = 256;
const MAX_PATH_BYTES: usize = 1024;

impl Core {
    pub async fn execution_targets(&self) -> Result<ExecutionTargetsSnapshot, CoreError> {
        Ok(self.repository.execution_targets().await?)
    }
}

pub(crate) fn validate(
    value: &ExecutionTargets,
    envelope: &CommandEnvelope<CoreCommand>,
) -> Result<(), CoreError> {
    if envelope.actor != ActorRef::LocalUser {
        return Err(CoreError::InvalidArgument(
            "execution targets require an authenticated local user".into(),
        ));
    }
    if envelope.expected_stream_version.is_none() {
        return Err(CoreError::InvalidArgument(
            "修改远程连接需要版本号，请重新读取设置".into(),
        ));
    }
    if value.targets.len() > MAX_TARGETS {
        return Err(CoreError::InvalidArgument("远程连接数量过多".into()));
    }
    let unique: BTreeSet<_> = value.targets.iter().map(|t| &t.target_id).collect();
    if unique.len() != value.targets.len() {
        return Err(CoreError::InvalidArgument("远程连接标识重复".into()));
    }
    value.targets.iter().try_for_each(validate_target)
}

fn validate_target(target: &ExecutionTarget) -> Result<(), CoreError> {
    if target.display_name.trim().is_empty()
        || target.display_name.len() > MAX_NAME_BYTES
        || target.display_name.chars().any(char::is_control)
    {
        return Err(CoreError::InvalidArgument("远程连接名称无效".into()));
    }
    validate_connection(&target.connection)
}

/// These values reach the `ssh` argument list. A leading `-` would be read as an
/// option (for example `-oProxyCommand=...`), so it is rejected rather than quoted.
pub fn validate_connection(connection: &SshConnection) -> Result<(), CoreError> {
    if !valid_ssh_word(&connection.destination) {
        return Err(CoreError::InvalidArgument("远程主机地址无效".into()));
    }
    if connection.port == Some(0) {
        return Err(CoreError::InvalidArgument("远程主机端口无效".into()));
    }
    if connection
        .user
        .as_deref()
        .is_some_and(|u| !valid_ssh_word(u))
    {
        return Err(CoreError::InvalidArgument("远程用户名无效".into()));
    }
    match (connection.auth, connection.identity_file.as_deref()) {
        (SshAuth::PrivateKey, None) => {
            return Err(CoreError::InvalidArgument("请选择私钥文件".into()));
        }
        (SshAuth::PrivateKey, Some(path)) if !valid_identity_path(path) => {
            return Err(CoreError::InvalidArgument("私钥路径无效".into()));
        }
        (SshAuth::System | SshAuth::Password, Some(_)) => {
            return Err(CoreError::InvalidArgument(
                "只有私钥认证可以指定私钥文件".into(),
            ));
        }
        _ => {}
    }
    if connection
        .install_directory
        .as_deref()
        .is_some_and(|path| !valid_install_directory(path))
    {
        return Err(CoreError::InvalidArgument(
            "远端安装目录必须是绝对路径".into(),
        ));
    }
    if let Some(agent_protocol::SshProxy::Custom { host, port, .. }) = &connection.proxy
        && (*port == 0 || !valid_proxy_host(host))
    {
        return Err(CoreError::InvalidArgument("backendText.invalidProxyAddress".into()));
    }
    Ok(())
}

/// Becomes part of a command `ssh` hands to a shell.
fn valid_proxy_host(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 255
        && value
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_' | ':'))
}

fn valid_ssh_word(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= MAX_NAME_BYTES
        && !value.starts_with('-')
        && !value
            .chars()
            .any(|c| c.is_whitespace() || c.is_control() || matches!(c, '\'' | '"' | '`' | '$'))
}

fn valid_path(value: &str) -> bool {
    !value.is_empty() && value.len() <= MAX_PATH_BYTES && !value.chars().any(char::is_control)
}

fn valid_identity_path(value: &str) -> bool {
    valid_path(value)
        && !value.starts_with('-')
        && (value.starts_with("~/") || std::path::Path::new(value).is_absolute())
}

/// The target is POSIX whichever system this client runs on, so `Path` cannot judge it.
fn valid_install_directory(value: &str) -> bool {
    valid_path(value) && value.starts_with('/') && !value.split('/').any(|part| part == "..")
}

#[cfg(test)]
mod tests {
    use super::*;
    use agent_domain::TargetId;

    fn connection(destination: &str) -> SshConnection {
        SshConnection {
            destination: destination.into(),
            port: None,
            user: None,
            auth: SshAuth::System,
            identity_file: None,
            install_directory: None,
            proxy: None,
        }
    }

    fn envelope(targets: ExecutionTargets) -> CommandEnvelope<CoreCommand> {
        let mut request = CommandEnvelope::local(CoreCommand::SetExecutionTargets(targets));
        request.expected_stream_version = Some(0);
        request
    }

    fn check(targets: &ExecutionTargets) -> Result<(), CoreError> {
        validate(targets, &envelope(targets.clone()))
    }

    #[test]
    fn option_shaped_routes_cannot_reach_the_ssh_argument_list() {
        for bad in [
            "-oProxyCommand=touch /tmp/x",
            "a b",
            "a\nb",
            "",
            "a'b",
            "$(x)",
        ] {
            assert!(validate_connection(&connection(bad)).is_err(), "{bad:?}");
        }
        let mut user = connection("host");
        user.user = Some("-l".into());
        assert!(validate_connection(&user).is_err());
        assert!(validate_connection(&connection("my-alias")).is_ok());
        assert!(validate_connection(&connection("192.168.1.10")).is_ok());
        assert!(validate_connection(&connection("[fe80::1]")).is_ok());
    }

    #[test]
    fn an_identity_file_only_accompanies_private_key_authentication() {
        let mut value = connection("host");
        value.auth = SshAuth::PrivateKey;
        assert!(validate_connection(&value).is_err());
        value.identity_file = Some("relative/key".into());
        assert!(validate_connection(&value).is_err());
        value.identity_file = Some("~/.ssh/id_ed25519".into());
        assert!(validate_connection(&value).is_ok());
        value.auth = SshAuth::Password;
        assert!(validate_connection(&value).is_err());
    }

    #[test]
    fn a_proxy_address_cannot_carry_a_command() {
        let mut value = connection("host");
        let custom = |host: &str, port| {
            Some(agent_protocol::SshProxy::Custom {
                kind: agent_protocol::SshProxyKind::Socks5,
                host: host.to_owned(),
                port,
            })
        };
        value.proxy = Some(agent_protocol::SshProxy::System);
        assert!(validate_connection(&value).is_ok());
        for host in ["127.0.0.1", "proxy.example.com", "::1"] {
            value.proxy = custom(host, 7890);
            assert!(validate_connection(&value).is_ok(), "{host}");
        }
        value.proxy = custom("127.0.0.1", 0);
        assert!(validate_connection(&value).is_err());
        for host in ["", "a b", "a;id", "$(id)", "`id`"] {
            value.proxy = custom(host, 7890);
            assert!(validate_connection(&value).is_err(), "{host}");
        }
    }

    #[test]
    fn install_directory_is_judged_as_a_posix_path() {
        let mut value = connection("host");
        value.install_directory = Some("/data/dogcodes".into());
        assert!(validate_connection(&value).is_ok());
        for bad in ["relative", "/data/../etc", "C:\\dogcodes", "/a\nb"] {
            value.install_directory = Some(bad.into());
            assert!(validate_connection(&value).is_err(), "{bad:?}");
        }
    }

    #[test]
    fn duplicate_identities_and_missing_versions_are_rejected() {
        let target = ExecutionTarget {
            target_id: TargetId::new(),
            display_name: "Build".into(),
            connection: connection("host"),
            machine_identity: None,
        };
        let one = ExecutionTargets {
            targets: vec![target.clone()],
        };
        assert!(check(&one).is_ok());
        let two = ExecutionTargets {
            targets: vec![target.clone(), target],
        };
        assert!(check(&two).is_err());
        let unversioned = CommandEnvelope::local(CoreCommand::SetExecutionTargets(one.clone()));
        assert!(validate(&one, &unversioned).is_err());
    }
}
