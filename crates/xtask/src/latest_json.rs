//! `latest.json`, the manifest Tauri's updater reads: for each platform,
//! where its update file is and the file's signature.

use std::path::Path;

use chrono::{DateTime, SecondsFormat, Utc};
use serde_json::{Value, json};

use crate::Error;

/// Each platform the updater asks about, and the file it downloads. Both
/// Macs take the universal app.
const PLATFORMS: [(&str, &str); 4] = [
    ("darwin-aarch64", "Epik.app.tar.gz"),
    ("darwin-x86_64", "Epik.app.tar.gz"),
    ("windows-x86_64", "Epik.msi"),
    ("linux-x86_64", "Epik_amd64.AppImage"),
];

/// The manifest for release `tag` of `version`, whose update files and
/// their `.sig` files are in `dir`. A signature is the `.sig` file's
/// content: Tauri takes no path or URL there.
pub fn manifest(
    version: &str,
    tag: &str,
    dir: &Path,
    pub_date: DateTime<Utc>,
) -> Result<Value, Error> {
    let mut platforms = serde_json::Map::new();
    for (platform, file) in PLATFORMS {
        let sig = dir.join(format!("{file}.sig"));
        let signature =
            std::fs::read_to_string(&sig).map_err(|e| format!("{}: {e}", sig.display()))?;
        platforms.insert(
            platform.into(),
            json!({
                "signature": signature.trim(),
                "url": format!("https://github.com/epik-agent/Epik/releases/download/{tag}/{file}"),
            }),
        );
    }
    Ok(json!({
        "version": version,
        "pub_date": pub_date.to_rfc3339_opts(SecondsFormat::Secs, true),
        "platforms": platforms,
    }))
}

/// Writes `dir/latest.json`, published now.
pub fn write(version: &str, tag: &str, dir: &Path) -> Result<(), Error> {
    let manifest = manifest(version, tag, dir, Utc::now())?;
    let path = dir.join("latest.json");
    std::fs::write(&path, serde_json::to_string_pretty(&manifest)?)?;
    println!("Wrote {}", path.display());
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A directory holding a `.sig` for each of `files`, whose content
    /// names the file.
    fn signed(files: &[&str]) -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        for file in files {
            std::fs::write(dir.path().join(file), "update").unwrap();
            std::fs::write(
                dir.path().join(format!("{file}.sig")),
                format!("sig of {file}\n"),
            )
            .unwrap();
        }
        dir
    }

    fn all() -> tempfile::TempDir {
        signed(&["Epik.app.tar.gz", "Epik.msi", "Epik_amd64.AppImage"])
    }

    fn date() -> DateTime<Utc> {
        "2026-11-05T01:12:54Z".parse().unwrap()
    }

    #[test]
    fn the_manifest_names_the_version_date_and_every_platform() {
        let dir = all();
        let manifest = manifest("0.4.0", "v0.4.0", dir.path(), date()).unwrap();
        assert_eq!(
            manifest,
            json!({
                "version": "0.4.0",
                "pub_date": "2026-11-05T01:12:54Z",
                "platforms": {
                    "darwin-aarch64": {
                        "signature": "sig of Epik.app.tar.gz",
                        "url": "https://github.com/epik-agent/Epik/releases/download/v0.4.0/Epik.app.tar.gz",
                    },
                    "darwin-x86_64": {
                        "signature": "sig of Epik.app.tar.gz",
                        "url": "https://github.com/epik-agent/Epik/releases/download/v0.4.0/Epik.app.tar.gz",
                    },
                    "windows-x86_64": {
                        "signature": "sig of Epik.msi",
                        "url": "https://github.com/epik-agent/Epik/releases/download/v0.4.0/Epik.msi",
                    },
                    "linux-x86_64": {
                        "signature": "sig of Epik_amd64.AppImage",
                        "url": "https://github.com/epik-agent/Epik/releases/download/v0.4.0/Epik_amd64.AppImage",
                    },
                },
            })
        );
    }

    #[test]
    fn a_missing_signature_is_an_error_naming_it() {
        let dir = signed(&["Epik.app.tar.gz", "Epik_amd64.AppImage"]);
        let error = manifest("0.4.0", "v0.4.0", dir.path(), date()).unwrap_err();
        assert!(error.to_string().contains("Epik.msi.sig"), "{error}");
    }

    #[test]
    fn write_puts_latest_json_beside_the_files() {
        let dir = all();
        write("0.4.0", "v0.4.0", dir.path()).unwrap();
        let written: Value =
            serde_json::from_str(&std::fs::read_to_string(dir.path().join("latest.json")).unwrap())
                .unwrap();
        assert_eq!(written["version"], "0.4.0");
        assert_eq!(written["platforms"].as_object().unwrap().len(), 4);
    }
}
