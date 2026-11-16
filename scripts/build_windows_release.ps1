param(
    [switch]$SelfSign,
    [string]$CertSubject = "CN=Squad-Air Open Source"
)

$ErrorActionPreference = "Stop"

$RootDir = Split-Path -Parent (Split-Path -Parent $MyInvocation.MyCommand.Path)
Set-Location $RootDir

$IconPath = Join-Path $RootDir "assets\icons\squad-air.ico"
if (-not (Test-Path $IconPath)) {
    throw "Missing Material 3 icon: $IconPath"
}

Write-Host "==> [1/4] Building squad-client (release)..." -ForegroundColor Cyan
cargo build --release -p squad-client
if ($LASTEXITCODE -ne 0) {
    throw "cargo build --release failed with exit code $LASTEXITCODE"
}

$ExePath = Join-Path $RootDir "target\release\squad-client.exe"
if (-not (Test-Path $ExePath)) {
    throw "Expected binary missing: $ExePath"
}

$SignTool = Get-Command "signtool.exe" -ErrorAction SilentlyContinue
$CertThumbprint = $null

if ($SelfSign) {
    Write-Host "==> [2/4] Preparing self-signed Code Signing certificate..." -ForegroundColor Cyan
    $ExistingCert = Get-ChildItem Cert:\CurrentUser\My -CodeSigningCert |
        Where-Object { $_.Subject -eq $CertSubject } |
        Select-Object -First 1

    if (-not $ExistingCert) {
        $ExistingCert = New-SelfSignedCertificate `
            -Type CodeSigningCert `
            -Subject $CertSubject `
            -CertStoreLocation "Cert:\CurrentUser\My"
    }
    $CertThumbprint = $ExistingCert.Thumbprint

    if ($SignTool) {
        Write-Host "==> Signing $ExePath..." -ForegroundColor Cyan
        & $SignTool.Source sign /fd SHA256 /sha1 $CertThumbprint /t http://timestamp.digicert.com $ExePath
    } else {
        Write-Warning "signtool.exe not found in PATH; skipping binary signing."
    }
} else {
    Write-Host "==> [2/4] Skipping Authenticode signing (pass -SelfSign to enable)." -ForegroundColor DarkGray
}

Write-Host "==> [3/4] Compiling Inno Setup installer..." -ForegroundColor Cyan
$Iscc = Get-Command "ISCC.exe" -ErrorAction SilentlyContinue
if (-not $Iscc) {
    $DefaultIscc = "C:\Program Files (x86)\Inno Setup 6\ISCC.exe"
    if (Test-Path $DefaultIscc) {
        $Iscc = Get-Item $DefaultIscc
    }
}

$SetupPath = $null
if ($Iscc) {
    $IssFile = Join-Path $RootDir "installer\squad-air.iss"
    & $Iscc $IssFile
    if ($LASTEXITCODE -ne 0) {
        throw "ISCC.exe failed with exit code $LASTEXITCODE"
    }
    $SetupPath = Join-Path $RootDir "dist\Squad-Air-Setup-windows-x86_64.exe"

    if ($SelfSign -and $SignTool -and $CertThumbprint -and (Test-Path $SetupPath)) {
        Write-Host "==> Signing $SetupPath..." -ForegroundColor Cyan
        & $SignTool.Source sign /fd SHA256 /sha1 $CertThumbprint /t http://timestamp.digicert.com $SetupPath
    }
} else {
    Write-Warning "ISCC.exe (Inno Setup 6) not found; skipped installer packaging."
}

Write-Host "==> [4/4] Release SHA-256 Checksums:" -ForegroundColor Cyan
Get-FileHash $ExePath -Algorithm SHA256 | Format-Table -AutoSize
if ($SetupPath -and (Test-Path $SetupPath)) {
    Get-FileHash $SetupPath -Algorithm SHA256 | Format-Table -AutoSize
}
