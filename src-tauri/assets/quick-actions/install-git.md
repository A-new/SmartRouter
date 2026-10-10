# Git Installation — Agent Instructions

You are installing **Git** (the version-control CLI) on the user's
machine. Execute autonomously — the installer is ~60 MB, no
ask-before-download gate needed. Fail loud when install genuinely
cannot proceed; do NOT silently retry or fudge state.

**Scope**: install the `git` CLI and verify it runs. Do NOT touch the
user's git identity (`user.name` / `user.email`), do NOT install GUI
clients, and do NOT configure SSH keys — those are the user's own
choices. If they ask, answer; otherwise leave configuration alone.

Reply in the user's UI language. The trigger phrase tells you which one:
- "Help me install Git" → English
- "帮我安装 Git" → Simplified Chinese
- "幫我安裝 Git" → Traditional Chinese
- "Git のインストールを支援" → Japanese

## Step 0: Presence check

Run `git --version`.

- Exit 0 → Git is already installed. Report the version and STOP.
  Do not reinstall or upgrade unless the user explicitly asks.
- Command not found → proceed to the platform step below.

On Windows ALSO probe the standard install location before declaring
absence — PATH may simply be stale in this shell:
`& "C:\Program Files\Git\cmd\git.exe" --version` (PowerShell 5.1).
If that works, Git is installed but PATH needs a new terminal; tell
the user and STOP.

## Windows (PowerShell 5.1)

1. **winget (official path, try first):**
   `winget install --id Git.Git -e --source winget --accept-source-agreements --accept-package-agreements --silent`
   The Git.Git package downloads from GitHub releases. If winget is
   missing (LTSC / stripped images) or the download stalls/fails
   (common on mainland-China networks without a VPN), fall through to
   step 2 — do NOT loop retries.
2. **npmmirror binary mirror (mainland-friendly fallback):**
   - List versions: `https://registry.npmmirror.com/-/binary/git-for-windows/`
     (JSON array of directories). Pick the newest stable
     `v<X.Y.Z>.windows.1/` entry (skip `-rc` prereleases).
   - Inside it, download `Git-<X.Y.Z>-64-bit.exe` to `$env:TEMP`.
   - Run silently: `Start-Process -Wait <exe> -ArgumentList '/VERYSILENT','/NORESTART','/NOCANCEL'`
     The installer is per-machine and raises UAC itself — if the user
     declines, the exit code surfaces the failure; report it, don't
     re-launch in a loop.
3. **Verify** in the CURRENT session via the full path
   (`& "C:\Program Files\Git\cmd\git.exe" --version`), then remind the
   user that new terminals (and SmartRouter-launched tools) pick up `git`
   from PATH automatically; an already-open terminal needs reopening.

## macOS

1. If Homebrew exists (`which brew`) → `brew install git`.
2. Otherwise → `xcode-select --install`. This opens a GUI dialog the
   USER must click to confirm (Apple CDN, works in mainland China).
   Tell them to click Install, then poll `git --version` every ~15 s
   until it succeeds (the CLT package is a few hundred MB; say so).
   Note: macOS ships an Apple `git` stub via the CLT path — after the
   dialog completes, `git --version` just works.

## Linux

Use the distro package manager, with sudo:
- Debian/Ubuntu: `sudo apt-get update && sudo apt-get install -y git`
- Fedora/RHEL: `sudo dnf install -y git`
- Arch: `sudo pacman -S --noconfirm git`
- openSUSE: `sudo zypper install -y git`
If sudo prompts for a password the shell cannot supply, give the user
the exact command to run themselves and STOP — do not work around
privilege boundaries.

## Final verification

`git --version` (or the Windows full path) must print a version.
Report the installed version in one line. Do not suggest follow-up
configuration unless the user asks.
