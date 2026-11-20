param(
    [string]$Version = "",
    [switch]$SelfSign,
    [string]$CertSubject = "CN=3E59CE0D-CA59-4794-A596-D535A5F4C1FE"
)

$ErrorActionPreference = "Stop"

$RootDir = Split-Path -Parent (Split-Path -Parent $MyInvocation.MyCommand.Path)
Set-Location $RootDir

if ([string]::IsNullOrWhiteSpace($Version)) {
    $CargoToml = Get-Content (Join-Path $RootDir "Cargo.toml") -Raw
    if ($CargoToml -match '(?m)^version\s*=\s*"([^"]+)"') {
        $Version = $Matches[1]
    } else {
        $Version = "1.0.0"
    }
}

$CleanVersion = $Version.Trim().TrimStart('v', 'V')
$Parts = $CleanVersion.Split('.')
$Major = if ($Parts.Length -ge 1) { [int]$Parts[0] } else { 1 }
$Minor = if ($Parts.Length -ge 2) { [int]$Parts[1] } else { 0 }
$Patch = if ($Parts.Length -ge 3) { [int]$Parts[2] } else { 0 }
# Microsoft Store reserves the 4th MSIX version component and expects X.Y.Z.0.
$MsixVersion = "$Major.$Minor.$Patch.0"

Write-Host "==> [1/4] Building squad-client (release, ms_store feature, v$CleanVersion)..." -ForegroundColor Cyan
$env:SQUADAIR_VERSION = $CleanVersion
cargo build --release --locked -p squad-client --features ms_store
if ($LASTEXITCODE -ne 0) {
    throw "cargo build --release --features ms_store failed with exit code $LASTEXITCODE"
}

$ExePath = Join-Path $RootDir "target\release\squad-client.exe"
if (-not (Test-Path $ExePath)) {
    throw "Expected binary missing: $ExePath"
}

$StagingDir = Join-Path $RootDir "target\msix-staging"
$DistDir = Join-Path $RootDir "dist"
$MsixOut = Join-Path $DistDir "SquadAir-msix-windows-x86_64.msix"

if (Test-Path $StagingDir) {
    Remove-Item -Recurse -Force $StagingDir
}
New-Item -ItemType Directory -Force -Path $StagingDir | Out-Null
New-Item -ItemType Directory -Force -Path $DistDir | Out-Null

Write-Host "==> [2/4] Staging MSIX payload (version $MsixVersion)..." -ForegroundColor Cyan
Copy-Item $ExePath (Join-Path $StagingDir "SquadAir.exe") -Force
Copy-Item (Join-Path $RootDir "installer\msix\Assets") (Join-Path $StagingDir "Assets") -Recurse -Force

$ManifestTemplate = Get-Content (Join-Path $RootDir "installer\msix\AppxManifest.xml.in") -Raw
$ManifestContent = $ManifestTemplate.Replace("__MSIX_VERSION__", $MsixVersion)
[System.IO.File]::WriteAllText(
    (Join-Path $StagingDir "AppxManifest.xml"),
    $ManifestContent,
    [System.Text.UTF8Encoding]::new($false)
)

function Find-WindowsSdkTool([string]$ToolName) {
    $Cmd = Get-Command $ToolName -ErrorAction SilentlyContinue
    if ($Cmd) {
        return $Cmd.Source
    }
    $KitRoots = @(
        "${env:ProgramFiles(x86)}\Windows Kits\10\bin",
        "${env:ProgramFiles}\Windows Kits\10\bin"
    )
    foreach ($Root in $KitRoots) {
        if (Test-Path $Root) {
            $Found = Get-ChildItem -Path $Root -Filter $ToolName -Recurse -ErrorAction SilentlyContinue |
                Where-Object { $_.FullName -match '\\x64\\' } |
                Sort-Object FullName -Descending |
                Select-Object -First 1
            if ($Found) {
                return $Found.FullName
            }
        }
    }
    return $null
}

$MakeAppx = Find-WindowsSdkTool "makeappx.exe"
if (-not $MakeAppx) {
    throw "makeappx.exe not found. Install the Windows 10/11 SDK."
}

Write-Host "==> [3/4] Packing MSIX ($MsixOut)..." -ForegroundColor Cyan
& $MakeAppx pack /o /h SHA256 /d $StagingDir /p $MsixOut
if ($LASTEXITCODE -ne 0) {
    throw "makeappx.exe failed with exit code $LASTEXITCODE"
}

if ($SelfSign) {
    $SignTool = Find-WindowsSdkTool "signtool.exe"
    if (-not $SignTool) {
        throw "signtool.exe not found for -SelfSign."
    }
    Write-Host "==> Signing MSIX with self-signed certificate for local testing..." -ForegroundColor Cyan
    $Cert = Get-ChildItem Cert:\CurrentUser\My -CodeSigningCert |
        Where-Object { $_.Subject -eq $CertSubject } |
        Select-Object -First 1
    if (-not $Cert) {
        $Cert = New-SelfSignedCertificate `
            -Type Custom `
            -Subject $CertSubject `
            -KeyUsage DigitalSignature `
            -FriendlyName "SquadAir Local MSIX Test" `
            -CertStoreLocation "Cert:\CurrentUser\My" `
            -TextExtension @("2.5.29.37={text}1.3.6.1.5.5.7.3.3", "2.5.29.19={text}")
    }
    & $SignTool sign /fd SHA256 /sha1 $Cert.Thumbprint $MsixOut
    if ($LASTEXITCODE -ne 0) {
        throw "signtool.exe failed with exit code $LASTEXITCODE"
    }
}

Write-Host "==> [4/4] MSIX package ready:" -ForegroundColor Green
Get-FileHash $MsixOut -Algorithm SHA256 | Format-Table -AutoSize
