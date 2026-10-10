//! In-app self-update (Windows) — DIY flow.
//!
//! Download the installer from GitHub releases, launch its wizard, then exit
//! so the installer can
//! replace our files. The user clicks through the official installer; there is
//! NO silent run and NO signature verification here — the signed + silent +
//! auto-relaunch alternative is `tauri-plugin-updater`, deliberately not adopted
//! yet (it needs signing keys + an updater manifest in the release flow).
//!
//! Non-Windows is not supported in-app (the caller opens the download page
//! instead): macOS .dmg / Linux .deb|.rpm aren't "run the installer and exit".

#[cfg(target_os = "windows")]
use std::path::Path;
#[cfg(target_os = "windows")]
use std::time::Duration;
use tauri::AppHandle;
#[cfg(target_os = "windows")]
use tauri::Emitter;

/// Event the Settings dialog listens to for a progress bar.
#[cfg(target_os = "windows")]
const PROGRESS_EVENT: &str = "self-update-progress";

#[cfg(target_os = "windows")]
#[derive(Clone, serde::Serialize)]
struct UpdateProgress {
    /// "speed_test" | "downloading" | "launching" | "error"
    status: &'static str,
    percent: u8,
}

#[cfg(target_os = "windows")]
fn emit(app: &AppHandle, status: &'static str, percent: u8) {
    let _ = app.emit(PROGRESS_EVENT, UpdateProgress { status, percent });
}

/// Windows x64 installer asset name — the only in-app update target. Matches
/// the released asset on GitHub.
#[cfg(target_os = "windows")]
fn installer_asset(version: &str) -> String {
    format!("SmartRouter_{version}_Windows_x64-setup.exe")
}

/// Download candidate: GitHub releases is the only source now.
/// `pick_fastest` probes it and falls back to it if unreachable.
#[cfg(target_os = "windows")]
fn candidate_urls(version: &str, asset: &str) -> Vec<String> {
    vec![format!(
        "https://github.com/edison7009/EchoBird/releases/download/v{version}/{asset}"
    )]
}

/// Time a tiny ranged GET from each URL; return the fastest that responds.
/// Falls back to the first candidate if none answer within the probe window.
#[cfg(target_os = "windows")]
async fn pick_fastest(urls: &[String]) -> Option<String> {
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(6))
        .build()
        .ok()?;
    let mut best: Option<(String, Duration)> = None;
    for url in urls {
        let start = std::time::Instant::now();
        let ok = client
            .get(url)
            .header("Range", "bytes=0-2047")
            .send()
            .await
            .map(|r| r.status().is_success() || r.status().as_u16() == 206)
            .unwrap_or(false);
        if ok {
            let dt = start.elapsed();
            if best.as_ref().map(|(_, b)| dt < *b).unwrap_or(true) {
                best = Some((url.clone(), dt));
            }
        }
    }
    best.map(|(u, _)| u).or_else(|| urls.first().cloned())
}

#[cfg(target_os = "windows")]
async fn download_to(app: &AppHandle, url: &str, dest: &Path) -> Result<(), String> {
    use futures_util::StreamExt;
    use std::io::Write;

    let resp = reqwest::Client::new()
        .get(url)
        .send()
        .await
        .map_err(|e| e.to_string())?;
    if !resp.status().is_success() {
        return Err(format!("download returned HTTP {}", resp.status()));
    }
    let total = resp.content_length().unwrap_or(0);
    let mut file = std::fs::File::create(dest).map_err(|e| e.to_string())?;
    let mut downloaded: u64 = 0;
    let mut last_pct = 0u8;
    let mut stream = resp.bytes_stream();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|e| e.to_string())?;
        file.write_all(&chunk).map_err(|e| e.to_string())?;
        downloaded += chunk.len() as u64;
        if let Some(pct) = downloaded.saturating_mul(100).checked_div(total) {
            let pct = pct.min(100) as u8;
            if pct != last_pct {
                last_pct = pct;
                emit(app, "downloading", pct);
            }
        }
    }
    file.flush().map_err(|e| e.to_string())?;
    // Guard against a silently-truncated download (dropped connection / proxy
    // cutoff): never hand a partial .exe to the installer launcher. With no
    // signature or hash check, this size check is the only integrity guard.
    if total > 0 && downloaded != total {
        let _ = std::fs::remove_file(dest);
        return Err(format!(
            "incomplete download: {downloaded} of {total} bytes"
        ));
    }
    Ok(())
}

/// Launch the installer DETACHED + visible (the wizard), so terminating our
/// own process afterward doesn't take it down.
#[cfg(target_os = "windows")]
fn spawn_installer(path: &Path) -> Result<(), String> {
    use std::os::windows::process::CommandExt;
    const DETACHED_PROCESS: u32 = 0x00000008;
    const CREATE_NEW_PROCESS_GROUP: u32 = 0x00000200;
    std::process::Command::new(path)
        .creation_flags(DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP)
        .spawn()
        .map(|_| ())
        .map_err(|e| format!("failed to launch installer: {e}"))
}

/// Windows: download the installer (fastest source), launch it, then exit so
/// it can overwrite our running files. `Err` is surfaced to the UI, which then
/// falls back to opening the download page.
#[cfg(target_os = "windows")]
pub async fn download_and_install(app: AppHandle, version: String) -> Result<(), String> {
    let asset = installer_asset(&version);
    let urls = candidate_urls(&version, &asset);
    emit(&app, "speed_test", 0);
    let url = pick_fastest(&urls)
        .await
        .ok_or_else(|| "no reachable download source".to_string())?;
    let dest = std::env::temp_dir().join(&asset);
    download_to(&app, &url, &dest).await?;
    emit(&app, "launching", 100);
    spawn_installer(&dest)?;
    // Give the installer a moment to start, then exit so our .exe unlocks and
    // the installer can replace it. Graceful exit (not a self-kill) so window
    // state + child-process cleanup run.
    tokio::time::sleep(Duration::from_millis(600)).await;
    app.exit(0);
    Ok(())
}

#[cfg(not(target_os = "windows"))]
pub async fn download_and_install(_app: AppHandle, _version: String) -> Result<(), String> {
    Err("in-app update is Windows-only".to_string())
}
