//! The one remote command of a connection. Everything happens on a single SSH
//! session, so a password or a second factor is asked for only once.

/// Runs under the account's login shell, which can be fish or csh, so it is one
/// line inside `sh -c '...'` and contains no single quote and no `!`.
///
/// Exchange after the script starts:
/// 1. script prints `DOGCODES-BOOT <nonce>`, `PLATFORM <os> <machine>`, `HOME <path>`;
/// 2. desktop sends install root (`-` for the default), version directory, digest, size;
/// 3. script prints `PRESENT`, or `NEED` and then reads exactly `size` bytes;
/// 4. script replaces itself with the shim, which prints its ready line.
///
/// Any failure prints `DOGCODES-ERR <code>`. Output before the first line is
/// ignored: `.bashrc` and login banners may write to stdout.
pub(crate) fn remote_command(nonce: &str) -> String {
    debug_assert!(nonce.bytes().all(|byte| byte.is_ascii_hexdigit()));
    let script = [
        r#"fail() { printf "\nDOGCODES-ERR %s\n" "$1"; exit 78; }"#,
        &format!(r#"printf "\nDOGCODES-BOOT {nonce}\n""#),
        r#"printf "PLATFORM %s %s\n" "$(uname -s)" "$(uname -m)""#,
        r#"[ -n "${HOME:-}" ] && [ -d "$HOME" ] || fail home-missing"#,
        r#"printf "HOME %s\n" "$HOME""#,
        r"read -r root || fail input",
        r"read -r version || fail input",
        r"read -r digest || fail input",
        r"read -r size || fail input",
        r#"if [ "$root" = "-" ]; then root="$HOME/.dogcodes"; fi"#,
        r#"if [ -L "$root" ]; then fail unsafe-dir; fi"#,
        r#"mkdir -p "$root/shim/$version" 2>/dev/null || fail dir-unwritable"#,
        r#"[ -O "$root" ] || fail unsafe-dir"#,
        r#"chmod 700 "$root" 2>/dev/null || fail unsafe-dir"#,
        r#"bin="$root/shim/$version/dogcodes-shim""#,
        r#"expected="DOGCODES-SHIM-CHECK $digest""#,
        r#"if [ -x "$bin" ] && [ "$("$bin" --self-check 2>/dev/null)" = "$expected" ]; then printf "PRESENT\n"; else printf "NEED\n"; tmp="$bin.tmp.$$"; head -c "$size" > "$tmp" || { rm -f "$tmp"; fail upload-failed; }; [ "$(wc -c < "$tmp" | tr -d " ")" = "$size" ] || { rm -f "$tmp"; fail upload-failed; }; chmod 700 "$tmp" || { rm -f "$tmp"; fail upload-failed; }; out="$("$tmp" --self-check 2>/dev/null)"; code=$?; if [ "$code" = 126 ]; then rm -f "$tmp"; fail noexec; fi; [ "$out" = "$expected" ] || { rm -f "$tmp"; fail bad-binary; }; mv -f "$tmp" "$bin" || { rm -f "$tmp"; fail upload-failed; }; fi"#,
        &format!(r#"exec "$bin" serve --nonce {nonce}"#),
    ]
    .join("; ");
    debug_assert!(!script.contains('\''));
    format!("sh -c '{script}'")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_script_survives_being_passed_through_a_login_shell() {
        let command = remote_command("00ff");
        let inner = command
            .strip_prefix("sh -c '")
            .and_then(|tail| tail.strip_suffix('\''))
            .unwrap();
        assert!(!inner.contains('\''));
        assert!(!inner.contains('\n'));
        // csh expands history on `!` even inside single quotes.
        assert!(!inner.contains('!'));
    }

    #[cfg(unix)]
    #[test]
    fn the_script_is_valid_posix_shell() {
        let status = std::process::Command::new("/bin/sh")
            .args([
                "-n",
                "-c",
                remote_command("00ff")["sh -c '".len()..].trim_end_matches('\''),
            ])
            .status()
            .unwrap();
        assert!(status.success());
    }
}
