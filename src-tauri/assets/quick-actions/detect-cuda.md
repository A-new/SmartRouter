# CUDA Module Detection — Agent Instructions

You are diagnosing whether the user's Windows machine has the **CUDA
modules** (= CUDA Toolkit / `cudart64_*.dll` user-mode libraries) needed
for NVIDIA GPU code to run.

**Scope**: this script ONLY inspects CUDA modules and the prerequisites
to install them (NVIDIA driver, Windows edition health). Anything about
the engine — SmartRouter's CUDA-build of the inference engine, the SETUP
ENGINE button — is **out of scope**. Do NOT mention it. The user
handles the engine themselves through SmartRouter's normal one-click UI;
that flow is independent of this script.

**Read-only mode**: this script ONLY inspects. Do NOT install, enable
services, modify env vars, or change anything. If remediation is needed,
point the user at the "Help me install CUDA modules" button and stop.

## Terminology

- **NVIDIA driver** — kernel-mode display + GPU compute driver from
  NVIDIA. Provides `nvidia-smi`. **Not the same as CUDA modules.**
  Having the driver does NOT mean having CUDA modules.
- **CUDA modules** — the user-mode CUDA Toolkit. Authoritative
  presence check: `cudart64_*.dll` is reachable. This is what the
  "CUDA module" UI label refers to.
- `nvidia-smi`'s "CUDA Version: X.Y" line is the **driver's API
  ceiling**, NOT the installed Toolkit version. A user can see
  "CUDA Version: 12.6" in `nvidia-smi` with zero CUDA modules
  installed on disk.

All commands here are PowerShell. The user's shell is `powershell.exe`
(Windows PowerShell 5.1) — do NOT assume pwsh 7 features.

Reply in the user's UI language. The trigger phrase tells you which one:
- "Detect CUDA module status" → English
- "检测CUDA模块状态" → Simplified Chinese
- "檢測 CUDA 模組狀態" → Traditional Chinese
- "CUDA モジュールの状態を確認" → Japanese

## Step 1: Platform Gate

```powershell
(Get-CimInstance Win32_OperatingSystem).Caption
```

If NOT Windows: tell the user this script is Windows-only and stop.
macOS has no CUDA (Metal instead); Linux users should install
`cuda-toolkit` from their distro package manager.

## Step 2: GPU Inventory

```powershell
Get-CimInstance Win32_VideoController | Select-Object Name, DriverVersion, AdapterRAM
```

- **NVIDIA card present** → continue to Step 3.
- **AMD / Intel only OR no GPU** → tell the user CUDA does not apply
  to their hardware and stop. None of the rest matters.

## Step 3: NVIDIA Driver

```powershell
nvidia-smi
```

Parse:
- Driver version (e.g. `551.61`)
- "CUDA Version: X.Y" line (driver API ceiling, not module state)

If `nvidia-smi` is not found:
- Try `& "C:\Program Files\NVIDIA Corporation\NVSMI\nvidia-smi.exe"`
  (older drivers) or `& "C:\Windows\System32\nvidia-smi.exe"` (newer).
- Record as **driver missing** if both attempts fail.

## Step 4: CUDA Modules — the main probe

**Critical**: do NOT trust `$env:CUDA_PATH` / `$env:Path` /
`Get-Command nvcc` — the agent's PowerShell session inherited
SmartRouter's process env at startup. If install-cuda just ran (or any
out-of-band CUDA install happened), the system registry has the new
env vars but this session does NOT. Reading `$env:*` will misreport
"unset" right after a successful install.

**Read everything from the registry.** It's the authoritative source
that survives process-env staleness.

```powershell
# 4a. System env vars — authoritative source (NOT $env:)
$sysEnv  = Get-ItemProperty 'HKLM:\SYSTEM\CurrentControlSet\Control\Session Manager\Environment' -ErrorAction SilentlyContinue
$userEnv = Get-ItemProperty 'HKCU:\Environment' -ErrorAction SilentlyContinue

$cudaPath = $sysEnv.CUDA_PATH                 # the canonical CUDA_PATH
$versionedPaths = $sysEnv.PSObject.Properties |
                  Where-Object { $_.Name -like 'CUDA_PATH_V*' } |
                  Select-Object Name, Value   # any CUDA_PATH_V12_4 etc.

# 4b. NVIDIA Toolkit registry — authoritative "is Toolkit installed?"
$toolkitInstalls = Get-ItemProperty "HKLM:\SOFTWARE\NVIDIA Corporation\GPU Computing Toolkit\CUDA\*" -ErrorAction SilentlyContinue
# Each entry's InstallDir points at the install root (e.g.
# "C:\Program Files\NVIDIA GPU Computing Toolkit\CUDA\v12.6\")

# 4c. THE decisive probe — cudart64_*.dll on disk
$cudartInSystem32 = Get-ChildItem C:\Windows\System32\cudart64_*.dll -ErrorAction SilentlyContinue
$cudartInToolkit  = if ($cudaPath) {
    Get-ChildItem "$cudaPath\bin\cudart64_*.dll" -ErrorAction SilentlyContinue
}

# 4d. PATH check — read from registry, not $env:Path
$systemPath = $sysEnv.Path -split ';'
$userPath   = if ($userEnv -and $userEnv.Path) { $userEnv.Path -split ';' } else { @() }
$cudaOnPath = ($systemPath + $userPath) | Where-Object { $_ -like '*\CUDA\v*\bin*' }

# 4e. nvcc.exe on disk (not via Get-Command — that uses stale PATH)
$nvccExe = if ($toolkitInstalls) {
    foreach ($t in $toolkitInstalls) {
        $candidate = Join-Path $t.PSObject.Properties['InstallDir'].Value 'bin\nvcc.exe'
        if (Test-Path $candidate) { $candidate }
    }
}
```

Decision rules (all read from registry / disk, NOT process env):

- `cudart64_*.dll` reachable in System32 OR `$cudaPath\bin\` →
  **modules present**.
- `cudart64_*.dll` absent everywhere → **modules missing**.
- `$cudaPath` non-empty in the system registry → **CUDA_PATH is set**.
  Do NOT report "CUDA_PATH not set" just because `$env:CUDA_PATH` is
  blank — fresh SmartRouter processes will see the registry value.
- `$cudaOnPath` non-empty → **PATH contains CUDA bin**. Same rule:
  trust the registry, not `$env:Path`.
- `$nvccExe` exists on disk → **nvcc is installed and reachable to
  any fresh shell**. Do NOT report "nvcc not in PATH" if nvcc.exe is
  on disk and PATH contains its parent dir in the registry.

`nvcc` and the NVIDIA registry key enrich the report; the decisive
module-presence check is still `cudart64_*.dll` on disk.

## Step 5: Windows Edition — the stripped / Home-edition trap

The single most-asked support question. Probe carefully and report clearly.

```powershell
$os = Get-CimInstance Win32_OperatingSystem
$os.Caption          # e.g. "Microsoft Windows 11 专业版"
$os.OperatingSystemSKU
$os.BuildNumber
$os.Version
```

SKU decode:
- 4, 27, 70, 84 → Enterprise / LTSC variants
- 48 → Pro
- 49 → Pro N (Europe — media stack stripped)
- 98, 100, 101 → Home / Home N / Home Single Language
- 121 → Education

Flag these Caption substrings as **modified Windows** — they almost
always cause CUDA install pain:
- `tiny11`, `Tiny11`, `Atlas`, `AtlasOS`, `ReviOS`, `AME`, `Ghost`,
  `精简`, `优化`, `纯净`, `Lite`.

Even on un-modified editions, probe the components a CUDA Toolkit
install needs:

```powershell
# 5a. Windows Installer service
Get-Service msiserver | Select-Object Status, StartType

# 5b. Trusted Installer
Get-Service TrustedInstaller | Select-Object Status, StartType

# 5c. Visual C++ Redistributable — CUDA Toolkit depends on it
Test-Path "C:\Windows\System32\vcruntime140.dll"
Test-Path "C:\Windows\System32\msvcp140.dll"
Test-Path "C:\Windows\System32\vcruntime140_1.dll"
Get-ItemProperty "HKLM:\SOFTWARE\Microsoft\VisualStudio\14.0\VC\Runtimes\x64" -ErrorAction SilentlyContinue

# 5d. Universal C Runtime — sometimes stripped
Test-Path "C:\Windows\System32\ucrtbase.dll"
```

## Step 6: Verdict Report

Render a single structured report. Translate the labels into the user's
UI language; keep the SHAPE identical so users posting screenshots are
easy to support.

```
═════════════════════════════════════════════════════
CUDA Module Status Report
═════════════════════════════════════════════════════

[Hardware]
GPU model                     : NVIDIA RTX 4090   (or "no NVIDIA GPU")
VRAM                          : 24 GB

[NVIDIA driver]
Driver version                : 551.61            (or "not installed" ⚠)
Driver-supported max CUDA     : 12.4              (driver API ceiling, NOT
                                                   the installed module version)

[CUDA modules]  ← what this report is really about
                              (all fields read from registry / disk —
                              NOT from this shell's stale process env)
cudart64_*.dll                : found in System32 / found in CUDA_PATH / NOT FOUND ⚠
CUDA_PATH (registry)          : C:\Program Files\NVIDIA GPU Computing Toolkit\CUDA\v12.4
                                (or "not set in registry")
PATH contains CUDA bin        : ✓ yes / ✗ no  (registry-read, not $env:Path)
Installed Toolkit version     : 12.4              (or "not installed")
nvcc.exe on disk              : C:\...\CUDA\v12.4\bin\nvcc.exe  (or "not present")

[Windows]
Edition                       : Windows 11 Pro (build 22631)
SKU                           : 48 (Pro)

[Critical services]
Windows Installer (msiserver) : Running / Stopped⚠ / Disabled⚠
Trusted Installer             : Manual / Disabled⚠

[Runtime libraries]
Visual C++ Runtime            : ✓ vcruntime140.dll, msvcp140.dll, vcruntime140_1.dll all present
Universal C Runtime           : ✓ ucrtbase.dll present

[⚠ Modified / stripped Windows]
(Only emit this block when tiny11/Atlas/ReviOS/AME/Ghost/精简/优化/纯净/Lite detected.)
Your Windows is **<exact name detected>**. System components needed by
CUDA have been removed. Toolkit installs on this kind of system fail
roughly 80% of the time, and the failures are usually outside what
SmartRouter can repair. Reinstalling an official Windows image is the
reliable fix.

═════════════════════════════════════════════════════
Verdict: see Step 7
═════════════════════════════════════════════════════
```

## Step 7: Verdict + Next Step (exactly one)

Combine driver + module state:

- **Driver ✓ AND modules ✓** → "Ready":
  > "Your CUDA modules are installed and ready."

- **Driver ✓ AND modules ✗** → "Modules missing":
  > "Your NVIDIA driver is installed, but the CUDA modules
  > (`cudart64_*.dll`) are missing. Click the **Help me install
  > CUDA modules** button to install them."

- **Driver ✗** → "Driver missing":
  > "Your NVIDIA driver isn't installed (`nvidia-smi` is not
  > available). Install the latest **Game Ready** or **Studio**
  > driver from `https://www.nvidia.com/Download/index.aspx`,
  > reboot, then re-run this detection."

- **Modified / stripped Windows** → "Refuse on this OS":
  > "Your Windows is a stripped build (tiny11/Atlas/ReviOS/etc.).
  > Installing CUDA on this fails ~80% of the time. The reliable
  > fix is reinstalling an official Windows image — don't waste
  > time patching this build."

- **No NVIDIA GPU** → "Not applicable":
  > "There is no NVIDIA GPU in this machine, so CUDA does not
  > apply."

## Tone & Boundaries

- Never `web_fetch` — every probe above is a local PowerShell command.
- Never modify system state in this script. Detect-only.
- Don't dump raw command output line-by-line. Interpret first, then
  report.
- If a probe errors out (access denied, unknown command), record that
  as a finding ("could not read registry — possibly insufficient
  permissions") and continue. Don't abort the scan over one failed
  probe.
- **Stay in scope**: CUDA modules + the prerequisites to install them.
  Do NOT mention SmartRouter's engine, the SETUP ENGINE button, GPU
  acceleration in SmartRouter, or "what happens after modules are
  installed". That is the user's normal one-click UI flow and is
  independent of this script.
- **Never conflate NVIDIA driver with CUDA modules.** Separate pieces.
- **Never trust `$env:*` for module-presence decisions.** This script
  often runs right after install-cuda; the agent's shell inherited
  SmartRouter's process env at startup and the new env vars set by the
  CUDA installer won't be visible until SmartRouter itself restarts.
  Always read from `HKLM:\SYSTEM\CurrentControlSet\Control\Session
  Manager\Environment` (system) and `HKCU:\Environment` (user). Apply
  the same rule to PATH and to nvcc.exe (use Test-Path on the
  registry-resolved InstallDir, not `Get-Command nvcc`).
