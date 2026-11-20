use std::fs;
use std::path::Path;
use std::process::Command;

const LATEST_RELEASE_API_URL: &str =
    "https://api.github.com/repos/alivergg/squad-air/releases/latest";
const ALLOWED_DOWNLOAD_PREFIX: &str = "https://github.com/alivergg/squad-air/releases/download/";
const ALLOWED_HTML_PREFIX: &str = "https://github.com/alivergg/squad-air/";
const MAX_METADATA_BYTES: &str = "1048576";
const MAX_SUMS_BYTES: &str = "65536";
const MAX_INSTALLER_BYTES: u64 = 134_217_728;
const MAX_INSTALLER_BYTES_STR: &str = "134217728";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReleaseInfo {
    pub tag_name: String,
    pub version_num: String,
    pub asset_name: String,
    pub asset_url: String,
    pub sha256sums_url: String,
    pub html_url: String,
    pub notes: String,
}

pub fn parse_version_tag(raw: &str) -> Option<(u32, u32, u32)> {
    let clean = raw.trim().trim_start_matches(['v', 'V']);
    if clean.is_empty() {
        return None;
    }
    let parts: Vec<&str> = clean.split('.').collect();
    if !(2..=3).contains(&parts.len()) {
        return None;
    }
    let major = parts[0].parse::<u32>().ok()?;
    let minor = parts[1].parse::<u32>().ok()?;
    let patch = if parts.len() == 3 {
        parts[2].parse::<u32>().ok()?
    } else {
        0
    };
    Some((major, minor, patch))
}

pub fn is_newer_version(current: &str, candidate: &str) -> bool {
    match (parse_version_tag(current), parse_version_tag(candidate)) {
        (Some(cur), Some(cand)) => cand > cur,
        _ => false,
    }
}

pub fn sha256_hex(data: &[u8]) -> String {
    const K: [u32; 64] = [
        0x428a2f98, 0x71374491, 0xb5c0fbcf, 0xe9b5dba5, 0x3956c25b, 0x59f111f1, 0x923f82a4,
        0xab1c5ed5, 0xd807aa98, 0x12835b01, 0x243185be, 0x550c7dc3, 0x72be5d74, 0x80deb1fe,
        0x9bdc06a7, 0xc19bf174, 0xe49b69c1, 0xefbe4786, 0x0fc19dc6, 0x240ca1cc, 0x2de92c6f,
        0x4a7484aa, 0x5cb0a9dc, 0x76f988da, 0x983e5152, 0xa831c66d, 0xb00327c8, 0xbf597fc7,
        0xc6e00bf3, 0xd5a79147, 0x06ca6351, 0x14292967, 0x27b70a85, 0x2e1b2138, 0x4d2c6dfc,
        0x53380d13, 0x650a7354, 0x766a0abb, 0x81c2c92e, 0x92722c85, 0xa2bfe8a1, 0xa81a664b,
        0xc24b8b70, 0xc76c51a3, 0xd192e819, 0xd6990624, 0xf40e3585, 0x106aa070, 0x19a4c116,
        0x1e376c08, 0x2748774c, 0x34b0bcb5, 0x391c0cb3, 0x4ed8aa4a, 0x5b9cca4f, 0x682e6ff3,
        0x748f82ee, 0x78a5636f, 0x84c87814, 0x8cc70208, 0x90befffa, 0xa4506ceb, 0xbef9a3f7,
        0xc67178f2,
    ];

    let mut h: [u32; 8] = [
        0x6a09e667, 0xbb67ae85, 0x3c6ef372, 0xa54ff53a, 0x510e527f, 0x9b05688c, 0x1f83d9ab,
        0x5be0cd19,
    ];

    let bit_len = (data.len() as u64).wrapping_mul(8);
    let mut padded = Vec::with_capacity(data.len() + 72);
    padded.extend_from_slice(data);
    padded.push(0x80);
    while (padded.len() % 64) != 56 {
        padded.push(0);
    }
    padded.extend_from_slice(&bit_len.to_be_bytes());

    for block in padded.as_chunks::<64>().0 {
        let mut w = [0u32; 64];
        for (i, word) in block.as_chunks::<4>().0.iter().enumerate() {
            w[i] = u32::from_be_bytes(*word);
        }
        for i in 16..64 {
            let s0 = w[i - 15].rotate_right(7) ^ w[i - 15].rotate_right(18) ^ (w[i - 15] >> 3);
            let s1 = w[i - 2].rotate_right(17) ^ w[i - 2].rotate_right(19) ^ (w[i - 2] >> 10);
            w[i] = w[i - 16]
                .wrapping_add(s0)
                .wrapping_add(w[i - 7])
                .wrapping_add(s1);
        }

        let [mut a, mut b, mut c, mut d, mut e, mut f, mut g, mut hh] = h;
        for i in 0..64 {
            let s1 = e.rotate_right(6) ^ e.rotate_right(11) ^ e.rotate_right(25);
            let ch = (e & f) ^ ((!e) & g);
            let temp1 = hh
                .wrapping_add(s1)
                .wrapping_add(ch)
                .wrapping_add(K[i])
                .wrapping_add(w[i]);
            let s0 = a.rotate_right(2) ^ a.rotate_right(13) ^ a.rotate_right(22);
            let maj = (a & b) ^ (a & c) ^ (b & c);
            let temp2 = s0.wrapping_add(maj);

            hh = g;
            g = f;
            f = e;
            e = d.wrapping_add(temp1);
            d = c;
            c = b;
            b = a;
            a = temp1.wrapping_add(temp2);
        }

        h[0] = h[0].wrapping_add(a);
        h[1] = h[1].wrapping_add(b);
        h[2] = h[2].wrapping_add(c);
        h[3] = h[3].wrapping_add(d);
        h[4] = h[4].wrapping_add(e);
        h[5] = h[5].wrapping_add(f);
        h[6] = h[6].wrapping_add(g);
        h[7] = h[7].wrapping_add(hh);
    }

    let mut out = String::with_capacity(64);
    for word in h {
        use std::fmt::Write;
        let _ = write!(out, "{word:08x}");
    }
    out
}

pub fn lookup_sha256_for_asset(sums_text: &str, asset_name: &str) -> Option<String> {
    let target = asset_name.trim();
    if target.is_empty() {
        return None;
    }

    for line in sums_text.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        let mut parts = trimmed.splitn(2, char::is_whitespace);
        let Some(hex_raw) = parts.next() else {
            continue;
        };
        let Some(rest) = parts.next() else {
            continue;
        };
        let filename = rest.trim().trim_start_matches('*').trim_start_matches("./");
        if filename != target {
            continue;
        }
        let hex = hex_raw.trim();
        if hex.len() == 64 && hex.bytes().all(|b| b.is_ascii_hexdigit()) {
            return Some(hex.to_ascii_lowercase());
        }
    }

    None
}

pub fn expected_platform_asset_names(version_num: &str) -> Vec<String> {
    let _ = version_num;
    #[cfg(target_os = "macos")]
    {
        vec!["SquadAir-Installer-macos-arm64.pkg".to_string()]
    }

    #[cfg(target_os = "windows")]
    {
        vec!["SquadAir-Setup-windows-x86_64.exe".to_string()]
    }

    #[cfg(all(not(target_os = "macos"), not(target_os = "windows")))]
    {
        let mut candidates = Vec::new();
        if Path::new("/etc/arch-release").exists() {
            candidates.push(format!("squad-air-{version_num}-1-x86_64.pkg.tar.zst"));
        } else if Path::new("/etc/fedora-release").exists()
            || Path::new("/etc/redhat-release").exists()
        {
            candidates.push(format!("squad-air-{version_num}-1.x86_64.rpm"));
        } else if Path::new("/etc/debian_version").exists() {
            candidates.push(format!("squad-air_{version_num}_amd64.deb"));
        }
        candidates.push("squad-client-linux-x86_64.tar.gz".to_string());
        candidates
    }
}

pub fn parse_github_release_json(json: &str) -> Option<ReleaseInfo> {
    parse_github_release_json_with(json, expected_platform_asset_names)
}

fn parse_github_release_json_with<F>(json: &str, candidates_fn: F) -> Option<ReleaseInfo>
where
    F: FnOnce(&str) -> Vec<String>,
{
    let mut parser = JsonCursor::new(json);
    parser.skip_ws();
    if parser.peek_byte()? != b'{' {
        return None;
    }
    parser.pos += 1;

    let mut tag_name: Option<String> = None;
    let mut html_url: Option<String> = None;
    let mut body_raw: Option<String> = None;
    let mut is_draft = false;
    let mut is_prerelease = false;
    let mut download_urls: Vec<String> = Vec::new();

    loop {
        parser.skip_ws();
        match parser.peek_byte()? {
            b'}' => break,
            b',' => {
                parser.pos += 1;
                continue;
            }
            _ => {}
        }

        let key = parser.parse_string()?;
        parser.skip_ws();
        if parser.peek_byte()? != b':' {
            return None;
        }
        parser.pos += 1;
        parser.skip_ws();

        match key.as_str() {
            "tag_name" => tag_name = parser.parse_string_or_null(),
            "html_url" => html_url = parser.parse_string_or_null(),
            "body" => body_raw = parser.parse_string_or_null(),
            "draft" => is_draft = parser.parse_bool().unwrap_or(false),
            "prerelease" => is_prerelease = parser.parse_bool().unwrap_or(false),
            "assets" => parse_assets_array(&mut parser, &mut download_urls)?,
            _ => parser.skip_value()?,
        }
    }

    if is_draft || is_prerelease {
        return None;
    }

    let tag_name = tag_name?;
    let parsed_ver = parse_version_tag(&tag_name)?;
    let version_num = tag_name.trim().trim_start_matches(['v', 'V']).to_string();
    let _ = parsed_ver;

    let html_url = html_url.unwrap_or_else(|| {
        format!("https://github.com/alivergg/squad-air/releases/tag/{tag_name}")
    });
    if !html_url.starts_with(ALLOWED_HTML_PREFIX) {
        return None;
    }

    let valid_urls: Vec<String> = download_urls
        .into_iter()
        .filter(|u| u.starts_with(ALLOWED_DOWNLOAD_PREFIX))
        .collect();

    let sha256sums_url = valid_urls
        .iter()
        .find(|u| u.rsplit('/').next() == Some("SHA256SUMS.txt"))?
        .clone();

    let candidates = candidates_fn(&version_num);
    let mut chosen: Option<(String, String)> = None;
    for candidate in &candidates {
        if let Some(url) = valid_urls
            .iter()
            .find(|u| u.rsplit('/').next() == Some(candidate.as_str()))
        {
            chosen = Some((candidate.clone(), url.clone()));
            break;
        }
    }
    let (asset_name, asset_url) = chosen?;
    validate_asset_name(&asset_name)?;

    let raw_notes = body_raw.unwrap_or_default();
    let clean_notes = raw_notes
        .lines()
        .map(|l| squad_proto::sanitize_display_str(l, 200))
        .filter(|l| !l.is_empty())
        .take(12)
        .collect::<Vec<_>>()
        .join("\n");
    let notes = if clean_notes.is_empty() {
        "New SquadAir release available on GitHub.".to_string()
    } else {
        clean_notes
    };

    Some(ReleaseInfo {
        tag_name,
        version_num,
        asset_name,
        asset_url,
        sha256sums_url,
        html_url,
        notes,
    })
}

fn validate_asset_name(name: &str) -> Option<()> {
    if name.is_empty()
        || name.len() > 128
        || name.contains('/')
        || name.contains('\\')
        || name.contains("..")
        || name.chars().any(char::is_control)
    {
        return None;
    }
    Some(())
}

fn parse_assets_array(parser: &mut JsonCursor<'_>, urls: &mut Vec<String>) -> Option<()> {
    if parser.peek_byte()? != b'[' {
        return parser.skip_value();
    }
    parser.pos += 1;

    loop {
        parser.skip_ws();
        match parser.peek_byte()? {
            b']' => {
                parser.pos += 1;
                return Some(());
            }
            b',' => {
                parser.pos += 1;
                continue;
            }
            b'{' => {
                parser.pos += 1;
                loop {
                    parser.skip_ws();
                    match parser.peek_byte()? {
                        b'}' => {
                            parser.pos += 1;
                            break;
                        }
                        b',' => {
                            parser.pos += 1;
                            continue;
                        }
                        _ => {}
                    }
                    let key = parser.parse_string()?;
                    parser.skip_ws();
                    if parser.peek_byte()? != b':' {
                        return None;
                    }
                    parser.pos += 1;
                    parser.skip_ws();
                    if key == "browser_download_url" {
                        if let Some(url) = parser.parse_string_or_null() {
                            urls.push(url);
                        }
                    } else {
                        parser.skip_value()?;
                    }
                }
            }
            _ => parser.skip_value()?,
        }
    }
}

struct JsonCursor<'a> {
    bytes: &'a [u8],
    pos: usize,
}

impl<'a> JsonCursor<'a> {
    fn new(input: &'a str) -> Self {
        Self {
            bytes: input.as_bytes(),
            pos: 0,
        }
    }

    fn peek_byte(&self) -> Option<u8> {
        self.bytes.get(self.pos).copied()
    }

    fn skip_ws(&mut self) {
        while let Some(b) = self.peek_byte() {
            if matches!(b, b' ' | b'\n' | b'\r' | b'\t') {
                self.pos += 1;
            } else {
                break;
            }
        }
    }

    fn parse_bool(&mut self) -> Option<bool> {
        if self.bytes[self.pos..].starts_with(b"true") {
            self.pos += 4;
            Some(true)
        } else if self.bytes[self.pos..].starts_with(b"false") {
            self.pos += 5;
            Some(false)
        } else {
            self.skip_value()?;
            None
        }
    }

    fn parse_string_or_null(&mut self) -> Option<String> {
        if self.bytes[self.pos..].starts_with(b"null") {
            self.pos += 4;
            return None;
        }
        self.parse_string()
    }

    fn parse_string(&mut self) -> Option<String> {
        if self.peek_byte()? != b'"' {
            return None;
        }
        self.pos += 1;
        let mut out = String::new();
        while self.pos < self.bytes.len() {
            let b = self.bytes[self.pos];
            match b {
                b'"' => {
                    self.pos += 1;
                    return Some(out);
                }
                b'\\' => {
                    self.pos += 1;
                    let esc = self.peek_byte()?;
                    self.pos += 1;
                    match esc {
                        b'"' => out.push('"'),
                        b'\\' => out.push('\\'),
                        b'/' => out.push('/'),
                        b'n' => out.push('\n'),
                        b'r' => out.push('\r'),
                        b't' => out.push('\t'),
                        b'b' => out.push('\x08'),
                        b'f' => out.push('\x0c'),
                        b'u' => {
                            let hex_bytes = self.bytes.get(self.pos..self.pos + 4)?;
                            let hex_str = std::str::from_utf8(hex_bytes).ok()?;
                            let code = u16::from_str_radix(hex_str, 16).ok()?;
                            self.pos += 4;
                            if (0xD800..=0xDBFF).contains(&code)
                                && self.bytes.get(self.pos..self.pos + 2) == Some(b"\\u")
                            {
                                if let Some(low_str) = self
                                    .bytes
                                    .get(self.pos + 2..self.pos + 6)
                                    .and_then(|s| std::str::from_utf8(s).ok())
                                {
                                    if let Ok(low) = u16::from_str_radix(low_str, 16) {
                                        if (0xDC00..=0xDFFF).contains(&low) {
                                            let cp = 0x10000
                                                + ((u32::from(code) - 0xD800) << 10)
                                                + (u32::from(low) - 0xDC00);
                                            if let Some(ch) = char::from_u32(cp) {
                                                out.push(ch);
                                            }
                                            self.pos += 6;
                                            continue;
                                        }
                                    }
                                }
                            }
                            if let Some(ch) = char::from_u32(u32::from(code)) {
                                out.push(ch);
                            }
                        }
                        other => out.push(other as char),
                    }
                }
                _ => {
                    let rest = std::str::from_utf8(&self.bytes[self.pos..]).ok()?;
                    let ch = rest.chars().next()?;
                    out.push(ch);
                    self.pos += ch.len_utf8();
                }
            }
        }
        None
    }

    fn skip_value(&mut self) -> Option<()> {
        self.skip_ws();
        let mut depth = 0usize;
        loop {
            let b = self.peek_byte()?;
            match b {
                b'"' => {
                    let _ = self.parse_string()?;
                    if depth == 0 {
                        return Some(());
                    }
                }
                b'{' | b'[' => {
                    depth += 1;
                    self.pos += 1;
                }
                b'}' | b']' => {
                    if depth == 0 {
                        return Some(());
                    }
                    depth -= 1;
                    self.pos += 1;
                    if depth == 0 {
                        return Some(());
                    }
                }
                b',' if depth == 0 => return Some(()),
                b' ' | b'\n' | b'\r' | b'\t' if depth == 0 => return Some(()),
                _ => self.pos += 1,
            }
        }
    }
}

fn curl_command() -> Command {
    #[cfg(target_os = "macos")]
    {
        Command::new("/usr/bin/curl")
    }

    #[cfg(target_os = "windows")]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        let sys_root = std::env::var_os("SystemRoot").unwrap_or_else(|| "C:\\Windows".into());
        let curl_exe = std::path::PathBuf::from(sys_root)
            .join("System32")
            .join("curl.exe");
        let mut cmd = if curl_exe.exists() {
            Command::new(curl_exe)
        } else {
            Command::new("curl.exe")
        };
        cmd.creation_flags(CREATE_NO_WINDOW);
        cmd
    }

    #[cfg(all(not(target_os = "macos"), not(target_os = "windows")))]
    {
        if Path::new("/usr/bin/curl").exists() {
            Command::new("/usr/bin/curl")
        } else {
            Command::new("curl")
        }
    }
}

pub fn check_latest_release() -> Result<Option<ReleaseInfo>, String> {
    let mut cmd = curl_command();
    cmd.args([
        "--proto",
        "=https",
        "--proto-redir",
        "=https",
        "--tlsv1.2",
        "-fsSL",
        "--max-time",
        "10",
        "--max-filesize",
        MAX_METADATA_BYTES,
        "-H",
        "Accept: application/vnd.github+json",
        "-H",
        "User-Agent: SquadAir-Updater",
        LATEST_RELEASE_API_URL,
    ]);

    let out = cmd
        .output()
        .map_err(|e| format!("Failed to invoke curl: {e}"))?;
    if !out.status.success() {
        let stderr = String::from_utf8_lossy(&out.stderr);
        return Err(format!("GitHub update check failed: {}", stderr.trim()));
    }
    if out.stdout.len() > 1_048_576 {
        return Err("GitHub release metadata exceeded size limit".to_string());
    }

    let json = String::from_utf8(out.stdout)
        .map_err(|_| "GitHub release metadata is not valid UTF-8".to_string())?;
    let Some(info) = parse_github_release_json(&json) else {
        return Ok(None);
    };

    if is_newer_version(crate::current_app_version(), &info.tag_name) {
        Ok(Some(info))
    } else {
        Ok(None)
    }
}

pub fn download_verify_and_launch(info: ReleaseInfo) -> Result<String, String> {
    if !info.asset_url.starts_with(ALLOWED_DOWNLOAD_PREFIX)
        || !info.sha256sums_url.starts_with(ALLOWED_DOWNLOAD_PREFIX)
    {
        return Err("Untrusted update download URL blocked.".to_string());
    }
    if validate_asset_name(&info.asset_name).is_none() {
        return Err("Invalid installer asset filename blocked.".to_string());
    }

    let mut sums_cmd = curl_command();
    sums_cmd.args([
        "--proto",
        "=https",
        "--proto-redir",
        "=https",
        "--tlsv1.2",
        "-fsSL",
        "--max-time",
        "15",
        "--max-filesize",
        MAX_SUMS_BYTES,
        "-H",
        "User-Agent: SquadAir-Updater",
        &info.sha256sums_url,
    ]);
    let sums_out = sums_cmd
        .output()
        .map_err(|e| format!("Failed to download SHA256SUMS.txt: {e}"))?;
    if !sums_out.status.success() {
        return Err("Failed to fetch SHA256SUMS.txt from GitHub Release.".to_string());
    }
    let sums_text = String::from_utf8_lossy(&sums_out.stdout);
    let expected_sha = lookup_sha256_for_asset(&sums_text, &info.asset_name).ok_or_else(|| {
        format!(
            "SHA-256 checksum entry for '{}' missing in SHA256SUMS.txt.",
            info.asset_name
        )
    })?;

    let update_dir = std::env::temp_dir().join(format!("squad-air-update-{}", std::process::id()));
    create_private_dir(&update_dir)?;

    let part_path = update_dir.join(format!("{}.part", info.asset_name));
    let final_path = update_dir.join(&info.asset_name);

    let mut dl_cmd = curl_command();
    dl_cmd.args([
        "--proto",
        "=https",
        "--proto-redir",
        "=https",
        "--tlsv1.2",
        "-fsSL",
        "--max-time",
        "180",
        "--max-filesize",
        MAX_INSTALLER_BYTES_STR,
        "-H",
        "User-Agent: SquadAir-Updater",
        "-o",
    ]);
    dl_cmd.arg(&part_path);
    dl_cmd.arg(&info.asset_url);

    let dl_out = dl_cmd
        .output()
        .map_err(|e| format!("Failed to download installer: {e}"))?;
    if !dl_out.status.success() {
        let _ = fs::remove_file(&part_path);
        return Err("Failed to download installer package from GitHub.".to_string());
    }

    let meta = fs::metadata(&part_path)
        .map_err(|e| format!("Failed to inspect downloaded installer: {e}"))?;
    if meta.len() == 0 || meta.len() > MAX_INSTALLER_BYTES {
        let _ = fs::remove_file(&part_path);
        return Err("Downloaded installer size is outside safe bounds.".to_string());
    }

    let bytes = fs::read(&part_path).map_err(|e| {
        let _ = fs::remove_file(&part_path);
        format!("Failed to read downloaded installer: {e}")
    })?;
    let actual_sha = sha256_hex(&bytes);
    drop(bytes);

    if actual_sha != expected_sha {
        let _ = fs::remove_file(&part_path);
        return Err("SHA-256 verification failed; update aborted.".to_string());
    }

    fs::rename(&part_path, &final_path).map_err(|e| {
        let _ = fs::remove_file(&part_path);
        format!("Failed to finalize installer path: {e}")
    })?;

    launch_verified_installer(&update_dir, &final_path, &info.asset_name)?;
    Ok(format!(
        "SHA-256 verified! Launched {} ({}).",
        info.asset_name, info.tag_name
    ))
}

fn create_private_dir(dir: &Path) -> Result<(), String> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::{DirBuilderExt, PermissionsExt};
        if !dir.exists() {
            fs::DirBuilder::new()
                .recursive(true)
                .mode(0o700)
                .create(dir)
                .map_err(|e| format!("Failed to create update staging directory: {e}"))?;
        }
        fs::set_permissions(dir, fs::Permissions::from_mode(0o700))
            .map_err(|e| format!("Failed to lock down update staging directory: {e}"))?;
    }
    #[cfg(not(unix))]
    {
        fs::create_dir_all(dir)
            .map_err(|e| format!("Failed to create update staging directory: {e}"))?;
    }
    Ok(())
}

fn launch_verified_installer(
    update_dir: &Path,
    final_path: &Path,
    asset_name: &str,
) -> Result<(), String> {
    let _ = (update_dir, asset_name);

    #[cfg(target_os = "macos")]
    {
        Command::new("/usr/bin/open")
            .arg(final_path)
            .spawn()
            .map_err(|e| format!("Failed to open macOS installer: {e}"))?;
        Ok(())
    }

    #[cfg(target_os = "windows")]
    {
        Command::new(final_path)
            .spawn()
            .map_err(|e| format!("Failed to launch Windows setup installer: {e}"))?;
        Ok(())
    }

    #[cfg(all(not(target_os = "macos"), not(target_os = "windows")))]
    {
        if asset_name.ends_with(".tar.gz") {
            let status = Command::new("tar")
                .arg("-xzf")
                .arg(final_path)
                .arg("-C")
                .arg(update_dir)
                .status()
                .map_err(|e| format!("Failed to extract Linux release archive: {e}"))?;
            if !status.success() {
                return Err("Failed to extract Linux release archive.".to_string());
            }
            Command::new("xdg-open")
                .arg(update_dir)
                .spawn()
                .map_err(|e| format!("Failed to open extracted folder: {e}"))?;
        } else {
            Command::new("xdg-open")
                .arg(final_path)
                .spawn()
                .map_err(|e| format!("Failed to open Linux package installer: {e}"))?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_sha256_nist_vectors() {
        assert_eq!(
            sha256_hex(b""),
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
        assert_eq!(
            sha256_hex(b"abc"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
        assert_eq!(
            sha256_hex(b"abcdbcdecdefdefgefghfghighijhijkijkljklmklmnlmnomnopnopq"),
            "248d6a61d20638b8e5c026930c3e6039a33ce45964ff2167f6ecedd419db06c1"
        );
    }

    #[test]
    fn test_version_parsing_and_comparison() {
        assert_eq!(parse_version_tag("v1.0"), Some((1, 0, 0)));
        assert_eq!(parse_version_tag("1.10.2"), Some((1, 10, 2)));
        assert_eq!(parse_version_tag("invalid"), None);

        assert!(is_newer_version("1.0.0", "v1.1"));
        assert!(is_newer_version("v1.1", "v1.10"));
        assert!(is_newer_version("1.0.0", "v1.0.1"));
        assert!(!is_newer_version("1.0.0", "v1.0"));
        assert!(!is_newer_version("v1.2", "v1.1"));
    }

    #[test]
    fn test_lookup_sha256_for_asset() {
        let sums = "\
aaaaaaaaaaaaaaaabbbbbbbbbbbbbbbbccccccccccccccccdddddddddddddddd  SquadAir-Installer-macos-arm64.pkg
1111111111111111222222222222222233333333333333334444444444444444 *SquadAir-Setup-windows-x86_64.exe
";
        assert_eq!(
            lookup_sha256_for_asset(sums, "SquadAir-Installer-macos-arm64.pkg").as_deref(),
            Some("aaaaaaaaaaaaaaaabbbbbbbbbbbbbbbbccccccccccccccccdddddddddddddddd")
        );
        assert_eq!(
            lookup_sha256_for_asset(sums, "SquadAir-Setup-windows-x86_64.exe").as_deref(),
            Some("1111111111111111222222222222222233333333333333334444444444444444")
        );
        assert_eq!(lookup_sha256_for_asset(sums, "missing.deb"), None);
    }

    #[test]
    fn test_parse_github_release_json_and_security_filters() {
        let sample_json = r####"{
  "tag_name": "v1.2",
  "html_url": "https://github.com/alivergg/squad-air/releases/tag/v1.2",
  "draft": false,
  "prerelease": false,
  "assets": [
    {
      "name": "SquadAir-Installer-macos-arm64.pkg",
      "browser_download_url": "https://github.com/alivergg/squad-air/releases/download/v1.2/SquadAir-Installer-macos-arm64.pkg"
    },
    {
      "name": "SquadAir-Setup-windows-x86_64.exe",
      "browser_download_url": "https://github.com/alivergg/squad-air/releases/download/v1.2/SquadAir-Setup-windows-x86_64.exe"
    },
    {
      "name": "squad-air_1.2_amd64.deb",
      "browser_download_url": "https://github.com/alivergg/squad-air/releases/download/v1.2/squad-air_1.2_amd64.deb"
    },
    {
      "name": "SHA256SUMS.txt",
      "browser_download_url": "https://github.com/alivergg/squad-air/releases/download/v1.2/SHA256SUMS.txt"
    }
  ],
  "body": "### What's Changed\r\n- Added 1-click \"auto\" updater\r\n- Fixed bug"
}"####;

        let win = parse_github_release_json_with(sample_json, |_| {
            vec!["SquadAir-Setup-windows-x86_64.exe".to_string()]
        })
        .expect("must parse release for windows asset");
        assert_eq!(win.tag_name, "v1.2");
        assert_eq!(win.version_num, "1.2");
        assert_eq!(win.asset_name, "SquadAir-Setup-windows-x86_64.exe");
        assert!(win.notes.contains("Added 1-click \"auto\" updater"));

        let deb = parse_github_release_json_with(sample_json, |ver| {
            vec![format!("squad-air_{ver}_amd64.deb")]
        })
        .expect("must parse release for debian asset");
        assert_eq!(deb.asset_name, "squad-air_1.2_amd64.deb");

        let untrusted_url_json = sample_json.replace(
            "https://github.com/alivergg/squad-air/releases/download/",
            "https://evil.example.com/",
        );
        assert!(parse_github_release_json_with(&untrusted_url_json, |_| {
            vec!["SquadAir-Setup-windows-x86_64.exe".to_string()]
        })
        .is_none());
    }
}
